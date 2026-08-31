//! Shared UDP socket construction for AES67 protocols.

use anyhow::{Context, Result, anyhow};
use socket2::{Domain, Protocol, Socket, Type};

/// Portable socket options shared by RTP, SAP, and PTP transports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdpSocketOptions {
    /// Enable `SO_REUSEADDR` before binding.
    pub reuse_address: bool,
    /// Enable `SO_REUSEPORT` where the operating system supports it.
    pub reuse_port: bool,
    /// Allow outgoing IPv4 multicast packets to be received locally.
    pub multicast_loop_v4: bool,
    /// Requested kernel send-buffer size in bytes.
    pub send_buffer_size: Option<usize>,
    /// Requested kernel receive-buffer size in bytes.
    pub recv_buffer_size: Option<usize>,
    /// Six-bit Differentiated Services Code Point for outgoing packets.
    pub dscp: Option<u8>,
}

/// Create an IPv4 UDP socket and apply the requested portable options.
pub fn create_udp_socket(options: UdpSocketOptions) -> Result<Socket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    if options.reuse_address {
        socket
            .set_reuse_address(true)
            .context("Failed to enable SO_REUSEADDR")?;
    }
    #[cfg(unix)]
    if options.reuse_port {
        socket
            .set_reuse_port(true)
            .context("Failed to enable SO_REUSEPORT")?;
    }
    if options.multicast_loop_v4 {
        socket
            .set_multicast_loop_v4(true)
            .context("Failed to enable IPv4 multicast loopback")?;
    }
    if let Some(size) = options.send_buffer_size {
        socket
            .set_send_buffer_size(size)
            .with_context(|| format!("Failed to set UDP send buffer size to {size}"))?;
    }
    if let Some(size) = options.recv_buffer_size {
        socket
            .set_recv_buffer_size(size)
            .with_context(|| format!("Failed to set UDP receive buffer size to {size}"))?;
    }
    if let Some(dscp) = options.dscp {
        socket
            .set_tos_v4(dscp_to_tos(dscp)?)
            .with_context(|| format!("Failed to set UDP DSCP value to {dscp}"))?;
    }

    Ok(socket)
}

/// DSCP Layout (Modern): Defined in RFC 2474, the field was renamed the Differentiated Services (DS) field. 
/// The first 6 bits form the DSCP value (allowing for 64 distinct traffic classes), 
/// while the final 2 bits are allocated for Explicit Congestion Notification (ECN)
/// tos stands for Type of Service
/// For AES67, ptpv2 uses dscp 46, rtp uses dscp 34
pub(crate) fn dscp_to_tos(dscp: u8) -> Result<u32> {
    if dscp > 63 {
        return Err(anyhow!("DSCP value {dscp} must be between 0 and 63"));
    }

    Ok((dscp as u32) << 2)
}
