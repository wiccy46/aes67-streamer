use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

const SAP_DSCP: u8 = 24;

pub struct SapAnnouncer {
    socket: Arc<UdpSocket>,
    sdp_payload: Arc<Mutex<String>>,
    origin_source: Ipv4Addr,
    shutdown: CancellationToken,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl SapAnnouncer {
    pub fn new(sdp_payload: String, interface_ip: Ipv4Addr) -> Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        crate::socket::apply_udp_socket_defaults(&socket, crate::socket::sap_socket_defaults())?;

        // Bind to wildcard
        let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0);
        socket.bind(&addr.into())?;

        // SAP DSCP 24 discovery/control
        socket.set_tos_v4(crate::socket::dscp_to_tos(SAP_DSCP)?)?;

        // Set multicast interface
        socket.set_multicast_if_v4(&interface_ip)?;

        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(socket.into())?;

        Ok(Self {
            socket: Arc::new(socket),
            sdp_payload: Arc::new(Mutex::new(sdp_payload)),
            origin_source: interface_ip,
            shutdown: CancellationToken::new(),
            task: Arc::new(Mutex::new(None)),
        })
    }

    pub fn update_sdp_payload(&self, sdp_payload: String) {
        *self.sdp_payload.lock().unwrap() = sdp_payload;
    }

    pub fn get_sdp_payload(&self) -> String {
        self.sdp_payload.lock().unwrap().clone()
    }

    pub async fn start(&self) -> Result<()> {
        let sap_addr = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 255), 9875);
        let mut interval = time::interval(Duration::from_secs(30));

        let socket = self.socket.clone();
        let shutdown = self.shutdown.child_token();
        let sdp_payload = self.sdp_payload.clone();
        let origin_source = self.origin_source;

        log::info!("Starting SAP announcer to {}", sap_addr);

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        log::info!("SAP announcer stopping");
                        break;
                    }
                    _ = interval.tick() => {
                        let packet = build_sap_packet(&sdp_payload.lock().unwrap(), origin_source);
                        if let Err(e) = socket.send_to(&packet, sap_addr).await {
                            log::warn!("Failed to send SAP announcement: {}", e);
                        } else {
                            log::debug!("Sent SAP announcement");
                        }
                    }
                }
            }
        });
        *self.task.lock().unwrap() = Some(handle);

        Ok(())
    }

    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    pub async fn shutdown(&self) {
        self.stop();
        let handle = self.task.lock().unwrap().take();
        if let Some(handle) = handle {
            match handle.await {
                Ok(()) => {}
                Err(e) => log::warn!("SAP announcer task failed to join: {e}"),
            }
        }
    }
}

fn build_sap_packet(sdp_payload: &str, origin_source: Ipv4Addr) -> Vec<u8> {
    let message_hash = sap_message_hash(sdp_payload, origin_source);
    let mut packet = Vec::with_capacity(24 + sdp_payload.len());
    packet.push(0x20);
    packet.push(0x00);
    packet.extend_from_slice(&message_hash.to_be_bytes());
    packet.extend_from_slice(&origin_source.octets());
    packet.extend_from_slice(b"application/sdp\0");
    packet.extend_from_slice(sdp_payload.as_bytes());
    packet
}

fn sap_message_hash(sdp_payload: &str, origin_source: Ipv4Addr) -> u16 {
    let mut hash = 0x811c9dc5u32;

    for byte in origin_source
        .octets()
        .into_iter()
        .chain(sdp_payload.bytes())
    {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }

    let folded = ((hash >> 16) as u16) ^ (hash as u16);
    if folded == 0 { 1 } else { folded }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sap_announcer_updates_sdp_payload() {
        let announcer =
            SapAnnouncer::new("v=0\r\ns=old\r\n".to_string(), Ipv4Addr::new(127, 0, 0, 1))
                .expect("SAP announcer should be created");

        announcer.update_sdp_payload("v=0\r\ns=new\r\n".to_string());

        assert_eq!(announcer.get_sdp_payload(), "v=0\r\ns=new\r\n");
    }

    #[test]
    fn sap_packet_uses_origin_source_and_application_sdp_payload_type() {
        let sdp = "v=0\r\ns=AES67 Stream\r\n";
        let origin_source = Ipv4Addr::new(192, 168, 1, 50);

        let packet = build_sap_packet(sdp, origin_source);

        assert_eq!(packet[0], 0x20);
        assert_eq!(packet[1], 0x00);
        assert_eq!(&packet[4..8], &[192, 168, 1, 50]);
        assert_eq!(&packet[8..24], b"application/sdp\0");
        assert_eq!(&packet[24..], sdp.as_bytes());
    }

    #[test]
    fn sap_message_hash_is_stable_and_changes_with_sdp_payload_or_origin_source() {
        let origin_source = Ipv4Addr::new(192, 168, 1, 50);
        let first = build_sap_packet("v=0\r\ns=first\r\n", origin_source);
        let first_again = build_sap_packet("v=0\r\ns=first\r\n", origin_source);
        let second = build_sap_packet("v=0\r\ns=second\r\n", origin_source);
        let different_origin =
            build_sap_packet("v=0\r\ns=first\r\n", Ipv4Addr::new(192, 168, 1, 51));

        assert_eq!(&first[2..4], &first_again[2..4]);
        assert_ne!(&first[2..4], &second[2..4]);
        assert_ne!(&first[2..4], &different_origin[2..4]);
        assert_ne!(&first[2..4], &[0x12, 0x34]);
    }
}
