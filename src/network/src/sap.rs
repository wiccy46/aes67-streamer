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
    shutdown: CancellationToken,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl SapAnnouncer {
    pub fn new(sdp_payload: String, interface_ip: Ipv4Addr) -> Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;

        // Bind to wildcard
        let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0);
        socket.bind(&addr.into())?;

        // Set multicast loop so we can receive our own announcements (useful for testing)
        socket.set_multicast_loop_v4(true)?;
        socket.set_tos_v4(crate::socket::dscp_to_tos(SAP_DSCP)?)?;

        // Set multicast interface
        socket.set_multicast_if_v4(&interface_ip)?;

        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(socket.into())?;

        Ok(Self {
            socket: Arc::new(socket),
            sdp_payload: Arc::new(Mutex::new(sdp_payload)),
            shutdown: CancellationToken::new(),
            task: Arc::new(Mutex::new(None)),
        })
    }

    pub fn update_sdp_payload(&self, sdp_payload: String) {
        *self.sdp_payload.lock().unwrap() = sdp_payload;
    }

    pub fn sdp_payload(&self) -> String {
        self.sdp_payload.lock().unwrap().clone()
    }

    pub async fn start(&self) -> Result<()> {
        let sap_addr = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 255), 9875);
        let mut interval = time::interval(Duration::from_secs(30));

        let socket = self.socket.clone();
        let shutdown = self.shutdown.child_token();
        let sdp_payload = self.sdp_payload.clone();

        // SAP Header:
        // V=1, A=0, R=0, T=0, E=0, C=0
        // Auth len = 0
        // Msg Id Hash = 0 (should be random/unique but 0 is fine for simple)
        // Originating Source = Interface IP (we'll just use 0.0.0.0 or handle it in packet construction)

        log::info!("Starting SAP announcer to {}", sap_addr);

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        log::info!("SAP announcer stopping");
                        break;
                    }
                    _ = interval.tick() => {
                        let packet = build_sap_packet(&sdp_payload.lock().unwrap());
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

fn build_sap_packet(sdp_payload: &str) -> Vec<u8> {
    // Construct SAP packet
    // Header (1 byte): 00100000 (V=1, others 0) -> 0x20
    // Auth Len (1 byte): 0x00
    // Msg Id Hash (2 bytes): 0x1234 (random)
    // Originating Source (4 bytes): 0.0.0.0 (or actual IP)
    // Payload Type (MIME): "application/sdp" -> but SAP usually just puts SDP text after header
    let mut packet = vec![
        0x20, // Header
        0x00, // Auth Len
        0x12, // Msg Id Hash
        0x34,
    ];
    packet.extend_from_slice(&[0, 0, 0, 0]); // Originating Source (should be IP)
    packet.extend_from_slice(sdp_payload.as_bytes());
    packet
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

        assert_eq!(announcer.sdp_payload(), "v=0\r\ns=new\r\n");
    }
}
