use anyhow::{Context, Result, anyhow};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use crate::rtp::RtpPacket;
use crate::udp::{UdpSocketOptions, create_udp_socket};

#[cfg(test)]
use crate::udp::dscp_to_tos;

pub use crate::interfaces::{NetworkInterface, list_ipv4_interfaces, resolve_interface_ip};

const RTP_DSCP: u8 = 34;
const RTP_SEND_BUFFER_SIZE: usize = 1_048_576;
const RTP_RECEIVE_BUFFER_SIZE: usize = 1_048_576;
const SAP_SEND_BUFFER_SIZE: usize = 65_536;

pub(crate) fn rtp_socket_defaults(send_buffer_size: usize) -> UdpSocketOptions {
    UdpSocketOptions {
        reuse_address: true,
        reuse_port: true,
        multicast_loop_v4: true,
        send_buffer_size: Some(send_buffer_size),
        recv_buffer_size: None,
        dscp: Some(RTP_DSCP),
    }
}

pub(crate) fn rtp_receive_socket_defaults(recv_buffer_size: usize) -> UdpSocketOptions {
    UdpSocketOptions {
        reuse_address: true,
        reuse_port: true,
        multicast_loop_v4: false,
        send_buffer_size: None,
        recv_buffer_size: Some(recv_buffer_size),
        dscp: None,
    }
}

pub(crate) fn sap_socket_defaults() -> UdpSocketOptions {
    UdpSocketOptions {
        reuse_address: true,
        reuse_port: true,
        multicast_loop_v4: true,
        send_buffer_size: Some(SAP_SEND_BUFFER_SIZE),
        recv_buffer_size: None,
        dscp: None,
    }
}

/// Multicast socket configuration for AES67 streaming
#[derive(Debug, Clone)]
pub struct MulticastConfig {
    /// Multicast group address (e.g., 239.192.1.1)
    pub multicast_addr: Ipv4Addr,
    /// UDP port number
    pub port: u16,
    /// Local interface IP address (determined from interface name)
    pub local_addr: Ipv4Addr,
    /// TTL for multicast packets
    pub ttl: u8,
    /// Socket send buffer size
    pub send_buffer_size: usize,
}

impl MulticastConfig {
    pub fn new(multicast_addr: Ipv4Addr, port: u16, local_addr: Ipv4Addr) -> Self {
        Self {
            multicast_addr,
            port,
            local_addr,
            ttl: 32, // Default TTL for AES67
            send_buffer_size: RTP_SEND_BUFFER_SIZE,
        }
    }

    /// Get multicast socket address
    pub fn get_multicast_socket_addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(self.multicast_addr), self.port)
    }

    /// Get local bind address
    pub fn get_local_socket_addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(self.local_addr), 0) // Let OS choose port
    }
}

/// Multicast UDP socket wrapper for AES67 streaming
pub struct MulticastSocket {
    socket: UdpSocket,
    config: MulticastConfig,
    target_addr: SocketAddr,
}

#[derive(Debug, Clone)]
pub struct RtpReceiveSocketConfig {
    /// Multicast group or local unicast/loopback address to listen on.
    pub address: Ipv4Addr,
    /// UDP port number.
    pub port: u16,
    /// Local interface IPv4 address used for multicast joins.
    pub interface: Ipv4Addr,
    /// Optional source IPv4 filter.
    pub sender_filter: Option<Ipv4Addr>,
    /// Socket receive buffer size.
    pub recv_buffer_size: usize,
}

impl RtpReceiveSocketConfig {
    pub fn new(address: Ipv4Addr, port: u16, interface: Ipv4Addr) -> Self {
        Self {
            address,
            port,
            interface,
            sender_filter: None,
            recv_buffer_size: RTP_RECEIVE_BUFFER_SIZE,
        }
    }

    pub fn bind_addr(&self) -> SocketAddr {
        let bind_ip = if self.address.is_multicast() {
            Ipv4Addr::UNSPECIFIED
        } else {
            self.address
        };

        SocketAddr::new(IpAddr::V4(bind_ip), self.port)
    }
}

#[derive(Debug, Clone)]
pub struct ReceivedRtpPacket {
    pub packet: RtpPacket,
    pub source: SocketAddr,
}

pub struct RtpReceiveSocket {
    socket: tokio::net::UdpSocket,
    config: RtpReceiveSocketConfig,
}

