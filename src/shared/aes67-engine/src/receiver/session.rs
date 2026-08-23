use anyhow::{anyhow, Context, Result};
use config::PlayerArgs;
use network::{
    decode_l24_payload_interleaved, parse_sdp_file, parse_stream_address, resolve_interface_ip,
    Aes67SessionDescription, AudioEncoding, JitterBufferConfig, JitterBufferStats, PlayoutPacket,
    RtpJitterBuffer, RtpReceiveSocket, RtpReceiveSocketConfig,
};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use tokio::time::{self, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

#[cfg(any(test, debug_assertions))]
use super::output::NullOutput;
use super::output::{build_cpal_output, AudioOutput, OutputStats};

const MIN_RTP_RECEIVE_BUFFER_BYTES: usize = 2048;
const RTP_FIXED_HEADER_BYTES: usize = 12;
const L24_BYTES_PER_SAMPLE: usize = 3;

#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    pub output_device: Option<String>,
    pub latency_ms: u32,
    pub duration: Option<Duration>,
    pub verbose: bool,
    pub test_null_output: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReceiverStats {
    pub packets_received: u64,
    pub packets_accepted: u64,
    pub packets_decoded: u64,
    pub frames_decoded: u64,
    pub silence_frames: u64,
}

pub struct Aes67Receiver {
    receiver: RtpReceiveSocket,
    jitter: RtpJitterBuffer,
    output: Box<dyn AudioOutput + Send>,
    session: Aes67SessionDescription,
    config: ReceiverConfig,
    stats: ReceiverStats,
    started_playout: bool,
    preroll_packets: usize,
}

impl Aes67Receiver {
    pub async fn new(args: &PlayerArgs, config: ReceiverConfig) -> Result<Self> {
        let session = session_from_args(args)?;
        let output = build_receiver_output(&session, &config)?;
        let interface = resolve_interface_ip(args.interface.as_deref().unwrap_or("127.0.0.1"))
            .context("Failed to resolve receive interface")?;
        let sender_filter = args
            .sender
            .as_deref()
            .map(str::parse::<Ipv4Addr>)
            .transpose()
            .context("Invalid sender filter IPv4 address")?;

        let mut receive_config =
            RtpReceiveSocketConfig::new(session.address, session.port, interface);
        receive_config.sender_filter = sender_filter;
        let receiver = RtpReceiveSocket::new(receive_config)?;

        let jitter = RtpJitterBuffer::new(JitterBufferConfig {
            payload_type: session.payload_type,
            ssrc: None,
            frames_per_packet: session.get_frames_per_packet(),
            capacity_packets: jitter_capacity_packets(&session),
        })?;

        let preroll_packets = preroll_packets(config.latency_ms, session.packet_time_ms);

        log::info!(
            "Receiving AES67 stream {}:{} PT={} {:?}/{}Hz/{}ch ptime={}ms latency={}ms",
            session.address,
            session.port,
            session.payload_type,
            session.encoding,
            session.sample_rate,
            session.channels,
            session.packet_time_ms,
            config.latency_ms
        );
        if session.ts_refclk.is_some() || session.mediaclk.is_some() {
            log::info!("PTP clock metadata present; using local-clock playout in this release");
        }

        Ok(Self {
            receiver,
            jitter,
            output,
            session,
            config,
            stats: ReceiverStats::default(),
            started_playout: false,
            preroll_packets,
        })
    }

    pub async fn run_until_cancelled(&mut self, shutdown: CancellationToken) -> Result<()> {
        let mut recv_buffer = vec![0u8; receive_buffer_bytes(&self.session)];
        let mut decode_buffer = Vec::new();
        let start_time = Instant::now();
        let mut playout_interval =
            time::interval(Duration::from_millis(self.session.packet_time_ms as u64));
        playout_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let stop_reason: &'static str;

        loop {
            let remaining = match self.config.duration {
                Some(duration) => match duration.checked_sub(start_time.elapsed()) {
                    Some(remaining) => Some(remaining),
                    None => {
                        stop_reason = "configured duration reached";
                        break;
                    }
                },
                None => None,
            };

            let receive = self.receiver.recv_packet(&mut recv_buffer);
            let received = if let Some(remaining) = remaining {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => {
                        stop_reason = "shutdown requested";
                        break;
                    }
                    _ = playout_interval.tick(), if self.started_playout && !self.jitter.is_empty() => {
                        self.playout_next_packet(&mut decode_buffer)?;
                        continue;
                    }
                    result = time::timeout(remaining, receive) => match result {
                        Ok(received) => received?,
                        Err(_) => {
                            stop_reason = "configured duration reached";
                            break;
                        }
                    }
                }
            } else {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => {
                        stop_reason = "shutdown requested";
                        break;
                    }
                    _ = playout_interval.tick(), if self.started_playout && !self.jitter.is_empty() => {
                        self.playout_next_packet(&mut decode_buffer)?;
                        continue;
                    }
                    received = receive => received?,
                }
            };

            self.stats.packets_received += 1;
            match self.jitter.insert(received.packet)? {
                network::InsertResult::Accepted => {
                    self.stats.packets_accepted += 1;
                }
                network::InsertResult::Duplicate
                | network::InsertResult::Late
                | network::InsertResult::DroppedFull => {}
            }

            self.start_playout_if_prerolled(&mut decode_buffer)?;
        }

        self.decode_available_packets(&mut decode_buffer)?;
        self.log_summary(stop_reason, start_time.elapsed());
        if self.stats.packets_decoded == 0 {
            return Err(anyhow!(
                "no RTP audio packets were decoded before {stop_reason}"
            ));
        }
        Ok(())
    }

    /// Return a copyable SDP description of the RTP stream this receiver joins.
    pub fn get_receiver_sdp(&self) -> String {
        self.session.to_receiver_sdp()
    }

    fn playout_next_packet(&mut self, decode_buffer: &mut Vec<f32>) -> Result<bool> {
        let Some(packet) = self.jitter.pop_next() else {
            return Ok(false);
        };

        self.decode_playout_packet(packet, decode_buffer)?;
        Ok(true)
    }

    fn decode_available_packets(&mut self, decode_buffer: &mut Vec<f32>) -> Result<()> {
        while !self.jitter.is_empty() {
            let Some(packet) = self.jitter.pop_next() else {
                break;
            };
            self.decode_playout_packet(packet, decode_buffer)?;
        }

        Ok(())
    }

    fn decode_playout_packet(
        &mut self,
        packet: PlayoutPacket,
        decode_buffer: &mut Vec<f32>,
    ) -> Result<()> {
        match packet {
            PlayoutPacket::Packet(packet) => {
                let frames = decode_l24_payload_interleaved(
                    &packet.payload,
                    self.session.channels,
                    decode_buffer,
                )?;
                if frames as u32 != self.session.get_frames_per_packet() {
                    return Err(anyhow!(
                        "RTP packet decoded to {frames} frames; expected {}",
                        self.session.get_frames_per_packet()
                    ));
                }
                self.output
                    .write_interleaved(decode_buffer, self.session.channels)?;
                self.stats.packets_decoded += 1;
                self.stats.frames_decoded += frames as u64;
            }
            PlayoutPacket::Silence { frames, .. } => {
                self.output.write_silence(frames, self.session.channels)?;
                self.stats.silence_frames += frames as u64;
            }
        }

        Ok(())
    }

    fn start_playout_if_prerolled(&mut self, decode_buffer: &mut Vec<f32>) -> Result<()> {
        if !self.started_playout && self.jitter.len() >= self.preroll_packets {
            self.decode_available_packets(decode_buffer)?;
            self.started_playout = true;
            self.output.start()?;
            log::info!(
                "Starting playout after preroll of {} packets",
                self.preroll_packets
            );
        }

        Ok(())
    }

    fn log_summary(&self, stop_reason: &str, elapsed: Duration) {
        let jitter_stats = self.jitter.get_stats();
        let output_stats: OutputStats = self.output.get_stats();

        log::info!("AES67 receiver completed:");
        log::info!("  Stop reason: {stop_reason}");
        log::info!("  Duration: {:.2} seconds", elapsed.as_secs_f64());
        log::info!("  Packets received: {}", self.stats.packets_received);
        log::info!("  Packets accepted: {}", self.stats.packets_accepted);
        log::info!("  Packets decoded: {}", self.stats.packets_decoded);
        log::info!("  Frames decoded: {}", self.stats.frames_decoded);
        log::info!("  RTP silence frames: {}", self.stats.silence_frames);
        log::info!("  Jitter lost packets: {}", jitter_stats.lost_packets);
        log::info!("  Jitter late packets: {}", jitter_stats.late_packets);
        log::info!(
            "  Jitter duplicate packets: {}",
            jitter_stats.duplicate_packets
        );
        log::info!(
            "  Jitter dropped-full packets: {}",
            jitter_stats.dropped_full_packets
        );
        log::info!(
            "  Jitter timestamp discontinuities: {}",
            jitter_stats.timestamp_discontinuities
        );
        log::info!("  Output frames: {}", output_stats.frames_written);
        log::info!("  Output samples: {}", output_stats.samples_written);
        log::info!("  Output silence frames: {}", output_stats.silence_frames);
        log::info!("  Output dropped samples: {}", output_stats.dropped_samples);

        for warning in playback_warning_messages(self.stats, jitter_stats, output_stats) {
            log::warn!("{warning}");
        }

        if self.config.verbose {
            log::debug!("  Jitter stats: {jitter_stats:?}");
            log::debug!("  Output stats: {output_stats:?}");
        }
    }
}

