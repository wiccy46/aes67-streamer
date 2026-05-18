use anyhow::{Context, Result};
use audio::{AudioNode, AudioReader, GainNode};
use network::{
    parse_stream_address, resolve_interface_ip, MulticastConfig, MulticastSocket, RtpPacketizer,
    SapAnnouncer,
};
use ptp::{ClockIdentity, PtpClient, PtpConfig};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use tokio::time;
use tokio_util::sync::CancellationToken;

const MAX_RELEASE_CHANNELS: u32 = 8;
const L24_BYTES_PER_SAMPLE: usize = 3;
const MAX_IPV4_RTP_AUDIO_PAYLOAD_BYTES: usize = 1460;

/// AES67 Audio Streamer
pub struct Aes67Streamer {
    audio_reader: AudioReader,
    audio_chain: audio::AudioNodeChain,
    rtp_packetizer: RtpPacketizer,
    multicast_socket: MulticastSocket,
    ptp_client: PtpClient,
    sap_announcer: Option<SapAnnouncer>,
    sdp_context: SdpContext,
    clock_identity: ClockIdentity,
    config: StreamConfig,
}

#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Target sample rate for streaming, for now only support 48000 Hz
    pub target_sample_rate: u32,
    /// Packet time in milliseconds (1ms typical for AES67, 48 samples per packet, can be lower for RAVENNA)
    pub packet_time_ms: u32,
    /// Gain in decibels
    pub gain_db: f32,
    /// PTP domain (0 for AES67)
    pub ptp_domain: u8,
    pub verbose: bool,
    /// Optional maximum stream duration for tests and scripted runs.
    pub duration: Option<Duration>,
    pub loop_playback: bool,
    pub ttl: u8,
    pub sap: bool,
    pub payload_type: u8,
    pub ssrc: Option<u32>,
    pub session_name: String,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            target_sample_rate: 48000,
            packet_time_ms: 1,
            gain_db: 0.0,
            ptp_domain: 0,
            verbose: false,
            duration: None,
            loop_playback: false,
            ttl: 32,
            sap: true,
            payload_type: 97,
            ssrc: None,
            session_name: "AES67 Stream".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SdpContext {
    local_ip: Ipv4Addr,
    multicast_ip: Ipv4Addr,
    port: u16,
    audio_channels: u32,
}