impl RtpReceiveSocket {
    pub fn new(config: RtpReceiveSocketConfig) -> Result<Self> {
        log::info!(
            "Creating RTP receive socket for {}:{} via interface {}",
            config.address,
            config.port,
            config.interface
        );

        let socket = create_udp_socket(rtp_receive_socket_defaults(config.recv_buffer_size))?;

        socket.bind(&config.bind_addr().into()).with_context(|| {
            format!(
                "Failed to bind RTP receive socket to {}",
                config.bind_addr()
            )
        })?;

        if config.address.is_multicast() {
            socket
                .join_multicast_v4(&config.address, &config.interface)
                .with_context(|| {
                    format!(
                        "Failed to join multicast group {} on interface {}",
                        config.address, config.interface
                    )
                })?;
        }

        socket.set_nonblocking(true)?;
        let socket = tokio::net::UdpSocket::from_std(socket.into())?;

        log::info!("RTP receive socket created at {}", socket.local_addr()?);

        Ok(Self { socket, config })
    }

    pub async fn recv_packet(&self, buffer: &mut [u8]) -> Result<ReceivedRtpPacket> {
        loop {
            let (len, source) = self
                .socket
                .recv_from(buffer)
                .await
                .context("Failed to receive RTP packet")?;

            if !self.source_matches_filter(source) {
                log::debug!(
                    "Dropping RTP packet from {} because it does not match sender filter",
                    source
                );
                continue;
            }

            let packet = RtpPacket::parse(&buffer[..len])
                .with_context(|| format!("Failed to parse RTP packet from {source}"))?;

            return Ok(ReceivedRtpPacket { packet, source });
        }
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.socket
            .local_addr()
            .context("Failed to get RTP receive socket local address")
    }

    pub fn config(&self) -> &RtpReceiveSocketConfig {
        &self.config
    }

    fn source_matches_filter(&self, source: SocketAddr) -> bool {
        match (self.config.sender_filter, source.ip()) {
            (Some(expected), IpAddr::V4(actual)) => actual == expected,
            (Some(_), IpAddr::V6(_)) => false,
            (None, _) => true,
        }
    }
}

impl MulticastSocket {
    /// Create and configure multicast socket
    pub fn new(config: MulticastConfig) -> Result<Self> {
        log::info!(
            "Creating multicast socket for {}:{}",
            config.multicast_addr,
            config.port
        );

        let socket = create_udp_socket(rtp_socket_defaults(config.send_buffer_size))?;

        // Bind to local interface
        // Note: For sending multicast, we usually bind to 0.0.0.0 or the interface IP.
        // If we bind to 127.0.0.1, we might have issues sending to multicast group if routing isn't set.
        // Let's try binding to the specific local address as before.
        socket.bind(&config.get_local_socket_addr().into())?;

        // Configure multicast settings
        socket.set_multicast_ttl_v4(config.ttl as u32)?;

        if !config.local_addr.is_unspecified() {
            socket.set_multicast_if_v4(&config.local_addr)?;
        }

        let socket = UdpSocket::from(socket);
        let target_addr = config.get_multicast_socket_addr();

        log::info!("Multicast socket created successfully");
        log::info!("Local address: {}", socket.local_addr()?);
        log::info!("Target address: {}", target_addr);

        Ok(Self {
            socket,
            config,
            target_addr,
        })
    }

    // Removed configure_multicast as it's now inline

    /// Send RTP packet to multicast group
    pub fn send_packet(&self, packet_data: &[u8]) -> Result<usize> {
        log::debug!(
            "Sending {} bytes to {}",
            packet_data.len(),
            self.target_addr
        );
        let bytes_sent = self
            .socket
            .send_to(packet_data, self.target_addr)
            .with_context(|| {
                format!(
                    "Failed to send packet to multicast group {}",
                    self.target_addr
                )
            })?;

        if bytes_sent != packet_data.len() {
            log::warn!(
                "Partial packet sent: {} of {} bytes",
                bytes_sent,
                packet_data.len()
            );
        }

        Ok(bytes_sent)
    }

    /// Get socket statistics (if available)
    pub fn get_stats(&self) -> Result<SocketStats> {
        let local_addr = self
            .socket
            .local_addr()
            .context("Failed to get local socket address")?;

        // Note: More detailed stats would require platform-specific code
        Ok(SocketStats {
            local_addr,
            target_addr: self.target_addr,
            send_buffer_size: self.config.send_buffer_size,
        })
    }

    /// Get multicast configuration
    pub fn get_config(&self) -> &MulticastConfig {
        &self.config
    }
}

/// Socket statistics and information
#[derive(Debug)]
pub struct SocketStats {
    pub local_addr: SocketAddr,
    pub target_addr: SocketAddr,
    pub send_buffer_size: usize,
}

