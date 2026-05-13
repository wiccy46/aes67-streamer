use anyhow::{Context, Result};
use audio::{AudioNode, AudioReader, GainNode};
use network::{
    parse_stream_address, resolve_interface_ip, MulticastConfig, MulticastSocket, RtpPacketizer,
    SapAnnouncer,
};
use ptp::{PtpClient, PtpConfig};
use std::future::{pending, Future};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use tokio::time;

/// AES67 Audio Streamer
pub struct Aes67Streamer {
    audio_reader: AudioReader,
    audio_chain: audio::AudioNodeChain,
    rtp_packetizer: RtpPacketizer,
    multicast_socket: MulticastSocket,
    ptp_client: PtpClient,
    sap_announcer: SapAnnouncer,
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
        }
    }
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
            config.target_sample_rate as usize / (config.packet_time_ms as usize * 1000);

        let audio_reader =
            AudioReader::with_resampling(audio_file, config.target_sample_rate, samples_per_packet)
                .context("Failed to load audio file")?;

        let audio_info = audio_reader.info();
        log::info!(
            "Loaded audio: {} Hz, {} channels, duration: {:?}",
            audio_info.sample_rate,
            audio_info.channels,
            audio_info.duration
        );

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

        let multicast_config = MulticastConfig::new(multicast_ip, port, local_ip);
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

        // Setup SAP Announcer
        // Generate simple SDP
        let sdp = format!(
            "v=0\r\n\
             o=- 123456 123456 IN IP4 {}\r\n\
             s=AES67 Streamer\r\n\
             c=IN IP4 {}/{}\r\n\
             t=0 0\r\n\
             m=audio {} RTP/AVP 97\r\n\
             a=rtpmap:97 L24/{}/2\r\n\
             a=ptime:{}\r\n\
             a=ts-refclk:ptp=IEEE1588-2008:00-00-00-00-00-00-00-00:0\r\n\
             a=mediaclk:direct=0\r\n",
            local_ip,
            multicast_ip,
            1, // TTL
            port,
            config.target_sample_rate,
            config.packet_time_ms
        );

        let sap_announcer =
            SapAnnouncer::new(sdp, local_ip).context("Failed to create SAP announcer")?;
        sap_announcer
            .start()
            .await
            .context("Failed to start SAP announcer")?;

        // Create RTP packetizer using actual sample rate
        let payload_type = 97; // Dynamic payload type for AES67 (24-bit audio)
        let ssrc = 0x12345678; // TODO: Generate random SSRC
        let mut rtp_packetizer = RtpPacketizer::new(payload_type, ssrc);

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
            config,
        })
    }

    #[allow(dead_code)]
    pub async fn start(&mut self) -> Result<()> {
        self.start_until_shutdown(pending()).await
    }

    pub async fn start_until_shutdown<F>(&mut self, shutdown: F) -> Result<()>
    where
        F: Future<Output = ()>,
    {
        log::info!("Starting audio stream...");

        let mut packets_sent = 0;
        let mut bytes_sent = 0;
        let start_time = Instant::now();
        let packet_duration = Duration::from_millis(self.config.packet_time_ms as u64);
        let stop_reason: &'static str;
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
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

                    // PTP is handled in background task now

                    // Create RTP packet with PTP timestamp
                    let rtp_packet = if let Ok(ptp_timestamp) = self
                        .ptp_client
                        .rtp_timestamp(self.config.target_sample_rate)
                    {
                        self.rtp_packetizer
                            .create_packet_with_timestamp(&sample, ptp_timestamp)
                            .context("Failed to create RTP packet with PTP timestamp")?
                    } else {
                        // Fallback to regular timestamp if PTP fails
                        self.rtp_packetizer
                            .create_packet(&sample)
                            .context("Failed to create RTP packet")?
                    };

                    // Serialize packet for transmission
                    let mut packet_data = rtp_packet.header.to_bytes().to_vec();
                    packet_data.extend(rtp_packet.payload);

                    // Send packet
                    let sent = self
                        .multicast_socket
                        .send_packet(&packet_data)
                        .context("Failed to send RTP packet")?;

                    packets_sent += 1;
                    bytes_sent += sent;

                    if packets_sent % 100 == 0 {
                        log::debug!("Sent packet {}", packets_sent);
                    }

                    if self.config.verbose && packets_sent % 1000 == 0 {
                        let ptp_stats = self.ptp_client.get_stats();
                        log::debug!(
                            "Sent {} packets, {} bytes - PTP: {:?}, offset: {}ns",
                            packets_sent,
                            bytes_sent,
                            ptp_stats.state,
                            ptp_stats.offset_ns
                        );
                    }

                    // Timing control - maintain packet rate
                    // Calculate when this packet should be sent based on audio timeline
                    let target_time = start_time + packet_duration * packets_sent as u32;
                    let now = Instant::now();

                    if now < target_time {
                        tokio::select! {
                            biased;
                            _ = &mut shutdown => {
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

        // Stop background services
        log::info!("Stopping background services...");
        self.sap_announcer.stop();
        self.ptp_client.stop();
        log::info!("Background services stopped");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_config_default() {
        let config = StreamConfig::default();
        assert_eq!(config.target_sample_rate, 48000);
        assert_eq!(config.packet_time_ms, 1);
        assert_eq!(config.gain_db, 0.0);
        assert_eq!(config.ptp_domain, 0);
        assert_eq!(config.duration, None);
        assert!(!config.verbose);
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
}
