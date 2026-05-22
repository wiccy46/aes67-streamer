use anyhow::{anyhow, Context, Result};
use config::PlayerArgs;
use network::{
    decode_l24_payload_interleaved, parse_sdp_file, parse_stream_address, resolve_interface_ip,
    Aes67SessionDescription, AudioEncoding, JitterBufferConfig, PlayoutPacket, RtpJitterBuffer,
    RtpReceiveSocket, RtpReceiveSocketConfig,
};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::output::{build_output, AudioOutput, OutputMode, OutputStats};

const RTP_RECEIVE_BUFFER_BYTES: usize = 2048;

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    pub output_mode: OutputMode,
    pub latency_ms: u32,
    pub duration: Option<Duration>,
    pub verbose: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlayerStats {
    pub packets_received: u64,
    pub packets_accepted: u64,
    pub packets_decoded: u64,
    pub frames_decoded: u64,
    pub silence_frames: u64,
}

pub struct Aes67Player {
    receiver: RtpReceiveSocket,
    jitter: RtpJitterBuffer,
    output: Box<dyn AudioOutput + Send>,
    session: Aes67SessionDescription,
    config: PlayerConfig,
    stats: PlayerStats,
    started_playout: bool,
    preroll_packets: usize,
}

impl Aes67Player {
    pub async fn new(args: &PlayerArgs, config: PlayerConfig) -> Result<Self> {
        let output = build_output(config.output_mode)?;
        let session = session_from_args(args)?;
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
            frames_per_packet: session.frames_per_packet(),
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
            stats: PlayerStats::default(),
            started_playout: false,
            preroll_packets,
        })
    }

    pub async fn run_until_cancelled(&mut self, shutdown: CancellationToken) -> Result<()> {
        let mut recv_buffer = [0u8; RTP_RECEIVE_BUFFER_BYTES];
        let mut decode_buffer = Vec::new();
        let start_time = Instant::now();
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

            if !self.started_playout && self.jitter.len() >= self.preroll_packets {
                self.started_playout = true;
                log::info!(
                    "Starting null playout after preroll of {} packets",
                    self.preroll_packets
                );
            }

            if self.started_playout {
                self.decode_available_packets(&mut decode_buffer)?;
            }
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

    fn decode_available_packets(&mut self, decode_buffer: &mut Vec<f32>) -> Result<()> {
        while !self.jitter.is_empty() {
            match self.jitter.pop_next() {
                Some(PlayoutPacket::Packet(packet)) => {
                    let frames = decode_l24_payload_interleaved(
                        &packet.payload,
                        self.session.channels,
                        decode_buffer,
                    )?;
                    if frames as u32 != self.session.frames_per_packet() {
                        return Err(anyhow!(
                            "RTP packet decoded to {frames} frames; expected {}",
                            self.session.frames_per_packet()
                        ));
                    }
                    self.output
                        .write_interleaved(decode_buffer, self.session.channels)?;
                    self.stats.packets_decoded += 1;
                    self.stats.frames_decoded += frames as u64;
                }
                Some(PlayoutPacket::Silence { frames, .. }) => {
                    self.output.write_silence(frames, self.session.channels)?;
                    self.stats.silence_frames += frames as u64;
                }
                None => break,
            }
        }

        Ok(())
    }

    fn log_summary(&self, stop_reason: &str, elapsed: Duration) {
        let jitter_stats = self.jitter.stats();
        let output_stats: OutputStats = self.output.stats();

        log::info!("AES67 player completed:");
        log::info!("  Stop reason: {stop_reason}");
        log::info!("  Duration: {:.2} seconds", elapsed.as_secs_f64());
        log::info!("  Packets received: {}", self.stats.packets_received);
        log::info!("  Packets accepted: {}", self.stats.packets_accepted);
        log::info!("  Packets decoded: {}", self.stats.packets_decoded);
        log::info!("  Frames decoded: {}", self.stats.frames_decoded);
        log::info!("  Jitter lost packets: {}", jitter_stats.lost_packets);
        log::info!("  Jitter late packets: {}", jitter_stats.late_packets);
        log::info!(
            "  Jitter duplicate packets: {}",
            jitter_stats.duplicate_packets
        );
        log::info!("  Output frames: {}", output_stats.frames_written);
        log::info!("  Output silence frames: {}", output_stats.silence_frames);

        if self.config.verbose {
            log::debug!("  Jitter stats: {jitter_stats:?}");
            log::debug!("  Output stats: {output_stats:?}");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_cli_args_build_default_l24_session() {
        let args = config::parse_player_args_from([
            "aes67-player",
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
        assert_eq!(session.frames_per_packet(), 48);
    }

    #[test]
    fn sdp_args_build_session_from_file() {
        let sdp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/example.sdp")
            .canonicalize()
            .unwrap();
        let args = config::parse_player_args_from([
            "aes67-player",
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
}