fn build_receiver_output(
    session: &Aes67SessionDescription,
    config: &ReceiverConfig,
) -> Result<Box<dyn AudioOutput + Send>> {
    #[cfg(any(test, debug_assertions))]
    if config.test_null_output {
        log::warn!("Using internal null audio output for test validation");
        return Ok(Box::new(NullOutput::default()));
    }

    #[cfg(not(any(test, debug_assertions)))]
    if config.test_null_output {
        return Err(anyhow!(
            "test null output is not available in release builds"
        ));
    }

    build_cpal_output(
        session.sample_rate,
        session.channels,
        config.latency_ms,
        config.output_device.as_deref(),
    )
}

fn session_from_args(args: &PlayerArgs) -> Result<Aes67SessionDescription> {
    if let Some(sdp) = args.sdp.as_deref() {
        return parse_sdp_file(sdp);
    }

    let address = parse_stream_address(
        args.address
            .as_deref()
            .ok_or_else(|| anyhow!("missing receive address"))?,
    )?;
    let port = args.port.ok_or_else(|| anyhow!("missing receive port"))?;
    let channels = args
        .channels
        .ok_or_else(|| anyhow!("missing receive channel count"))?;
    let payload_type = args
        .payload_type
        .ok_or_else(|| anyhow!("missing receive payload type"))?;

    Ok(Aes67SessionDescription {
        session_name: None,
        address,
        ttl: None,
        port,
        payload_type,
        encoding: AudioEncoding::L24,
        sample_rate: 48_000,
        channels,
        packet_time_ms: 1,
        ts_refclk: None,
        mediaclk: None,
    })
}