/// An Aes67Streamer is the main entry of the app
/// It loads an audio file, creates a multicast udp socket,
/// packetize the audio data, and sends it over the network.
impl Aes67Streamer {
    pub async fn new(
        audio_file: &str,
        multicast_addr: &str,
        port: u16,
        interface: Option<&str>,
        config: StreamConfig,
    ) -> Result<Self> {
        log::info!("Initializing AES67 Streamer...");
        let samples_per_packet =
            samples_per_packet(config.target_sample_rate, config.packet_time_ms);

        let audio_reader =
            AudioReader::with_resampling(audio_file, config.target_sample_rate, samples_per_packet)
                .context("Failed to load audio file")?;

        let audio_info = audio_reader.get_info();
        log::info!(
            "Loaded audio: {} Hz, {} channels, duration: {:?}",
            audio_info.sample_rate,
            audio_info.channels,
            audio_info.duration
        );
        validate_stream_audio_format(audio_info, samples_per_packet)
            .context("Unsupported audio format for AES67 stream")?;

        // Create audio processing chain
        let gain_node = GainNode::new_db(config.gain_db);
        let audio_chain = gain_node.into_chain();

        // Resolve network interface
        let local_ip = if let Some(iface) = interface {
            resolve_interface_ip(iface).context("Failed to resolve network interface")?
        } else {
            Ipv4Addr::new(127, 0, 0, 1)
        };

        let multicast_ip: Ipv4Addr =
            parse_stream_address(multicast_addr).context("Invalid stream address")?;

        let mut multicast_config = MulticastConfig::new(multicast_ip, port, local_ip);
        multicast_config.ttl = config.ttl;
        let multicast_socket =
            MulticastSocket::new(multicast_config).context("Failed to create multicast socket")?;

        let ptp_config = PtpConfig {
            domain: config.ptp_domain,
            interface_ip: local_ip,
            ..Default::default()
        };
        let ptp_client = PtpClient::new(ptp_config);
        ptp_client
            .start()
            .await
            .context("Failed to start PTP client")?;

        let ptp_stats = ptp_client.get_stats();
        let clock_identity = ptp_client.get_reference_clock_identity();
        if ptp_stats.master_identity.is_none() {
            log::warn!(
                "No PTP grandmaster observed yet; SDP will use local clock identity {clock_identity}"
            );
        }

        let sdp_context = SdpContext {
            local_ip,
            multicast_ip,
            port,
            audio_channels: audio_info.channels,
        };
        let sdp = build_sdp(&sdp_context, clock_identity, &config);
        log::info!("Generated SDP:\n{sdp}");

        let sap_announcer = if config.sap {
            let sap_announcer =
                SapAnnouncer::new(sdp, local_ip).context("Failed to create SAP announcer")?;
            sap_announcer
                .start()
                .await
                .context("Failed to start SAP announcer")?;
            Some(sap_announcer)
        } else {
            None
        };

        // Create RTP packetizer using actual sample rate
        let ssrc = resolve_ssrc(config.ssrc);
        if config.ssrc.is_none() {
            log::info!("Generated RTP SSRC: 0x{ssrc:08X}");
        }
        let mut rtp_packetizer = RtpPacketizer::new(config.payload_type, ssrc);

        // Set initial PTP timestamp
        if let Ok(ptp_timestamp) = ptp_client.rtp_timestamp(config.target_sample_rate) {
            rtp_packetizer.set_base_timestamp(ptp_timestamp);
            log::info!("RTP base timestamp set from PTP: {ptp_timestamp}");
        }

        log::info!("AES67 Streamer initialized successfully");
        log::info!("Streaming to {multicast_ip}:{port} via interface {local_ip}");

        Ok(Self {
            audio_reader,
            audio_chain,
            rtp_packetizer,
            multicast_socket,
            ptp_client,
            sap_announcer,
            sdp_context,
            clock_identity,
            config,
        })
    }

