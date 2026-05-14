use anyhow::{Context, Result};
use socket2::{Domain, Protocol, Socket, Type};
use std::ffi::CStr;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::messages::{
    AnnounceMessage, ClockIdentity, DelayReqMessage, DelayRespMessage, MessageType, PtpHeader,
    Timestamp,
};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum PtpState {
    #[default]
    Initializing,
    Listening,
    Uncalibrated,
    Slave,
    Master,
    Passive,
}

#[derive(Debug, Clone)]
pub struct PtpConfig {
    pub domain: u8,
    pub priority1: u8,
    pub priority2: u8,
    pub interface_ip: Ipv4Addr,
    pub clock_identity: Option<ClockIdentity>,
}

impl Default for PtpConfig {
    fn default() -> Self {
        Self {
            domain: 0,
            priority1: 128,
            priority2: 128,
            interface_ip: Ipv4Addr::new(0, 0, 0, 0),
            clock_identity: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct PtpStats {
    pub offset_ns: i64,
    pub mean_path_delay_ns: i64,
    pub sync_count: u64,
    pub announce_count: u64,
    pub state: PtpState,
    pub local_identity: ClockIdentity,
    pub master_identity: Option<ClockIdentity>,
}

/// A simple clock abstraction that can be adjusted
#[derive(Debug)]
struct SimpleClock {
    /// Offset from system time in nanoseconds
    offset_ns: i64,
    /// Base time for PTP epoch
    base_time: SystemTime,
}

impl SimpleClock {
    fn new() -> Self {
        Self {
            offset_ns: 0,
            base_time: UNIX_EPOCH,
        }
    }

    fn now_ns(&self) -> u64 {
        let sys_time = SystemTime::now()
            .duration_since(self.base_time)
            .unwrap_or_default();
        let sys_ns = sys_time.as_nanos() as u64;
        (sys_ns as i64 + self.offset_ns) as u64
    }

    fn set_offset(&mut self, offset_ns: i64) {
        self.offset_ns = offset_ns;
    }

    fn system_now_ns() -> Result<u64> {
        Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64)
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingDelayRequest {
    sequence_id: u16,
    sent_ns: u64,
    sync_origin_ns: u64,
    sync_arrival_ns: u64,
    requesting_port_identity: [u8; 10],
}

pub struct PtpClient {
    /// PTP configuration
    config: PtpConfig,
    /// Simple clock for timing
    clock: Arc<Mutex<SimpleClock>>,
    /// Current PTP statistics
    stats: Arc<Mutex<PtpStats>>,
    /// Stop signal
    shutdown: CancellationToken,
    /// Running task handle
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Running flag (for status check)
    is_running: Arc<Mutex<bool>>,
}

impl PtpClient {
    pub fn new(config: PtpConfig) -> Self {
        let local_identity = config
            .clock_identity
            .or_else(|| discover_clock_identity(config.interface_ip).ok())
            .unwrap_or_else(|| ClockIdentity::from_local_ipv4(config.interface_ip));

        let stats = PtpStats {
            local_identity,
            ..Default::default()
        };

        Self {
            config,
            clock: Arc::new(Mutex::new(SimpleClock::new())),
            stats: Arc::new(Mutex::new(stats)),
            shutdown: CancellationToken::new(),
            task: Arc::new(Mutex::new(None)),
            is_running: Arc::new(Mutex::new(false)),
        }
    }

    pub async fn start(&self) -> Result<()> {
        let config = self.config.clone();
        let clock = self.clock.clone();
        let stats = self.stats.clone();
        let shutdown = self.shutdown.child_token();
        let is_running = self.is_running.clone();

        *is_running.lock().unwrap() = true;

        let handle = tokio::spawn(async move {
            if let Err(e) = Self::run_ptp_loop(config, clock, stats, shutdown).await {
                log::error!("PTP loop error: {}", e);
            }
            *is_running.lock().unwrap() = false;
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
                Err(e) => log::warn!("PTP task failed to join: {e}"),
            }
        }
    }

    pub fn get_time(&self) -> u64 {
        self.clock.lock().unwrap().now_ns()
    }

    pub fn get_stats(&self) -> PtpStats {
        let guard = self.stats.lock().unwrap();
        PtpStats {
            offset_ns: guard.offset_ns,
            mean_path_delay_ns: guard.mean_path_delay_ns,
            sync_count: guard.sync_count,
            announce_count: guard.announce_count,
            state: guard.state,
            local_identity: guard.local_identity,
            master_identity: guard.master_identity,
        }
    }

    pub fn reference_clock_identity(&self) -> ClockIdentity {
        let stats = self.stats.lock().unwrap();
        stats.master_identity.unwrap_or(stats.local_identity)
    }

    pub fn apply_announce_bytes(&self, bytes: &[u8]) -> Result<()> {
        Self::apply_announce_bytes_to_stats(&self.config, &self.stats, bytes)
    }

    fn apply_announce_bytes_to_stats(
        config: &PtpConfig,
        stats: &Arc<Mutex<PtpStats>>,
        bytes: &[u8],
    ) -> Result<()> {
        let announce = AnnounceMessage::from_bytes(bytes)?;
        if announce.domain_number != config.domain {
            return Ok(());
        }

        let mut stats = stats.lock().unwrap();
        stats.announce_count += 1;
        stats.master_identity = Some(announce.grandmaster_identity);
        Ok(())
    }

    fn apply_delay_resp_bytes(
        clock: &Arc<Mutex<SimpleClock>>,
        stats: &Arc<Mutex<PtpStats>>,
        pending_delay_request: &mut Option<PendingDelayRequest>,
        bytes: &[u8],
        domain: u8,
    ) -> Result<()> {
        let delay_resp = DelayRespMessage::from_bytes(bytes)?;
        if delay_resp.domain_number != domain {
            return Ok(());
        }

        let Some(pending) = *pending_delay_request else {
            return Ok(());
        };

        if delay_resp.sequence_id != pending.sequence_id
            || delay_resp.requesting_port_identity != pending.requesting_port_identity
        {
            return Ok(());
        }

        let master_to_slave = pending.sync_arrival_ns as i128 - pending.sync_origin_ns as i128;
        let slave_to_master =
            delay_resp.receive_timestamp.as_nanos() as i128 - pending.sent_ns as i128;
        let mean_path_delay = (master_to_slave + slave_to_master) / 2;
        let offset = (slave_to_master - master_to_slave) / 2;

        {
            let mut clock = clock.lock().unwrap();
            clock.set_offset(offset as i64);
        }

        let mut stats = stats.lock().unwrap();
        stats.mean_path_delay_ns = mean_path_delay as i64;
        stats.offset_ns = offset as i64;
        stats.state = PtpState::Slave;
        *pending_delay_request = None;
        Ok(())
    }

    pub fn rtp_timestamp(&self, sample_rate: u32) -> Result<u32> {
        let now_ns = self.get_time();
        // Calculate RTP timestamp based on PTP time
        // Use u128 to avoid overflow during multiplication
        let timestamp = (now_ns as u128 * sample_rate as u128 / 1_000_000_000) as u32;
        Ok(timestamp)
    }

    pub fn is_running(&self) -> bool {
        *self.is_running.lock().unwrap()
    }

    async fn run_ptp_loop(
        config: PtpConfig,
        clock: Arc<Mutex<SimpleClock>>,
        stats: Arc<Mutex<PtpStats>>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        log::info!("Starting PTP client on domain {}", config.domain);

        {
            let mut stats = stats.lock().unwrap();
            stats.state = PtpState::Listening;
        }

        // Setup sockets
        // PTP Event port: 319
        // PTP General port: 320
        let event_socket = Self::setup_multicast_socket(319, config.interface_ip)?;
        let general_socket = Self::setup_multicast_socket(320, config.interface_ip)?;

        let mut event_buf = [0u8; 2048];
        let mut general_buf = [0u8; 2048];

        // Simple state tracking
        let local_port_identity = {
            let stats = stats.lock().unwrap();
            source_port_identity(stats.local_identity)
        };
        let ptp_event_addr = SocketAddrV4::new(Ipv4Addr::new(224, 0, 1, 129), 319);

        let mut last_sync_ts: Option<u64> = None;
        let mut last_sync_seq_id: Option<u16> = None;
        let mut delay_req_sequence_id: u16 = 0;
        let mut pending_delay_request: Option<PendingDelayRequest> = None;

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    log::info!("PTP loop stopping");
                    break;
                }
                Ok((len, _)) = event_socket.recv_from(&mut event_buf) => {
                    if let Ok(header) = PtpHeader::from_bytes(&event_buf[..len]) {
                        if header.domain_number != config.domain {
                            continue;
                        }

                        if header.message_type == MessageType::Sync {
                            // Record arrival time
                            let arrival_ts = SimpleClock::system_now_ns()?;
                            last_sync_ts = Some(arrival_ts);
                            last_sync_seq_id = Some(header.sequence_id);

                            let mut stats = stats.lock().unwrap();
                            stats.sync_count += 1;
                            if stats.state == PtpState::Listening {
                                stats.state = PtpState::Uncalibrated;
                            }
                        }
                    }
                }
                Ok((len, _)) = general_socket.recv_from(&mut general_buf) => {
                    if let Ok(header) = PtpHeader::from_bytes(&general_buf[..len]) {
                        if header.domain_number != config.domain {
                            continue;
                        }

                        match header.message_type {
                            MessageType::FollowUp => {
                                let (Some(sync_seq), Some(arrival_ns)) =
                                    (last_sync_seq_id, last_sync_ts)
                                else {
                                    continue;
                                };

                                if header.sequence_id != sync_seq {
                                    continue;
                                }

                                let Ok(origin_ts) = Timestamp::from_bytes(&general_buf[34..44])
                                else {
                                    continue;
                                };

                                let origin_ns = origin_ts.as_nanos() as u64;
                                let offset = origin_ns as i64 - arrival_ns as i64;
                                clock.lock().unwrap().set_offset(offset);

                                {
                                    let mut stats = stats.lock().unwrap();
                                    stats.offset_ns = offset;
                                    stats.state = PtpState::Uncalibrated;
                                    stats.master_identity = Some(ClockIdentity::from_bytes([
                                        header.source_port_identity[0],
                                        header.source_port_identity[1],
                                        header.source_port_identity[2],
                                        header.source_port_identity[3],
                                        header.source_port_identity[4],
                                        header.source_port_identity[5],
                                        header.source_port_identity[6],
                                        header.source_port_identity[7],
                                    ]));
                                }

                                let delay_req_sent_ns = SimpleClock::system_now_ns()?;
                                let delay_req = DelayReqMessage {
                                    domain_number: config.domain,
                                    source_port_identity: local_port_identity,
                                    sequence_id: delay_req_sequence_id,
                                    origin_timestamp: Timestamp::from_nanos(delay_req_sent_ns),
                                };
                                pending_delay_request = Some(PendingDelayRequest {
                                    sequence_id: delay_req_sequence_id,
                                    sent_ns: delay_req_sent_ns,
                                    sync_origin_ns: origin_ns,
                                    sync_arrival_ns: arrival_ns,
                                    requesting_port_identity: local_port_identity,
                                });
                                delay_req_sequence_id = delay_req_sequence_id.wrapping_add(1);

                                let delay_req_bytes = delay_req.to_bytes();
                                if let Err(error) = event_socket
                                    .send_to(&delay_req_bytes, ptp_event_addr)
                                    .await
                                {
                                    log::warn!("Failed to send PTP DelayReq: {error}");
                                }

                                log::debug!("Synced with master, one-way offset: {} ns", offset);
                            }
                            MessageType::DelayResp => {
                                let _ = Self::apply_delay_resp_bytes(
                                    &clock,
                                    &stats,
                                    &mut pending_delay_request,
                                    &general_buf[..len],
                                    config.domain,
                                );
                            }
                            MessageType::Announce => {
                                let _ = Self::apply_announce_bytes_to_stats(
                                    &config,
                                    &stats,
                                    &general_buf[..len],
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn setup_multicast_socket(port: u16, interface_ip: Ipv4Addr) -> Result<UdpSocket> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        // Allow reuse address and port
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;

        // Bind to wildcard address
        let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), port);
        socket.bind(&addr.into())?;

        // Join multicast group 224.0.1.129 (PTP primary)
        let multi_addr = Ipv4Addr::new(224, 0, 1, 129);
        socket.join_multicast_v4(&multi_addr, &interface_ip)?;

        socket.set_nonblocking(true)?;

        Ok(UdpSocket::from_std(socket.into())?)
    }
}

fn source_port_identity(clock_identity: ClockIdentity) -> [u8; 10] {
    let mut identity = [0u8; 10];
    identity[0..8].copy_from_slice(&clock_identity.as_bytes());
    identity[8..10].copy_from_slice(&1u16.to_be_bytes());
    identity
}

fn discover_clock_identity(interface_ip: Ipv4Addr) -> Result<ClockIdentity> {
    let interface_name = interface_name_for_ipv4(interface_ip)?
        .with_context(|| format!("no interface found for {interface_ip}"))?;
    let mac = mac_address_for_interface(&interface_name)?
        .with_context(|| format!("no MAC address found for interface {interface_name}"))?;
    Ok(ClockIdentity::from_mac_address(mac))
}

#[cfg(unix)]
fn interface_name_for_ipv4(interface_ip: Ipv4Addr) -> Result<Option<String>> {
    let mut interfaces: *mut libc::ifaddrs = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(std::io::Error::last_os_error()).context("failed to enumerate interfaces");
    }

    let mut cursor = interfaces;
    let mut found = None;

    while !cursor.is_null() {
        let interface = unsafe { &*cursor };

        if !interface.ifa_name.is_null() && !interface.ifa_addr.is_null() {
            let name = unsafe { CStr::from_ptr(interface.ifa_name) }.to_string_lossy();
            let family = unsafe { (*interface.ifa_addr).sa_family as i32 };

            if family == libc::AF_INET {
                let sockaddr = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_in) };
                let ip = Ipv4Addr::from(sockaddr.sin_addr.s_addr.to_ne_bytes());
                if ip == interface_ip {
                    found = Some(name.into_owned());
                    break;
                }
            }
        }

        cursor = unsafe { (*cursor).ifa_next };
    }

    unsafe { libc::freeifaddrs(interfaces) };
    Ok(found)
}

#[cfg(not(unix))]
fn interface_name_for_ipv4(_interface_ip: Ipv4Addr) -> Result<Option<String>> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn mac_address_for_interface(interface_name: &str) -> Result<Option<[u8; 6]>> {
    let mut interfaces: *mut libc::ifaddrs = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(std::io::Error::last_os_error()).context("failed to enumerate interfaces");
    }

    let mut cursor = interfaces;
    let mut found = None;

    while !cursor.is_null() {
        let interface = unsafe { &*cursor };

        if !interface.ifa_name.is_null() && !interface.ifa_addr.is_null() {
            let name = unsafe { CStr::from_ptr(interface.ifa_name) }.to_string_lossy();
            let family = unsafe { (*interface.ifa_addr).sa_family as i32 };

            if name == interface_name && family == libc::AF_PACKET {
                let sockaddr = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_ll) };
                if sockaddr.sll_halen >= 6 {
                    let mut mac = [0u8; 6];
                    mac.copy_from_slice(&sockaddr.sll_addr[..6]);
                    found = Some(mac);
                    break;
                }
            }
        }

        cursor = unsafe { (*cursor).ifa_next };
    }