fn preroll_packets(latency_ms: u32, packet_time_ms: u32) -> usize {
    latency_ms.div_ceil(packet_time_ms).max(1) as usize
}

fn jitter_capacity_packets(session: &Aes67SessionDescription) -> usize {
    let packets_per_second = 1000usize.div_ceil(session.packet_time_ms as usize);
    packets_per_second.max(128)
}

fn receive_buffer_bytes(session: &Aes67SessionDescription) -> usize {
    let payload_bytes =
        session.get_frames_per_packet() as usize * session.channels as usize * L24_BYTES_PER_SAMPLE;

    (RTP_FIXED_HEADER_BYTES + payload_bytes).max(MIN_RTP_RECEIVE_BUFFER_BYTES)
}

fn playback_warning_messages(
    receiver_stats: ReceiverStats,
    jitter_stats: JitterBufferStats,
    output_stats: OutputStats,
) -> Vec<String> {
    let mut warnings = Vec::new();

    if jitter_stats.late_packets > 0
        || jitter_stats.lost_packets > 0
        || receiver_stats.silence_frames > 0
    {
        warnings.push(format!(
            "RTP packets fell behind playout: late_packets={}, lost_packets={}, inserted_silence_frames={}",
            jitter_stats.late_packets, jitter_stats.lost_packets, receiver_stats.silence_frames
        ));
    }

    if jitter_stats.dropped_full_packets > 0 {
        warnings.push(format!(
            "RTP jitter buffer dropped {} packets because the buffer was full",
            jitter_stats.dropped_full_packets
        ));
    }

    if jitter_stats.duplicate_packets > 0 {
        warnings.push(format!(
            "RTP stream contained {} duplicate packets",
            jitter_stats.duplicate_packets
        ));
    }

    if output_stats.silence_frames > 0 {
        warnings.push(format!(
            "Audio output inserted {} silence frames",
            output_stats.silence_frames
        ));
    }

    if output_stats.dropped_samples > 0 {
        warnings.push(format!(
            "Audio output dropped {} samples",
            output_stats.dropped_samples
        ));
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use network::{RtpHeader, RtpPacket};
    use std::sync::{Arc, Mutex};

    #[test]
    fn basic_cli_args_build_default_l24_session() {
        let args = config::parse_receive_listen_args_from([
            "aes67 receive listen",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
        ])
        .unwrap();

        let session = session_from_args(&args).unwrap();

        assert_eq!(session.address, Ipv4Addr::new(239, 192, 1, 1));
        assert_eq!(session.port, 5004);
        assert_eq!(session.payload_type, 97);
        assert_eq!(session.encoding, AudioEncoding::L24);
        assert_eq!(session.sample_rate, 48_000);
        assert_eq!(session.channels, 2);
        assert_eq!(session.packet_time_ms, 1);
        assert_eq!(session.get_frames_per_packet(), 48);
    }

    #[test]
    fn sdp_args_build_session_from_file() {
        let sdp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/example.sdp")
            .canonicalize()
            .unwrap();
        let args = config::parse_receive_listen_args_from([
            "aes67 receive listen",
            "--sdp",
            sdp.to_str().expect("test SDP path should be UTF-8"),
        ])
        .unwrap();

        let session = session_from_args(&args).unwrap();

        assert_eq!(session.address, Ipv4Addr::new(239, 192, 1, 1));
        assert_eq!(session.port, 5004);
        assert_eq!(session.channels, 2);
    }

    #[test]
    fn latency_rounds_up_to_whole_packets() {
        assert_eq!(preroll_packets(50, 1), 50);
        assert_eq!(preroll_packets(51, 2), 26);
        assert_eq!(preroll_packets(1, 2), 1);
    }

    #[test]
    fn receive_buffer_fits_release_target_channel_count() {
        let session = Aes67SessionDescription {
            session_name: None,
            address: Ipv4Addr::LOCALHOST,
            ttl: None,
            port: 5004,
            payload_type: 97,
            encoding: AudioEncoding::L24,
            sample_rate: 48_000,
            channels: 8,
            packet_time_ms: 2,
            ts_refclk: None,
            mediaclk: None,
        };

        let expected_payload_bytes = session.get_frames_per_packet() as usize
            * session.channels as usize
            * L24_BYTES_PER_SAMPLE;
        let expected_packet_bytes = RTP_FIXED_HEADER_BYTES + expected_payload_bytes;

        assert_eq!(receive_buffer_bytes(&session), expected_packet_bytes);
    }

    #[tokio::test]
    async fn preroll_packets_are_written_before_output_starts() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut receiver = test_receiver_with_output(Box::new(RecordingOutput {
            events: events.clone(),
        }));

        receiver.jitter.insert(test_rtp_packet(0, 0)).unwrap();
        receiver.jitter.insert(test_rtp_packet(1, 48)).unwrap();

        let mut decode_buffer = Vec::new();
        receiver
            .start_playout_if_prerolled(&mut decode_buffer)
            .unwrap();

        let events = events.lock().unwrap().clone();
        assert_eq!(
            events.as_slice(),
            ["write:96", "write:96", "start"],
            "preroll audio should be queued before CPAL starts"
        );
        assert!(receiver.started_playout);
        assert_eq!(receiver.stats.packets_decoded, 2);
        assert_eq!(receiver.output.get_stats().frames_written, 96);
    }

    #[tokio::test]
    async fn playout_tick_decodes_only_one_packet_after_preroll() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut receiver = test_receiver_with_output(Box::new(RecordingOutput {
            events: events.clone(),
        }));
        receiver.started_playout = true;

        receiver.jitter.insert(test_rtp_packet(0, 0)).unwrap();
        receiver.jitter.insert(test_rtp_packet(1, 48)).unwrap();

        let mut decode_buffer = Vec::new();
        assert!(receiver.playout_next_packet(&mut decode_buffer).unwrap());

        assert_eq!(receiver.stats.packets_decoded, 1);
        assert_eq!(receiver.output.get_stats().frames_written, 48);
        assert_eq!(receiver.jitter.len(), 1);
        assert_eq!(events.lock().unwrap().as_slice(), ["write:96"]);
    }

    #[test]
    fn playback_warning_messages_report_smoothness_problems() {
        let warnings = playback_warning_messages(
            ReceiverStats {
                silence_frames: 96,
                ..ReceiverStats::default()
            },
            JitterBufferStats {
                late_packets: 2,
                lost_packets: 1,
                dropped_full_packets: 3,
                duplicate_packets: 4,
                ..JitterBufferStats::default()
            },
            OutputStats {
                silence_frames: 48,
                dropped_samples: 6,
                ..OutputStats::default()
            },
        );

        assert_eq!(warnings.len(), 5);
        assert!(warnings[0].contains("fell behind playout"));
        assert!(warnings[1].contains("buffer was full"));
        assert!(warnings[2].contains("duplicate packets"));
        assert!(warnings[3].contains("inserted 48 silence frames"));
        assert!(warnings[4].contains("dropped 6 samples"));
    }

    #[test]
    fn playback_warning_messages_are_empty_for_clean_playout() {
        assert!(playback_warning_messages(
            ReceiverStats::default(),
            JitterBufferStats::default(),
            OutputStats::default()
        )
        .is_empty());
    }

    fn test_receiver_with_output(output: Box<dyn AudioOutput + Send>) -> Aes67Receiver {
        let session = Aes67SessionDescription {
            session_name: None,
            address: Ipv4Addr::LOCALHOST,
            ttl: None,
            port: 0,
            payload_type: 97,
            encoding: AudioEncoding::L24,
            sample_rate: 48_000,
            channels: 2,
            packet_time_ms: 1,
            ts_refclk: None,
            mediaclk: None,
        };
        let receiver = RtpReceiveSocket::new(RtpReceiveSocketConfig::new(
            Ipv4Addr::LOCALHOST,
            0,
            Ipv4Addr::LOCALHOST,
        ))
        .unwrap();
        let jitter = RtpJitterBuffer::new(JitterBufferConfig {
            payload_type: session.payload_type,
            ssrc: None,
            frames_per_packet: session.get_frames_per_packet(),
            capacity_packets: 8,
        })
        .unwrap();

        Aes67Receiver {
            receiver,
            jitter,
            output,
            session,
            config: ReceiverConfig {
                output_device: None,
                latency_ms: 2,
                duration: None,
                verbose: false,
                test_null_output: true,
            },
            stats: ReceiverStats::default(),
            started_playout: false,
            preroll_packets: 2,
        }
    }

    fn test_rtp_packet(sequence_number: u16, timestamp: u32) -> RtpPacket {
        let mut header = RtpHeader::new(97, 0x12345678);
        header.sequence_number = sequence_number;
        header.timestamp = timestamp;

        RtpPacket {
            header,
            payload: vec![0; 48 * 2 * 3],
        }
    }

    struct RecordingOutput {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl AudioOutput for RecordingOutput {
        fn start(&mut self) -> Result<()> {
            self.events.lock().unwrap().push("start".to_string());
            Ok(())
        }

        fn write_interleaved(&mut self, samples: &[f32], _channels: u16) -> Result<usize> {
            self.events
                .lock()
                .unwrap()
                .push(format!("write:{}", samples.len()));
            Ok(samples.len() / 2)
        }

        fn write_silence(&mut self, frames: u32, _channels: u16) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("silence:{frames}"));
            Ok(())
        }

        fn get_stats(&self) -> OutputStats {
            let frames_written = self
                .events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|event| event.strip_prefix("write:"))
                .map(|samples| samples.parse::<u64>().unwrap() / 2)
                .sum();

            OutputStats {
                frames_written,
                samples_written: frames_written * 2,
                silence_frames: 0,
                dropped_samples: 0,
            }
        }
    }
}
