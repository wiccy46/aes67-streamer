use audio::{AudioReader, GainNode, ChainableNode};
use network::{MulticastSocket, MulticastConfig, RtpPacketizer, resolve_interface_ip};
use ptp::{PtpClient, PtpConfig};
use std::net::Ipv4Addr;
use std::thread;
use std::time::{Duration, Instant};
use anyhow::{Result, Context};

/// AES67 Audio Streamer
pub struct Aes67Streamer {
    /// Audio file reader
    audio_reader: AudioReader,
    /// Audio processing chain
    audio_chain: audio::AudioNodeChain,
    /// RTP packet generator
    rtp_packetizer: RtpPacketizer,
    /// Network socket for multicast streaming
    multicast_socket: MulticastSocket,
    /// PTP client for timing synchronization
    ptp_client: PtpClient,
    /// Streaming configuration
    config: StreamConfig,
}

/// Streaming configuration
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Target sample rate for streaming
    pub sample_rate: u32,
    /// Packet time in milliseconds (1ms typical for AES67)
    pub packet_time_ms: u32,
    /// Audio gain in dB
    pub gain_db: f32,
    /// PTP domain (0 for AES67)
    pub ptp_domain: u8,
    /// Enable verbose logging
    pub verbose: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            packet_time_ms: 1,
            gain_db: 0.0,
            ptp_domain: 0,
            verbose: false,
        }
    }
}

impl Aes67Streamer {
    /// Create new AES67 streamer
    pub fn new(
        audio_file: &str,
        multicast_addr: &str,
        port: u16,
        interface: Option<&str>,
        config: StreamConfig,
    ) -> Result<Self> {
        log::info!("Initializing AES67 Streamer");
        
        // Load audio file
        let audio_reader = AudioReader::new(audio_file)
            .context("Failed to load audio file")?;
        
        let audio_info = audio_reader.info();
        log::info!("Loaded audio: {} Hz, {} channels, duration: {:?}", 
                  audio_info.sample_rate, audio_info.channels, 
                  audio_info.duration);
        
        // Create audio processing chain
        let gain_node = GainNode::new_db(config.gain_db);
        let audio_chain = gain_node.into_chain();
        
        // Resolve network interface
        let local_ip = if let Some(iface) = interface {
            resolve_interface_ip(iface)
                .context("Failed to resolve network interface")?
        } else {
            Ipv4Addr::new(127, 0, 0, 1) // Default to loopback
        };
        
        // Parse multicast address
        let multicast_ip: Ipv4Addr = multicast_addr.parse()
            .context("Invalid multicast address")?;
        
        // Create multicast socket
        let multicast_config = MulticastConfig::new(multicast_ip, port, local_ip);
        let multicast_socket = MulticastSocket::new(multicast_config)
            .context("Failed to create multicast socket")?;
        
        // Create PTP client
        let ptp_config = PtpConfig {
            domain: config.ptp_domain,
            interface_ip: local_ip,
            ..Default::default()
        };
        let mut ptp_client = PtpClient::new(ptp_config)
            .context("Failed to create PTP client")?;
        
        // Start PTP synchronization
        ptp_client.start()
            .context("Failed to start PTP client")?;
        
        // Create RTP packetizer
        let payload_type = 97; // Dynamic payload type for AES67
        let ssrc = 0x12345678; // TODO: Generate random SSRC
        let packet_time_us = config.packet_time_ms * 1000;
        let mut rtp_packetizer = RtpPacketizer::new(
            payload_type,
            ssrc,
            config.sample_rate,
            packet_time_us,
        );
        
        // Set initial PTP timestamp
        if let Ok(ptp_timestamp) = ptp_client.rtp_timestamp(config.sample_rate) {
            rtp_packetizer.set_base_timestamp(ptp_timestamp);
            log::info!("RTP base timestamp set from PTP: {}", ptp_timestamp);
        }
        
        log::info!("AES67 Streamer initialized successfully");
        log::info!("Streaming to {}:{} via interface {}", multicast_ip, port, local_ip);
        
        Ok(Self {
            audio_reader,
            audio_chain,
            rtp_packetizer,
            multicast_socket,
            ptp_client,
            config,
        })
    }
    
    /// Start streaming audio
    pub fn start(&mut self) -> Result<()> {
        log::info!("Starting audio stream...");
        
        let mut packets_sent = 0;
        let mut bytes_sent = 0;
        let start_time = Instant::now();
        let target_interval = Duration::from_millis(self.config.packet_time_ms as u64);
        
        loop {
            let loop_start = Instant::now();
            
            // Read next audio frame
            match self.audio_reader.read_next_frame()? {
                Some(mut sample) => {
                    // Process audio through chain
                    self.audio_chain.process(&mut sample)
                        .context("Failed to process audio sample")?;
                    
                    // Process PTP synchronization
                    self.ptp_client.tick()
                        .context("Failed to process PTP synchronization")?;
                    
                    // Create RTP packet with PTP timestamp
                    let rtp_packet = if let Ok(ptp_timestamp) = self.ptp_client.rtp_timestamp(self.config.sample_rate) {
                        self.rtp_packetizer.create_packet_with_timestamp(&sample, ptp_timestamp)
                            .context("Failed to create RTP packet with PTP timestamp")?
                    } else {
                        // Fallback to regular timestamp if PTP fails
                        self.rtp_packetizer.create_packet(&sample)
                            .context("Failed to create RTP packet")?
                    };
                    
                    // Serialize packet for transmission
                    let mut packet_data = rtp_packet.header.to_bytes().to_vec();
                    packet_data.extend(rtp_packet.payload);
                    
                    // Send packet
                    let sent = self.multicast_socket.send_packet(&packet_data)
                        .context("Failed to send RTP packet")?;
                    
                    packets_sent += 1;
                    bytes_sent += sent;
                    
                    if self.config.verbose && packets_sent % 1000 == 0 {
                        let ptp_stats = self.ptp_client.stats();
                        log::info!("Sent {} packets, {} bytes - PTP: {:?}, offset: {}ns", 
                                  packets_sent, bytes_sent, ptp_stats.state, ptp_stats.offset_ns);
                    }
                    
                    // Timing control - maintain packet rate
                    let elapsed = loop_start.elapsed();
                    if elapsed < target_interval {
                        thread::sleep(target_interval - elapsed);
                    }
                    
                } None => {
                    log::info!("End of audio file reached");
                    break;
                }
            }
        }
        
        let total_time = start_time.elapsed();
        log::info!("Streaming completed:");
        log::info!("  Packets sent: {}", packets_sent);
        log::info!("  Bytes sent: {}", bytes_sent);
        log::info!("  Duration: {:.2} seconds", total_time.as_secs_f64());
        log::info!("  Rate: {:.1} packets/sec", packets_sent as f64 / total_time.as_secs_f64());
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_stream_config_default() {
        let config = StreamConfig::default();
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.packet_time_ms, 1);
        assert_eq!(config.gain_db, 0.0);
        assert_eq!(config.ptp_domain, 0);
        assert!(!config.verbose);
    }
    
    #[test]
    fn test_streamer_creation() {
        // This test requires a valid audio file
        let test_file = "../../tests/piano_freesound.wav";
        
        if std::path::Path::new(test_file).exists() {
            let config = StreamConfig::default();
            let streamer = Aes67Streamer::new(
                test_file,
                "239.192.1.1",
                5004,
                Some("127.0.0.1"),
                config,
            );
            
            assert!(streamer.is_ok(), "Failed to create streamer: {:?}", streamer.err());
        }
    }
}