    unsafe { libc::freeifaddrs(interfaces) };
    Ok(found)
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
fn mac_address_for_interface(interface_name: &str) -> Result<Option<[u8; 6]>> {
    let mut interfaces: *mut libc::ifaddrs = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(std::io::Error::last_os_error()).context("failed to enumerate interfaces");
    }

    let mut cursor = interfaces;
    let mut found = None;

    while !cursor.is_null() {
        let interface = unsafe { &*cursor };

        if !interface.ifa_name.is_null() && !interface.ifa_addr.is_null() {
            let name = unsafe { CStr::from_ptr(interface.ifa_name) }.to_string_lossy();
            let family = unsafe { (*interface.ifa_addr).sa_family as i32 };

            if name == interface_name && family == libc::AF_LINK {
                let sockaddr = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_dl) };
                if sockaddr.sdl_alen >= 6 {
                    let offset = sockaddr.sdl_nlen as usize;
                    let mut mac = [0u8; 6];
                    for (index, byte) in mac.iter_mut().enumerate() {
                        *byte = sockaddr.sdl_data[offset + index] as u8;
                    }
                    found = Some(mac);
                    break;
                }
            }
        }

        cursor = unsafe { (*cursor).ifa_next };
    }

    unsafe { libc::freeifaddrs(interfaces) };
    Ok(found)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