    pub async fn run_until_cancelled(&mut self, shutdown: CancellationToken) -> Result<()> {
        log::info!("Starting audio stream...");

        let mut packets_sent = 0;
        let mut bytes_sent = 0;
        let start_time = Instant::now();
        let packet_duration = Duration::from_millis(self.config.packet_time_ms as u64);
        let mut next_sdp_refresh_check = start_time + Duration::from_secs(10);
        let debug_packet_logging = log::log_enabled!(log::Level::Debug);
        let mut next_debug_packet_log = 100;
        let mut next_verbose_stats_log = 1000;
        let mut timing_drift = TimingDriftStats::default();
        let rtp_packet_capacity = 12
            + samples_per_packet(self.config.target_sample_rate, self.config.packet_time_ms)
                * self.audio_reader.get_info().channels as usize
                * L24_BYTES_PER_SAMPLE;
        let mut rtp_packet_buffer = Vec::with_capacity(rtp_packet_capacity);
        let stop_reason: &'static str;

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    stop_reason = "shutdown requested";
                    log::info!("Shutdown requested, stopping audio stream...");
                    break;
                }
                _ = async {} => {}
            }

            if let Some(duration) = self.config.duration {
                if start_time.elapsed() >= duration {
                    stop_reason = "configured duration reached";
                    log::info!(
                        "Configured stream duration reached after {:.2} seconds",
                        start_time.elapsed().as_secs_f64()
                    );
                    break;
                }
            }

            // Read next audio frame
            match self.audio_reader.read_next_frame()? {
                Some(mut sample) => {
                    // Process audio through chain
                    self.audio_chain
                        .process(&mut sample)
                        .context("Failed to process audio sample")?;

                    self.rtp_packetizer
                        .write_packet_into(&sample, &mut rtp_packet_buffer)
                        .context("Failed to create RTP packet")?;

                    // Send packet
                    let sent = self
                        .multicast_socket
                        .send_packet(&rtp_packet_buffer)
                        .context("Failed to send RTP packet")?;

                    packets_sent += 1;
                    bytes_sent += sent;

                    if debug_packet_logging && packets_sent >= next_debug_packet_log {
                        log::debug!("Sent packet {}", packets_sent);
                        next_debug_packet_log += 100;
                    }

                    let packet_sent_at = Instant::now();
                    if packet_sent_at >= next_sdp_refresh_check {
                        self.refresh_sdp_if_ptp_reference_changed();
                        next_sdp_refresh_check = packet_sent_at + Duration::from_secs(10);
                    }

                    if self.config.verbose && packets_sent >= next_verbose_stats_log {
                        let ptp_stats = self.ptp_client.get_stats();
                        log::debug!(
                            "Sent {} packets, {} bytes - PTP: {:?}, offset: {}ns",
                            packets_sent,
                            bytes_sent,
                            ptp_stats.state,
                            ptp_stats.offset_ns
                        );
                        next_verbose_stats_log += 1000;
                    }

                    // Timing control - maintain packet rate
                    // Calculate when this packet should be sent based on audio timeline
                    let target_time = start_time + packet_duration * packets_sent as u32;
                    let now = packet_sent_at;
                    timing_drift.observe(now, target_time);

                    if now < target_time {
                        tokio::select! {
                            biased;
                            _ = shutdown.cancelled() => {
                                stop_reason = "shutdown requested";
                                log::info!("Shutdown requested, stopping audio stream...");
                                break;
                            }
                            _ = time::sleep(target_time - now) => {}
                        }
                    } else if packets_sent % 1000 == 0 && now > target_time + packet_duration {
                        // Warn if we're falling behind real-time
                        let behind_ms = (now - target_time).as_millis();
                        log::warn!(
                            "Streaming falling behind by {}ms at packet {}",
                            behind_ms,
                            packets_sent
                        );
                    }
                }
                None => {
                    if self.config.loop_playback {
                        if self.audio_reader.can_read_full_packet() {
                            log::debug!("End of audio file reached, restarting from beginning");
                            self.audio_reader.rewind();
                            continue;
                        }

                        log::warn!(
                            "Loop playback requested, but audio file is shorter than one packet"
                        );
                    }

                    stop_reason = "end of audio file";
                    log::info!("End of audio file reached");
                    break;
                }
            }
        }

        let total_time = start_time.elapsed();
        log::info!("Streaming completed:");
        log::info!("  Stop reason: {stop_reason}");
        log::info!("  Packets sent: {packets_sent}");
        log::info!("  Bytes sent: {bytes_sent}");
        log::info!("  Duration: {:.2} seconds", total_time.as_secs_f64());
        log::info!(
            "  Rate: {:.1} packets/sec",
            packets_sent as f64 / total_time.as_secs_f64()
        );
        log::info!(
            "  Timing late packets: {}/{}",
            timing_drift.get_late_packets(),
            timing_drift.get_packets_observed()
        );
        log::info!(
            "  Timing max lateness: {:.3} ms",
            duration_ms(timing_drift.get_max_lateness())
        );
        log::info!(
            "  Timing avg late-packet lateness: {:.3} ms",
            timing_drift.get_average_late_lateness_ms()
        );

        // Stop background services
        log::info!("Stopping background services...");
        if let Some(sap_announcer) = &self.sap_announcer {
            sap_announcer.shutdown().await;
        }
        self.ptp_client.shutdown().await;
        log::info!("Background services stopped");

        Ok(())
    }

    fn refresh_sdp_if_ptp_reference_changed(&mut self) {
        let next_identity = self.ptp_client.get_reference_clock_identity();
        if let Some(sdp) = refresh_sdp_for_clock_identity(
            &mut self.clock_identity,
            next_identity,
            &self.sdp_context,
            &self.config,
        ) {
            if let Some(sap_announcer) = &self.sap_announcer {
                sap_announcer.update_sdp_payload(sdp.clone());
            }
            log::info!(
                "PTP reference changed; refreshed SDP/SAP announcement for {}",
                self.clock_identity
            );
            log::info!("Generated SDP:\n{sdp}");
        }
    }
}

