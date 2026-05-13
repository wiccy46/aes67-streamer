pub mod rtp;
pub mod sap;
pub mod socket;

pub use rtp::{RtpHeader, RtpPacket, RtpPacketizer};
pub use sap::SapAnnouncer;
pub use socket::{
    parse_stream_address, resolve_interface_ip, MulticastConfig, MulticastSocket, SocketStats,
};

pub type Result<T> = anyhow::Result<T>;
