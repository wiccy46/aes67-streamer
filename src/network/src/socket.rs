use anyhow::{Context, Result, anyhow};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

const RTP_DSCP: u8 = 34;

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
        socket.set_tos_v4(dscp_to_tos(RTP_DSCP)?)?;

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

pub(crate) fn dscp_to_tos(dscp: u8) -> Result<u32> {
    if dscp > 63 {
        return Err(anyhow!("DSCP value {dscp} must be between 0 and 63"));
    }

    Ok((dscp as u32) << 2)
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

/// Resolve an interface name or direct IPv4 address to an interface IPv4 address.
pub fn resolve_interface_ip(interface_name: &str) -> Result<Ipv4Addr> {
    let interface_name = interface_name.trim();
    if interface_name.is_empty() {
        return Err(anyhow!("Network interface cannot be empty"));
    }

    if let Ok(ip) = interface_name.parse::<Ipv4Addr>() {
        return Ok(ip);
    }

    match interface_name.to_lowercase().as_str() {
        "lo" | "loopback" => Ok(Ipv4Addr::new(127, 0, 0, 1)),
        _ => {
            if let Some(ip) = lookup_interface_ipv4(interface_name)? {
                Ok(ip)
            } else {
                Err(anyhow!(
                    "Network interface '{interface_name}' was not found; pass a valid interface name or IPv4 address"
                ))
            }
        }
    }
}

#[cfg(unix)]
fn lookup_interface_ipv4(interface_name: &str) -> Result<Option<Ipv4Addr>> {
    use std::ffi::CStr;
    use std::ptr;

    let mut interfaces: *mut libc::ifaddrs = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(std::io::Error::last_os_error()).context("Failed to enumerate interfaces");
    }

    let mut cursor = interfaces;
    let mut found = None;

    while !cursor.is_null() {
        let interface = unsafe { &*cursor };

        if !interface.ifa_name.is_null() && !interface.ifa_addr.is_null() {
            let name = unsafe { CStr::from_ptr(interface.ifa_name) }.to_string_lossy();
            let family = unsafe { (*interface.ifa_addr).sa_family as i32 };

            if name == interface_name && family == libc::AF_INET {
                let sockaddr = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_in) };
                found = Some(Ipv4Addr::from(sockaddr.sin_addr.s_addr.to_ne_bytes()));
                break;
            }
        }

        cursor = interface.ifa_next;
    }

    unsafe { libc::freeifaddrs(interfaces) };
    Ok(found)
}

#[cfg(not(unix))]
fn lookup_interface_ipv4(_interface_name: &str) -> Result<Option<Ipv4Addr>> {
    Ok(None)
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
    fn dscp_value_converts_to_ipv4_tos_byte() {
        assert_eq!(dscp_to_tos(0).unwrap(), 0);
        assert_eq!(dscp_to_tos(34).unwrap(), 136);
        assert_eq!(dscp_to_tos(46).unwrap(), 184);
        assert!(dscp_to_tos(64).is_err());
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