fn mac_address_for_interface(_interface_name: &str) -> Result<Option<[u8; 6]>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ptp_config_default() {
        let config = PtpConfig::default();
        assert_eq!(config.domain, 0);
        assert_eq!(config.priority1, 128);
        assert_eq!(config.clock_identity, None);
    }

    #[tokio::test]
    async fn test_ptp_client_creation() {
        let config = PtpConfig::default();
        let client = PtpClient::new(config);

        assert!(!client.is_running());

        // We can't easily test start() without network permissions/setup in CI environment
        // but we can verify structure initialization
        let stats = client.get_stats();
        assert_eq!(stats.state, PtpState::Initializing);
        assert_ne!(stats.local_identity, ClockIdentity::default());
        assert_eq!(client.reference_clock_identity(), stats.local_identity);
    }

    #[test]
    fn announce_updates_reference_identity_and_ignores_wrong_domain() {
        let client = PtpClient::new(PtpConfig {
            domain: 7,
            clock_identity: Some(ClockIdentity::from_bytes([
                0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01,
            ])),
            ..Default::default()
        });
        let first_master =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x12, 0x34, 0x56]);
        let wrong_domain_master =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0xaa, 0xbb, 0xcc]);
        let second_master =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x65, 0x43, 0x21]);

        client
            .apply_announce_bytes(&announce_packet(7, first_master))
            .expect("valid announce should be accepted");
        assert_eq!(client.reference_clock_identity(), first_master);

        client
            .apply_announce_bytes(&announce_packet(8, wrong_domain_master))
            .expect("wrong-domain announce should be ignored");
        assert_eq!(client.reference_clock_identity(), first_master);

        client
            .apply_announce_bytes(&announce_packet(7, second_master))
            .expect("later grandmaster should be accepted");
        assert_eq!(client.reference_clock_identity(), second_master);
    }

    fn announce_packet(domain: u8, grandmaster_identity: ClockIdentity) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[0] = 0x0b;
        bytes[1] = 0x02;
        bytes[4] = domain;
        bytes[53..61].copy_from_slice(&grandmaster_identity.as_bytes());
        bytes
    }

    #[test]
    fn matched_delay_response_updates_path_delay_and_clock_offset() {
        let clock = Arc::new(Mutex::new(SimpleClock::new()));
        let stats = Arc::new(Mutex::new(PtpStats {
            local_identity: ClockIdentity::from_bytes([
                0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01,
            ]),
            ..Default::default()
        }));
        let requesting_port_identity = [0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01, 0x00, 0x01];
        let mut pending_delay_request = Some(PendingDelayRequest {
            sequence_id: 42,
            sent_ns: 20_000,
            sync_origin_ns: 10_000,
            sync_arrival_ns: 11_200,
            requesting_port_identity,
        });

        PtpClient::apply_delay_resp_bytes(
            &clock,
            &stats,
            &mut pending_delay_request,
            &delay_resp_packet(7, 42, 20_800, requesting_port_identity),
            7,
        )
        .expect("matching delay response should apply");

        let stats = stats.lock().unwrap();
        assert_eq!(stats.mean_path_delay_ns, 1_000);
        assert_eq!(stats.offset_ns, -200);
        assert_eq!(stats.state, PtpState::Slave);
        assert!(pending_delay_request.is_none());
        assert_eq!(clock.lock().unwrap().offset_ns, -200);
    }

    fn delay_resp_packet(
        domain: u8,
        sequence_id: u16,
        receive_timestamp_ns: u64,
        requesting_port_identity: [u8; 10],
    ) -> Vec<u8> {
        let mut bytes = vec![0u8; 54];
        bytes[0] = 0x09;
        bytes[1] = 0x02;
        bytes[4] = domain;
        bytes[30..32].copy_from_slice(&sequence_id.to_be_bytes());
        bytes[34..44].copy_from_slice(&Timestamp::from_nanos(receive_timestamp_ns).to_bytes());
        bytes[44..54].copy_from_slice(&requesting_port_identity);
        bytes
    }
}
