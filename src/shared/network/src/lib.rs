pub mod jitter;
pub mod rtp;
pub mod sap;
pub mod sdp;
pub mod socket;
pub mod udp;

mod interfaces;

pub use interfaces::{
    NetworkInterface, find_interface_mac_by_ipv4, list_ipv4_interfaces, resolve_interface_ip,
};
pub use jitter::{
    InsertResult, JitterBufferConfig, JitterBufferStats, PlayoutPacket, RtpJitterBuffer,
};
pub use rtp::{RtpHeader, RtpPacket, RtpPacketizer, decode_l24_payload_interleaved};
pub use sap::{
    ReceivedSapMessage, SAP_MULTICAST_ADDRESS, SAP_PORT, SapAnnouncer, SapBrowser,
    SapBrowserConfig, SapMessage, SapMessageKey, SapMessageType, SapRegistry, SapRegistryEvent,
    SapStream, parse_sap_packet,
};
pub use sdp::{Aes67SessionDescription, AudioEncoding, parse_sdp, parse_sdp_file};
pub use socket::{
    MulticastConfig, MulticastSocket, ReceivedRtpPacket, RtpReceiveSocket, RtpReceiveSocketConfig,
    SocketStats, parse_stream_address,
};

pub type Result<T> = anyhow::Result<T>;
