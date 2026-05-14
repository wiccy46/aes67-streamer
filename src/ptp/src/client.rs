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

use crate::messages::{AnnounceMessage, ClockIdentity, MessageType, PtpHeader, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PtpState {
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

impl Default for PtpState {
    fn default() -> Self {
        PtpState::Initializing
    }
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

    fn adjust_offset(&mut self, delta_ns: i64) {
        self.offset_ns += delta_ns;
    }
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
            if let Err(e) = handle.await {
                log::warn!("PTP task failed to join: {e}");
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

    // Legacy method for compatibility with existing code
    pub fn tick(&mut self) {
        // No-op in async implementation
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
        let mut last_sync_ts: Option<u64> = None;
        let mut last_sync_seq_id: Option<u16> = None;

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

                        match header.message_type {
                            MessageType::Sync => {
                                // Record arrival time
                                let arrival_ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64;
                                last_sync_ts = Some(arrival_ts);
                                last_sync_seq_id = Some(header.sequence_id);

                                let mut stats = stats.lock().unwrap();
                                stats.sync_count += 1;
                                if stats.state == PtpState::Listening {
                                    stats.state = PtpState::Uncalibrated;
                                }
                            }
                            _ => {}
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
                                if let Some(sync_seq) = last_sync_seq_id {
                                    if header.sequence_id == sync_seq {
                                        // Found matching FollowUp
                                        if let Ok(origin_ts) = Timestamp::from_bytes(&general_buf[34..44]) {
                                            let origin_ns = origin_ts.as_nanos() as u64;
                                            if let Some(arrival_ns) = last_sync_ts {
                                                // Calculate offset
                                                // offset = master_time - slave_time
                                                // We want slave_time + offset = master_time
                                                // So offset = origin_ns - arrival_ns
                                                let offset = origin_ns as i64 - arrival_ns as i64;

                                                let mut clock_guard = clock.lock().unwrap();
                                                clock_guard.adjust_offset(offset);

                                                let mut stats = stats.lock().unwrap();
                                                stats.offset_ns = offset;
                                                stats.state = PtpState::Slave;
                                                stats.master_identity =
                                                    Some(ClockIdentity::from_bytes([
                                                        header.source_port_identity[0],
                                                        header.source_port_identity[1],
                                                        header.source_port_identity[2],
                                                        header.source_port_identity[3],
                                                        header.source_port_identity[4],
                                                        header.source_port_identity[5],
                                                        header.source_port_identity[6],
                                                        header.source_port_identity[7],
                                                    ]));

                                                log::debug!("Synced with master, offset: {} ns", offset);
                                            }
                                        }
                                    }
                                }
                            }
                            MessageType::Announce => {
                                let mut stats = stats.lock().unwrap();
                                stats.announce_count += 1;
                                if let Ok(announce) = AnnounceMessage::from_bytes(&general_buf[..len]) {
                                    stats.master_identity = Some(announce.grandmaster_identity);
                                }
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
}
