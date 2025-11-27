use anyhow::Result;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::time::{self, Duration};
use socket2::{Socket, Domain, Type, Protocol};

pub struct SapAnnouncer {
    socket: Arc<UdpSocket>,
    sdp_payload: String,
    stop_notify: Arc<Notify>,
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
        
        // Set multicast interface
        socket.set_multicast_if_v4(&interface_ip)?;
        
        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(socket.into())?;
        
        Ok(Self {
            socket: Arc::new(socket),
            sdp_payload,
            stop_notify: Arc::new(Notify::new()),
        })
    }
    
    pub async fn start(&self) -> Result<()> {
        let sap_addr = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 255), 9875);
        let mut interval = time::interval(Duration::from_secs(30));
        
        let socket = self.socket.clone();
        let stop_notify = self.stop_notify.clone();
        let sdp_payload = self.sdp_payload.clone();
        
        // SAP Header:
        // V=1, A=0, R=0, T=0, E=0, C=0
        // Auth len = 0
        // Msg Id Hash = 0 (should be random/unique but 0 is fine for simple)
        // Originating Source = Interface IP (we'll just use 0.0.0.0 or handle it in packet construction)
        
        // Construct SAP packet
        // Header (1 byte): 00100000 (V=1, others 0) -> 0x20
        // Auth Len (1 byte): 0x00
        // Msg Id Hash (2 bytes): 0x1234 (random)
        // Originating Source (4 bytes): 0.0.0.0 (or actual IP)
        // Payload Type (MIME): "application/sdp" -> but SAP usually just puts SDP text after header
        
        let mut packet = Vec::new();
        packet.push(0x20); // Header
        packet.push(0x00); // Auth Len
        packet.push(0x12); // Msg Id Hash
        packet.push(0x34);
        packet.extend_from_slice(&[0, 0, 0, 0]); // Originating Source (should be IP)
        
        // Payload
        packet.extend_from_slice(sdp_payload.as_bytes());
        
        log::info!("Starting SAP announcer to {}", sap_addr);
        
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_notify.notified() => {
                        log::info!("SAP announcer stopping");
                        break;
                    }
                    _ = interval.tick() => {
                        if let Err(e) = socket.send_to(&packet, sap_addr).await {
                            log::warn!("Failed to send SAP announcement: {}", e);
                        } else {
                            log::debug!("Sent SAP announcement");
                        }
                    }
                }
            }
        });
        
        Ok(())
    }
    
    pub fn stop(&self) {
        self.stop_notify.notify_one();
    }
}
