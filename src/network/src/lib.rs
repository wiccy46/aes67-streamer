pub mod rtp;
pub mod socket;

pub use rtp::{RtpPacket, RtpHeader, RtpPacketizer};
pub use socket::{MulticastSocket, MulticastConfig, SocketStats, resolve_interface_ip};

pub type Result<T> = anyhow::Result<T>;