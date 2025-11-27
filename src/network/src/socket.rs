use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use socket2::{Socket, Domain, Type, Protocol};

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
            ttl: 32,                 // Default TTL for AES67
            send_buffer_size: 65536, // 64KB send buffer
        }
    }

    /// Get multicast socket address
    pub fn multicast_socket_addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(self.multicast_addr), self.port)
    }

    /// Get local bind address
    pub fn local_socket_addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(self.local_addr), 0) // Let OS choose port
    }
}

/// Multicast UDP socket wrapper for AES67 streaming
pub struct MulticastSocket {
    socket: UdpSocket,
    config: MulticastConfig,
    target_addr: SocketAddr,
}

impl MulticastSocket {
    /// Create and configure multicast socket
    pub fn new(config: MulticastConfig) -> Result<Self> {
        log::info!(
            "Creating multicast socket for {}:{}",
            config.multicast_addr,
            config.port
        );

        // Create socket using socket2
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        
        // Allow reuse address and port (important for multicast)
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;

        // Bind to local interface
        // Note: For sending multicast, we usually bind to 0.0.0.0 or the interface IP.
        // If we bind to 127.0.0.1, we might have issues sending to multicast group if routing isn't set.
        // Let's try binding to the specific local address as before.
        socket.bind(&config.local_socket_addr().into())?;

        // Configure multicast settings
        socket.set_multicast_ttl_v4(config.ttl as u32)?;
        socket.set_multicast_loop_v4(true)?;
        
        if !config.local_addr.is_unspecified() {
            socket.set_multicast_if_v4(&config.local_addr)?;
        }

        let socket = UdpSocket::from(socket);
        let target_addr = config.multicast_socket_addr();

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
    pub fn config(&self) -> &MulticastConfig {
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

/// Helper function to resolve interface name to IP address
/// For now, this is a placeholder - in full implementation would use platform-specific APIs
pub fn resolve_interface_ip(interface_name: &str) -> Result<Ipv4Addr> {
    match interface_name.to_lowercase().as_str() {
        "lo" | "loopback" => Ok(Ipv4Addr::new(127, 0, 0, 1)),
        // For development: parse direct IP addresses
        ip_str if ip_str.parse::<Ipv4Addr>().is_ok() => ip_str
            .parse::<Ipv4Addr>()
            .context("Failed to parse IP address"),
        // Default to loopback for now - TODO: implement proper interface resolution
        _ => {
            log::warn!(
                "Interface '{}' not recognized, using loopback",
                interface_name
            );
            Ok(Ipv4Addr::new(127, 0, 0, 1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_socket_addresses() {
        let config = MulticastConfig::new(
            Ipv4Addr::new(239, 192, 1, 1),
            5004,
            Ipv4Addr::new(192, 168, 1, 100),
        );

        let multicast_addr = config.multicast_socket_addr();
        assert_eq!(
            multicast_addr.ip(),
            IpAddr::V4(Ipv4Addr::new(239, 192, 1, 1))
        );
        assert_eq!(multicast_addr.port(), 5004);

        let local_addr = config.local_socket_addr();
        assert_eq!(local_addr.ip(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)));
        assert_eq!(local_addr.port(), 0); // OS-assigned port
    }

    #[test]
    fn test_interface_resolution() {
        let lo_ip = resolve_interface_ip("lo").unwrap();
        assert_eq!(lo_ip, Ipv4Addr::new(127, 0, 0, 1));

        let direct_ip = resolve_interface_ip("192.168.1.100").unwrap();
        assert_eq!(direct_ip, Ipv4Addr::new(192, 168, 1, 100));

        let unknown_ip = resolve_interface_ip("unknown").unwrap();
        assert_eq!(unknown_ip, Ipv4Addr::new(127, 0, 0, 1));
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
}