fn samples_per_packet(sample_rate: u32, packet_time_ms: u32) -> usize {
    (sample_rate as usize * packet_time_ms as usize) / 1000
}

#[derive(Debug, Default)]
struct TimingDriftStats {
    packets_observed: u64,
    late_packets: u64,
    total_lateness: Duration,
    max_lateness: Duration,
}

impl TimingDriftStats {
    fn observe(&mut self, packet_sent_at: Instant, target_time: Instant) {
        self.packets_observed += 1;

        let Some(lateness) = packet_sent_at.checked_duration_since(target_time) else {
            return;
        };

        if lateness.is_zero() {
            return;
        }

        self.late_packets += 1;
        self.total_lateness += lateness;
        self.max_lateness = self.max_lateness.max(lateness);
    }

    fn get_packets_observed(&self) -> u64 {
        self.packets_observed
    }

    fn get_late_packets(&self) -> u64 {
        self.late_packets
    }

    fn get_max_lateness(&self) -> Duration {
        self.max_lateness
    }

    fn get_average_late_lateness_ms(&self) -> f64 {
        if self.late_packets == 0 {
            return 0.0;
        }

        duration_ms(self.total_lateness) / self.late_packets as f64
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn validate_stream_audio_format(info: &audio::AudioInfo, samples_per_packet: usize) -> Result<()> {
    if !(1..=MAX_RELEASE_CHANNELS).contains(&info.channels) {
        anyhow::bail!(
            "first release supports 1 to 8 channels, but input has {} channels",
            info.channels
        );
    }

    let payload_bytes = samples_per_packet * info.channels as usize * L24_BYTES_PER_SAMPLE;
    if payload_bytes > MAX_IPV4_RTP_AUDIO_PAYLOAD_BYTES {
        anyhow::bail!(
            "RTP audio payload would be {payload_bytes} bytes; reduce packet_time_ms or channel count to fit within {MAX_IPV4_RTP_AUDIO_PAYLOAD_BYTES} bytes"
        );
    }

    Ok(())
}

fn resolve_ssrc(configured_ssrc: Option<u32>) -> u32 {
    configured_ssrc.unwrap_or_else(generate_random_ssrc)
}

fn generate_random_ssrc() -> u32 {
    loop {
        let ssrc = rand::random::<u32>();
        if ssrc != 0 && ssrc != 0x12345678 {
            return ssrc;
        }
    }
}

fn build_sdp(context: &SdpContext, clock_identity: ClockIdentity, config: &StreamConfig) -> String {
    format!(
        "v=0\r\n\
         o=- 123456 123456 IN IP4 {}\r\n\
         s={}\r\n\
         c=IN IP4 {}/{}\r\n\
         t=0 0\r\n\
         m=audio {} RTP/AVP {}\r\n\
         a=rtpmap:{} L24/{}/{}\r\n\
         a=ptime:{}\r\n\
         a=ts-refclk:ptp=IEEE1588-2008:{}:{}\r\n\
         a=mediaclk:direct=0\r\n",
        context.local_ip,
        config.session_name,
        context.multicast_ip,
        config.ttl,
        context.port,
        config.payload_type,
        config.payload_type,
        config.target_sample_rate,
        context.audio_channels,
        config.packet_time_ms,
        clock_identity,
        config.ptp_domain
    )
}

fn refresh_sdp_for_clock_identity(
    current_identity: &mut ClockIdentity,
    next_identity: ClockIdentity,
    context: &SdpContext,
    config: &StreamConfig,
) -> Option<String> {
    if *current_identity == next_identity {
        return None;
    }

    log::info!(
        "PTP reference changed: {} -> {}",
        current_identity,
        next_identity
    );
    *current_identity = next_identity;
    Some(build_sdp(context, next_identity, config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_stream_config_default() {
        let config = StreamConfig::default();
        assert_eq!(config.target_sample_rate, 48000);
        assert_eq!(config.packet_time_ms, 1);
        assert_eq!(config.gain_db, 0.0);
        assert_eq!(config.ptp_domain, 0);
        assert_eq!(config.duration, None);
        assert!(!config.loop_playback);
        assert_eq!(config.ttl, 32);
        assert!(config.sap);
        assert_eq!(config.payload_type, 97);
        assert_eq!(config.ssrc, None);
        assert_eq!(config.session_name, "AES67 Stream");
        assert!(!config.verbose);
    }

    #[test]
    fn generated_ssrc_when_unspecified_is_nonzero_and_not_legacy_default() {
        let generated = resolve_ssrc(None);

        assert_ne!(generated, 0);
        assert_ne!(generated, 0x12345678);
    }

    #[test]
    fn configured_ssrc_is_used_without_randomization() {
        assert_eq!(resolve_ssrc(Some(0xDEADBEEF)), 0xDEADBEEF);
    }

    #[tokio::test]
    async fn test_streamer_creation() {
        // This test requires a valid audio file
        let test_file = "../../tests/piano_freesound.wav";

        if std::path::Path::new(test_file).exists() {
            let config = StreamConfig::default();
            let streamer =
                Aes67Streamer::new(test_file, "239.192.1.1", 5004, Some("127.0.0.1"), config).await;

            assert!(
                streamer.is_ok(),
                "Failed to create streamer: {:?}",
                streamer.err()
            );
        }
    }

    #[tokio::test]
    async fn streamer_creation_rejects_more_than_eight_channels() {
        let test_file = create_test_wav_file(9, 48);

        let result = Aes67Streamer::new(
            test_file.to_str().expect("temp path should be utf-8"),
            "239.192.1.1",
            5004,
            Some("127.0.0.1"),
            StreamConfig::default(),
        )
        .await;

        std::fs::remove_file(test_file).ok();

        let Err(error) = result else {
            panic!("9-channel file should be rejected");
        };
        assert!(
            error.to_string().contains("Unsupported audio format"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn test_sdp_uses_stream_metadata() {
        let config = StreamConfig {
            payload_type: 101,
            session_name: "Configured Stream".to_string(),
            packet_time_ms: 2,
            ttl: 12,
            ..Default::default()
        };

        let context = SdpContext {
            local_ip: Ipv4Addr::new(127, 0, 0, 1),
            multicast_ip: Ipv4Addr::new(239, 10, 20, 30),
            port: 6000,
            audio_channels: 8,
        };
        let sdp = build_sdp(
            &context,
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x12, 0x34, 0x56]),
            &config,
        );

        assert!(sdp.contains("s=Configured Stream\r\n"));
        assert!(sdp.contains("c=IN IP4 239.10.20.30/12\r\n"));
        assert!(sdp.contains("m=audio 6000 RTP/AVP 101\r\n"));
        assert!(sdp.contains("a=rtpmap:101 L24/48000/8\r\n"));
        assert!(sdp.contains("a=ptime:2\r\n"));
        assert!(sdp.contains("a=ts-refclk:ptp=IEEE1588-2008:00-1D-C1-FF-FE-12-34-56:0\r\n"));
    }

    #[test]
    fn test_samples_per_packet_uses_packet_time() {
        assert_eq!(samples_per_packet(48_000, 1), 48);
        assert_eq!(samples_per_packet(48_000, 2), 96);
    }

    #[test]
    fn timing_drift_stats_tracks_late_packets() {
        let base = Instant::now();
        let mut stats = TimingDriftStats::default();

        stats.observe(base, base + Duration::from_millis(1));
        stats.observe(
            base + Duration::from_millis(3),
            base + Duration::from_millis(1),
        );
        stats.observe(
            base + Duration::from_millis(8),
            base + Duration::from_millis(5),
        );

        assert_eq!(stats.get_packets_observed(), 3);
        assert_eq!(stats.get_late_packets(), 2);
        assert_eq!(stats.get_max_lateness(), Duration::from_millis(3));
        assert_eq!(stats.get_average_late_lateness_ms(), 2.5);
    }

    #[test]
    fn timing_drift_stats_reports_zero_when_no_packets_are_late() {
        let base = Instant::now();
        let mut stats = TimingDriftStats::default();

        stats.observe(base, base);
        stats.observe(
            base + Duration::from_millis(1),
            base + Duration::from_millis(2),
        );

        assert_eq!(stats.get_packets_observed(), 2);
        assert_eq!(stats.get_late_packets(), 0);
        assert_eq!(stats.get_max_lateness(), Duration::ZERO);
        assert_eq!(stats.get_average_late_lateness_ms(), 0.0);
    }

    #[test]
    fn audio_format_validation_accepts_release_target_eight_channels() {
        let info = audio::AudioInfo {
            sample_rate: 48_000,
            channels: 8,
            duration: None,
            bit_depth: Some(24),
            format: "test".to_string(),
        };

        validate_stream_audio_format(&info, 48).expect("8-channel 1ms L24 should be supported");
    }

    #[test]
    fn audio_format_validation_rejects_more_than_eight_channels() {
        let info = audio::AudioInfo {
            sample_rate: 48_000,
            channels: 9,
            duration: None,
            bit_depth: Some(24),
            format: "test".to_string(),
        };

        let error = validate_stream_audio_format(&info, 48)
            .expect_err("first release supports up to 8 channels");

        assert!(
            error.to_string().contains("supports 1 to 8 channels"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn audio_format_validation_rejects_payloads_that_exceed_standard_mtu() {
        let info = audio::AudioInfo {
            sample_rate: 48_000,
            channels: 8,
            duration: None,
            bit_depth: Some(24),
            format: "test".to_string(),
        };

        let error = validate_stream_audio_format(&info, 96)
            .expect_err("8-channel 2ms L24 should exceed one RTP packet payload");

        assert!(
            error.to_string().contains("RTP audio payload"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn refresh_sdp_when_clock_identity_changes_builds_updated_sdp_once() {
        let mut current_identity =
            ClockIdentity::from_bytes([0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01]);
        let next_identity =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x12, 0x34, 0x56]);
        let context = SdpContext {
            local_ip: Ipv4Addr::new(127, 0, 0, 1),
            multicast_ip: Ipv4Addr::new(239, 10, 20, 30),
            port: 6000,
            audio_channels: 2,
        };
        let config = StreamConfig::default();

        let refreshed =
            refresh_sdp_for_clock_identity(&mut current_identity, next_identity, &context, &config)
                .expect("identity change should rebuild SDP");

        assert_eq!(current_identity, next_identity);
        assert!(refreshed.contains("a=ts-refclk:ptp=IEEE1588-2008:00-1D-C1-FF-FE-12-34-56:0\r\n"));
        assert!(refresh_sdp_for_clock_identity(
            &mut current_identity,
            next_identity,
            &context,
            &config,
        )
        .is_none());
    }

    fn create_test_wav_file(channels: u16, frames: usize) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aes67-streamer-format-validation-{}-{}ch.wav",
            std::process::id(),
            channels
        ));
        let spec = hound::WavSpec {
            channels,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("test WAV should open");

        for frame in 0..frames {
            for channel in 0..channels {
                let frequency = 440.0 + channel as f32 * 10.0;
                let sample =
                    (2.0 * std::f32::consts::PI * frequency * frame as f32 / 48_000.0).sin();
                writer
                    .write_sample(sample * 0.5)
                    .expect("sample should write");
            }
        }

        writer.finalize().expect("test WAV should finalize");
        path
    }
}