/// Parse and validate a target stream address.
///
/// Production streams should use multicast. Loopback unicast is accepted for
/// deterministic local and CI media loopback tests.
pub fn parse_stream_address(address: &str) -> Result<Ipv4Addr> {
    let ip = address
        .parse::<Ipv4Addr>()
        .with_context(|| format!("Invalid stream address '{address}'"))?;

    if ip.is_multicast() || ip.is_loopback() {
        Ok(ip)
    } else {
        Err(anyhow!("Stream address {ip} must be multicast or loopback"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::RtpPacketizer;
    use audio::AudioSample;
    use tokio::time::{self, Duration};

    #[test]
    fn test_multicast_config() {
        let config = MulticastConfig::new(
            Ipv4Addr::new(239, 192, 1, 1),
            5004,
            Ipv4Addr::new(192, 168, 1, 100),
        );

        assert_eq!(config.multicast_addr, Ipv4Addr::new(239, 192, 1, 1));
        assert_eq!(config.port, 5004);
        assert_eq!(config.local_addr, Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(config.ttl, 32);
    }

    #[test]
    fn dscp_value_converts_to_ipv4_tos_byte() {
        assert_eq!(dscp_to_tos(0).unwrap(), 0);
        assert_eq!(dscp_to_tos(34).unwrap(), 136);
        assert_eq!(dscp_to_tos(46).unwrap(), 184);
        assert!(dscp_to_tos(64).is_err());
    }

    #[test]
    fn rtp_socket_defaults_use_fixed_professional_values() {
        let config = MulticastConfig::new(
            Ipv4Addr::new(239, 192, 1, 1),
            5004,
            Ipv4Addr::new(192, 168, 1, 100),
        );
        let defaults = rtp_socket_defaults(config.send_buffer_size);

        assert_eq!(config.send_buffer_size, 1_048_576);
        assert!(defaults.reuse_address);
        assert!(defaults.reuse_port);
        assert!(defaults.multicast_loop_v4);
        assert_eq!(defaults.send_buffer_size, Some(1_048_576));
        assert_eq!(defaults.recv_buffer_size, None);
    }

    #[test]
    fn rtp_receive_socket_defaults_use_fixed_professional_values() {
        let defaults = rtp_receive_socket_defaults(RTP_RECEIVE_BUFFER_SIZE);

        assert!(defaults.reuse_address);
        assert!(defaults.reuse_port);
        assert!(!defaults.multicast_loop_v4);
        assert_eq!(defaults.send_buffer_size, None);
        assert_eq!(defaults.recv_buffer_size, Some(RTP_RECEIVE_BUFFER_SIZE));
    }

    #[test]
    fn sap_socket_defaults_use_fixed_control_values() {
        let defaults = sap_socket_defaults();

        assert!(defaults.reuse_address);
        assert!(defaults.reuse_port);
        assert!(defaults.multicast_loop_v4);
        assert_eq!(defaults.send_buffer_size, Some(65_536));
        assert_eq!(defaults.recv_buffer_size, None);
    }

    #[test]
    fn test_socket_addresses() {
        let config = MulticastConfig::new(
            Ipv4Addr::new(239, 192, 1, 1),
            5004,
            Ipv4Addr::new(192, 168, 1, 100),
        );

        let multicast_addr = config.get_multicast_socket_addr();
        assert_eq!(
            multicast_addr.ip(),
            IpAddr::V4(Ipv4Addr::new(239, 192, 1, 1))
        );
        assert_eq!(multicast_addr.port(), 5004);

        let local_addr = config.get_local_socket_addr();
        assert_eq!(local_addr.ip(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)));
        assert_eq!(local_addr.port(), 0); // OS-assigned port
    }

    #[test]
    fn rtp_receive_socket_config_binds_multicast_to_wildcard() {
        let config = RtpReceiveSocketConfig::new(
            Ipv4Addr::new(239, 192, 1, 1),
            5004,
            Ipv4Addr::new(127, 0, 0, 1),
        );

        assert_eq!(
            config.bind_addr(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 5004)
        );
    }

    #[test]
    fn rtp_receive_socket_config_binds_unicast_to_address() {
        let config = RtpReceiveSocketConfig::new(
            Ipv4Addr::new(127, 0, 0, 1),
            5004,
            Ipv4Addr::new(127, 0, 0, 1),
        );

        assert_eq!(
            config.bind_addr(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 5004)
        );
    }

    #[test]
    fn test_interface_resolution() {
        let lo_ip = resolve_interface_ip("lo").unwrap();
        assert_eq!(lo_ip, Ipv4Addr::new(127, 0, 0, 1));

        let direct_ip = resolve_interface_ip("192.168.1.100").unwrap();
        assert_eq!(direct_ip, Ipv4Addr::new(192, 168, 1, 100));
    }

    #[test]
    fn list_ipv4_interfaces_reports_loopback() {
        let interfaces = list_ipv4_interfaces().expect("interfaces should enumerate");

        assert!(
            interfaces
                .iter()
                .any(|interface| interface.ipv4 == Ipv4Addr::new(127, 0, 0, 1)),
            "expected at least one loopback IPv4 interface, got {interfaces:?}"
        );
        assert!(
            interfaces
                .iter()
                .all(|interface| !interface.name.trim().is_empty())
        );
    }

    #[test]
    fn test_unknown_interface_returns_error() {
        let result = resolve_interface_ip("definitely-not-a-real-interface");
        assert!(result.is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_loopback_interface_name_resolves() {
        let lo_ip = resolve_interface_ip("lo0").unwrap();
        assert_eq!(lo_ip, Ipv4Addr::new(127, 0, 0, 1));
    }

    #[test]
    fn test_stream_address_validation() {
        assert_eq!(
            parse_stream_address("239.69.67.67").unwrap(),
            Ipv4Addr::new(239, 69, 67, 67)
        );
        assert_eq!(
            parse_stream_address("127.0.0.1").unwrap(),
            Ipv4Addr::new(127, 0, 0, 1)
        );
        assert!(parse_stream_address("192.168.1.100").is_err());
    }

    #[tokio::test]
    async fn rtp_receive_socket_receives_loopback_packet() -> Result<()> {
        let receiver = RtpReceiveSocket::new(RtpReceiveSocketConfig::new(
            Ipv4Addr::new(127, 0, 0, 1),
            0,
            Ipv4Addr::new(127, 0, 0, 1),
        ))?;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let packet = serialized_test_packet(77);

        sender.send_to(&packet, receiver.local_addr()?).await?;

        let mut buffer = [0u8; 2048];
        let received = time::timeout(Duration::from_secs(2), receiver.recv_packet(&mut buffer))
            .await
            .expect("receiver should get loopback packet")?;

        assert_eq!(received.packet.header.payload_type, 97);
        assert_eq!(received.packet.header.sequence_number, 0);
        assert_eq!(received.packet.header.timestamp, 77);
        assert_eq!(received.source.ip(), sender.local_addr()?.ip());

        Ok(())
    }

    #[tokio::test]
    async fn rtp_receive_socket_accepts_matching_sender_filter() -> Result<()> {
        let mut config = RtpReceiveSocketConfig::new(
            Ipv4Addr::new(127, 0, 0, 1),
            0,
            Ipv4Addr::new(127, 0, 0, 1),
        );
        config.sender_filter = Some(Ipv4Addr::new(127, 0, 0, 1));
        let receiver = RtpReceiveSocket::new(config)?;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let packet = serialized_test_packet(77);

        sender.send_to(&packet, receiver.local_addr()?).await?;

        let mut buffer = [0u8; 2048];
        let received = time::timeout(Duration::from_secs(2), receiver.recv_packet(&mut buffer))
            .await
            .expect("receiver should get packet from matching sender")?;

        assert_eq!(received.packet.header.timestamp, 77);

        Ok(())
    }

    #[tokio::test]
    async fn rtp_receive_socket_drops_non_matching_sender_filter() -> Result<()> {
        let mut config = RtpReceiveSocketConfig::new(
            Ipv4Addr::new(127, 0, 0, 1),
            0,
            Ipv4Addr::new(127, 0, 0, 1),
        );
        config.sender_filter = Some(Ipv4Addr::new(127, 0, 0, 2));
        let receiver = RtpReceiveSocket::new(config)?;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let packet = serialized_test_packet(77);

        sender.send_to(&packet, receiver.local_addr()?).await?;

        let mut buffer = [0u8; 2048];
        let result = time::timeout(
            Duration::from_millis(100),
            receiver.recv_packet(&mut buffer),
        )
        .await;

        assert!(result.is_err(), "filtered packet should not be returned");

        Ok(())
    }

    #[test]
    fn test_multicast_socket_creation() {
        // Test with loopback interface
        let config = MulticastConfig::new(
            Ipv4Addr::new(239, 192, 1, 1),
            5004,
            Ipv4Addr::new(127, 0, 0, 1),
        );

        // This should succeed on any system
        let socket = MulticastSocket::new(config);
        assert!(
            socket.is_ok(),
            "Failed to create multicast socket: {:?}",
            socket.err()
        );

        if let Ok(socket) = socket {
            let stats = socket.get_stats().unwrap();
            println!("Socket stats: {:?}", stats);
        }
    }

    fn serialized_test_packet(timestamp: u32) -> Vec<u8> {
        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let mut packet = Vec::new();
        packetizer
            .write_packet_with_timestamp_into(&sample, timestamp, &mut packet)
            .expect("test RTP packet should serialize");
        packet
    }
}
