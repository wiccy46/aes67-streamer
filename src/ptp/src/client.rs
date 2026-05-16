use anyhow::{Context, Result, anyhow};
use socket2::{Domain, Protocol, Socket, Type};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::messages::{
    AnnounceMessage, ClockIdentity, ClockQuality, DelayReqMessage, DelayRespMessage,
    FollowUpMessage, LocalAnnounceMessage, MessageType, PtpHeader, SyncMessage, Timestamp,
};

const ANNOUNCE_RECEIPT_TIMEOUT_MULTIPLIER: f64 = 3.0;
const MASTER_SELECTION_REFRESH: Duration = Duration::from_secs(1);
const LOCAL_MASTER_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(2);
const LOCAL_MASTER_SYNC_INTERVAL: Duration = Duration::from_secs(1);
const PTP_DSCP: u8 = 46;
const PTP_SOCKET_BUFFER_SIZE: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UdpSocketDefaults {
    reuse_address: bool,
    reuse_port: bool,
    multicast_loop_v4: bool,
    send_buffer_size: Option<usize>,
    recv_buffer_size: Option<usize>,
}

fn ptp_socket_defaults() -> UdpSocketDefaults {
    UdpSocketDefaults {
        reuse_address: true,
        reuse_port: true,
        multicast_loop_v4: true,
        send_buffer_size: Some(PTP_SOCKET_BUFFER_SIZE),
        recv_buffer_size: Some(PTP_SOCKET_BUFFER_SIZE),
    }
}

fn apply_udp_socket_defaults(socket: &Socket, defaults: UdpSocketDefaults) -> Result<()> {
    if defaults.reuse_address {
        socket.set_reuse_address(true)?;
    }
    #[cfg(unix)]
    if defaults.reuse_port {
        socket.set_reuse_port(true)?;
    }
    if defaults.multicast_loop_v4 {
        socket.set_multicast_loop_v4(true)?;
    }
    if let Some(size) = defaults.send_buffer_size {
        socket.set_send_buffer_size(size)?;
    }
    if let Some(size) = defaults.recv_buffer_size {
        socket.set_recv_buffer_size(size)?;
    }

    Ok(())
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MasterCandidate {
    priority1: u8,
    clock_quality: ClockQuality,
    priority2: u8,
    grandmaster_identity: ClockIdentity,
    last_seen: Instant,
    timeout: Duration,
}

impl MasterCandidate {
    fn from_announce(announce: &AnnounceMessage, now: Instant) -> Self {
        Self {
            priority1: announce.priority1,
            clock_quality: announce.clock_quality,
            priority2: announce.priority2,
            grandmaster_identity: announce.grandmaster_identity,
            last_seen: now,
            timeout: announce_timeout(announce.log_message_interval),
        }
    }

    fn is_better_than(&self, other: &Self) -> bool {
        self.dataset_key() < other.dataset_key()
    }

    fn is_expired(&self, now: Instant) -> bool {
        now.checked_duration_since(self.last_seen)
            .is_some_and(|elapsed| elapsed > self.timeout)
    }

    fn dataset_key(&self) -> (u8, u8, u8, u16, u8, ClockIdentity) {
        (
            self.priority1,
            self.clock_quality.clock_class,
            self.clock_quality.clock_accuracy,
            self.clock_quality.offset_scaled_log_variance,
            self.priority2,
            self.grandmaster_identity,
        )
    }
}

#[derive(Debug, Default)]
struct MasterSelection {
    candidates: BTreeMap<ClockIdentity, MasterCandidate>,
}

impl MasterSelection {
    fn observe(&mut self, announce: AnnounceMessage, now: Instant) -> Option<ClockIdentity> {
        self.remove_expired(now);
        let candidate = MasterCandidate::from_announce(&announce, now);
        self.candidates
            .insert(candidate.grandmaster_identity, candidate);
        self.best_identity()
    }

    fn refresh(&mut self, now: Instant) -> Option<ClockIdentity> {
        self.remove_expired(now);
        self.best_identity()
    }

    fn remove_expired(&mut self, now: Instant) {
        self.candidates
            .retain(|_, candidate| !candidate.is_expired(now));
    }

    fn best_identity(&self) -> Option<ClockIdentity> {
        self.candidates
            .values()
            .min_by(|left, right| {
                if left.is_better_than(right) {
                    Ordering::Less
                } else if right.is_better_than(left) {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .map(|candidate| candidate.grandmaster_identity)
    }
}

pub struct PtpClient {
    /// PTP configuration
    config: PtpConfig,
    /// Simple clock for timing
    clock: Arc<Mutex<SimpleClock>>,
    /// Current PTP statistics
    stats: Arc<Mutex<PtpStats>>,
    /// Best master selection state
    master_selection: Arc<Mutex<MasterSelection>>,
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
            master_selection: Arc::new(Mutex::new(MasterSelection::default())),
            shutdown: CancellationToken::new(),
            task: Arc::new(Mutex::new(None)),
            is_running: Arc::new(Mutex::new(false)),
        }
    }

    pub async fn start(&self) -> Result<()> {
        let config = self.config.clone();
        let clock = self.clock.clone();
        let stats = self.stats.clone();
        let master_selection = self.master_selection.clone();
        let shutdown = self.shutdown.child_token();
        let is_running = self.is_running.clone();

        *is_running.lock().unwrap() = true;

        let handle = tokio::spawn(async move {
            if let Err(e) =
                Self::run_ptp_loop(config, clock, stats, master_selection, shutdown).await
            {
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
        self.apply_announce_bytes_at(bytes, Instant::now())
    }

    fn apply_announce_bytes_at(&self, bytes: &[u8], now: Instant) -> Result<()> {
        Self::apply_announce_bytes_to_stats(
            &self.config,
            &self.stats,
            &self.master_selection,
            bytes,
            now,
        )
    }

    #[cfg(test)]
    fn expire_master_candidates_at(&self, now: Instant) {
        Self::refresh_master_selection(&self.stats, &self.master_selection, now);
    }

    fn apply_announce_bytes_to_stats(
        config: &PtpConfig,
        stats: &Arc<Mutex<PtpStats>>,
        master_selection: &Arc<Mutex<MasterSelection>>,
        bytes: &[u8],
        now: Instant,
    ) -> Result<()> {
        let announce = AnnounceMessage::from_bytes(bytes)?;
        if announce.domain_number != config.domain {
            return Ok(());
        }

        if announce.grandmaster_identity == stats.lock().unwrap().local_identity {
            return Ok(());
        }

        let master_identity = {
            let mut master_selection = master_selection.lock().unwrap();
            master_selection.observe(announce, now)
        };

        let mut stats = stats.lock().unwrap();
        stats.announce_count += 1;
        stats.master_identity = master_identity;
        if master_identity.is_some() && stats.state == PtpState::Master {
            stats.state = PtpState::Listening;
        }
        Ok(())
    }

    fn refresh_master_selection(
        stats: &Arc<Mutex<PtpStats>>,
        master_selection: &Arc<Mutex<MasterSelection>>,
        now: Instant,
    ) {
        let master_identity = {
            let mut master_selection = master_selection.lock().unwrap();
            master_selection.refresh(now)
        };

        let mut stats = stats.lock().unwrap();
        stats.master_identity = master_identity;
        if master_identity.is_none() {
            stats.state = PtpState::Master;
        } else if stats.state == PtpState::Master {
            stats.state = PtpState::Listening;
        }
    }

    fn local_master_is_active(stats: &Arc<Mutex<PtpStats>>) -> bool {
        let stats = stats.lock().unwrap();
        stats.master_identity.is_none() && stats.state == PtpState::Master
    }

    fn timing_source_is_selected(stats: &Arc<Mutex<PtpStats>>, header: &PtpHeader) -> bool {
        let selected_master = stats.lock().unwrap().master_identity;
        selected_master.is_none_or(|identity| identity == source_clock_identity(header))
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
        master_selection: Arc<Mutex<MasterSelection>>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        log::info!("Starting PTP client on domain {}", config.domain);

        {
            let mut stats = stats.lock().unwrap();
            stats.state = PtpState::Listening;
        }
        Self::refresh_master_selection(&stats, &master_selection, Instant::now());

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
        let ptp_general_addr = SocketAddrV4::new(Ipv4Addr::new(224, 0, 1, 129), 320);

        let mut last_sync_ts: Option<u64> = None;
        let mut last_sync_seq_id: Option<u16> = None;
        let mut delay_req_sequence_id: u16 = 0;
        let mut pending_delay_request: Option<PendingDelayRequest> = None;
        let mut master_refresh = tokio::time::interval(MASTER_SELECTION_REFRESH);
        let mut local_announce = tokio::time::interval(LOCAL_MASTER_ANNOUNCE_INTERVAL);
        let mut local_sync = tokio::time::interval(LOCAL_MASTER_SYNC_INTERVAL);
        let mut local_announce_sequence_id: u16 = 0;
        let mut local_sync_sequence_id: u16 = 0;

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    log::info!("PTP loop stopping");
                    break;
                }
                _ = master_refresh.tick() => {
                    Self::refresh_master_selection(&stats, &master_selection, Instant::now());
                }
                _ = local_announce.tick() => {
                    if Self::local_master_is_active(&stats) {
                        let local_identity = stats.lock().unwrap().local_identity;
                        let announce = LocalAnnounceMessage {
                            domain_number: config.domain,
                            source_port_identity: local_port_identity,
                            sequence_id: local_announce_sequence_id,
                            log_message_interval: 1,
                            priority1: config.priority1,
                            clock_quality: local_clock_quality(),
                            priority2: config.priority2,
                            grandmaster_identity: local_identity,
                        };
                        local_announce_sequence_id = local_announce_sequence_id.wrapping_add(1);

                        if let Err(error) = general_socket
                            .send_to(&announce.to_bytes(), ptp_general_addr)
                            .await
                        {
                            log::warn!("Failed to send local PTP Announce: {error}");
                        }
                    }
                }
                _ = local_sync.tick() => {
                    if Self::local_master_is_active(&stats) {
                        let timestamp = Timestamp::from_nanos(clock.lock().unwrap().now_ns());
                        let sync = SyncMessage {
                            domain_number: config.domain,
                            source_port_identity: local_port_identity,
                            sequence_id: local_sync_sequence_id,
                            origin_timestamp: timestamp,
                        };
                        let follow_up = FollowUpMessage {
                            domain_number: config.domain,
                            source_port_identity: local_port_identity,
                            sequence_id: local_sync_sequence_id,
                            precise_origin_timestamp: timestamp,
                        };
                        local_sync_sequence_id = local_sync_sequence_id.wrapping_add(1);

                        if let Err(error) = event_socket.send_to(&sync.to_bytes(), ptp_event_addr).await {
                            log::warn!("Failed to send local PTP Sync: {error}");
                        }
                        if let Err(error) = general_socket
                            .send_to(&follow_up.to_bytes(), ptp_general_addr)
                            .await
                        {
                            log::warn!("Failed to send local PTP FollowUp: {error}");
                        }
                    }
                }
                Ok((len, _)) = event_socket.recv_from(&mut event_buf) => {
                    if let Ok(header) = PtpHeader::from_bytes(&event_buf[..len]) {
                        if header.domain_number != config.domain {
                            continue;
                        }
                        if header.source_port_identity == local_port_identity {
                            continue;
                        }

                        match header.message_type {
                            MessageType::Sync => {
                                if !Self::timing_source_is_selected(&stats, &header) {
                                    continue;
                                }

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
                            MessageType::DelayReq => {
                                if !Self::local_master_is_active(&stats) {
                                    continue;
                                }

                                let receive_timestamp = Timestamp::from_nanos(clock.lock().unwrap().now_ns());
                                let delay_resp = DelayRespMessage {
                                    domain_number: config.domain,
                                    sequence_id: header.sequence_id,
                                    receive_timestamp,
                                    requesting_port_identity: header.source_port_identity,
                                };

                                if let Err(error) = general_socket
                                    .send_to(&delay_resp.to_bytes(local_port_identity), ptp_general_addr)
                                    .await
                                {
                                    log::warn!("Failed to send local PTP DelayResp: {error}");
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
                        if header.source_port_identity == local_port_identity {
                            continue;
                        }

                        match header.message_type {
                            MessageType::FollowUp => {
                                if !Self::timing_source_is_selected(&stats, &header) {
                                    continue;
                                }

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
                                    &master_selection,
                                    &general_buf[..len],
                                    Instant::now(),
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

        apply_udp_socket_defaults(&socket, ptp_socket_defaults())?;

        // Bind to wildcard address
        let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), port);
        socket.bind(&addr.into())?;
        socket.set_tos_v4(dscp_to_tos(PTP_DSCP)?)?;

        // Join multicast group 224.0.1.129 (PTP primary)
        let multi_addr = Ipv4Addr::new(224, 0, 1, 129);
        socket.join_multicast_v4(&multi_addr, &interface_ip)?;
        if !interface_ip.is_unspecified() {
            socket.set_multicast_if_v4(&interface_ip)?;
        }

        socket.set_nonblocking(true)?;

        Ok(UdpSocket::from_std(socket.into())?)
    }
}

fn dscp_to_tos(dscp: u8) -> Result<u32> {
    if dscp > 63 {
        return Err(anyhow!("DSCP value {dscp} must be between 0 and 63"));
    }

    Ok((dscp as u32) << 2)
}

fn announce_timeout(log_message_interval: i8) -> Duration {
    let seconds = 2f64.powi(log_message_interval as i32) * ANNOUNCE_RECEIPT_TIMEOUT_MULTIPLIER;
    Duration::from_secs_f64(seconds.clamp(0.1, 60.0))
}

fn local_clock_quality() -> ClockQuality {
    ClockQuality {
        clock_class: 248,
        clock_accuracy: 0xfe,
        offset_scaled_log_variance: 0xffff,
    }
}

fn source_clock_identity(header: &PtpHeader) -> ClockIdentity {
    ClockIdentity::from_bytes([
        header.source_port_identity[0],
        header.source_port_identity[1],
        header.source_port_identity[2],
        header.source_port_identity[3],
        header.source_port_identity[4],
        header.source_port_identity[5],
        header.source_port_identity[6],
        header.source_port_identity[7],
    ])
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

    #[test]
    fn ptp_socket_defaults_use_fixed_timing_values() {
        let defaults = ptp_socket_defaults();

        assert!(defaults.reuse_address);
        assert!(defaults.reuse_port);
        assert!(defaults.multicast_loop_v4);
        assert_eq!(defaults.send_buffer_size, Some(262_144));
        assert_eq!(defaults.recv_buffer_size, Some(262_144));
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

    #[tokio::test]
    async fn start_without_grandmaster_enters_local_master_state() {
        let client = PtpClient::new(PtpConfig {
            domain: 99,
            interface_ip: Ipv4Addr::new(127, 0, 0, 1),
            clock_identity: Some(ClockIdentity::from_bytes([
                0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01,
            ])),
            ..Default::default()
        });

        client.start().await.expect("PTP client should start");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let stats = client.get_stats();
        client.shutdown().await;

        assert_eq!(stats.state, PtpState::Master);
        assert_eq!(stats.master_identity, None);
        assert_eq!(client.reference_clock_identity(), stats.local_identity);
    }

    #[test]
    fn announce_selects_best_master_and_ignores_wrong_domain() {
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
        let worse_master =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x99, 0x88, 0x77]);

        client
            .apply_announce_bytes(&announce_packet(
                7,
                first_master,
                128,
                248,
                0x22,
                0xffff,
                128,
            ))
            .expect("valid announce should be accepted");
        assert_eq!(client.reference_clock_identity(), first_master);

        client
            .apply_announce_bytes(&announce_packet(
                8,
                wrong_domain_master,
                1,
                6,
                0x20,
                0x0100,
                1,
            ))
            .expect("wrong-domain announce should be ignored");
        assert_eq!(client.reference_clock_identity(), first_master);

        client
            .apply_announce_bytes(&announce_packet(
                7,
                second_master,
                100,
                248,
                0x22,
                0xffff,
                128,
            ))
            .expect("better grandmaster should be accepted");
        assert_eq!(client.reference_clock_identity(), second_master);

        client
            .apply_announce_bytes(&announce_packet(
                7,
                worse_master,
                200,
                248,
                0x22,
                0xffff,
                128,
            ))
            .expect("worse grandmaster should be tracked but not selected");
        assert_eq!(client.reference_clock_identity(), second_master);
    }

    #[test]
    fn bmca_uses_clock_quality_priority2_and_identity_tie_breakers() {
        let low_accuracy =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x00, 0x00, 0x30]);
        let better_accuracy =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x00, 0x00, 0x20]);
        let lower_identity =
            ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x00, 0x00, 0x10]);

        assert!(
            MasterCandidate::from_announce(
                &AnnounceMessage::from_bytes(&announce_packet(
                    7,
                    better_accuracy,
                    128,
                    248,
                    0x20,
                    0x1234,
                    128,
                ))
                .unwrap(),
                Instant::now(),
            )
            .is_better_than(&MasterCandidate::from_announce(
                &AnnounceMessage::from_bytes(&announce_packet(
                    7,
                    low_accuracy,
                    128,
                    248,
                    0x30,
                    0x1234,
                    128,
                ))
                .unwrap(),
                Instant::now(),
            ))
        );

        assert!(
            MasterCandidate::from_announce(
                &AnnounceMessage::from_bytes(&announce_packet(
                    7,
                    lower_identity,
                    128,
                    248,
                    0x20,
                    0x1234,
                    128,
                ))
                .unwrap(),
                Instant::now(),
            )
            .is_better_than(&MasterCandidate::from_announce(
                &AnnounceMessage::from_bytes(&announce_packet(
                    7,
                    better_accuracy,
                    128,
                    248,
                    0x20,
                    0x1234,
                    128,
                ))
                .unwrap(),
                Instant::now(),
            ))
        );
    }

    #[test]
    fn expired_master_candidates_are_removed() {
        let client = PtpClient::new(PtpConfig {
            domain: 7,
            clock_identity: Some(ClockIdentity::from_bytes([
                0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01,
            ])),
            ..Default::default()
        });
        let start = Instant::now();
        let local_identity = client.get_stats().local_identity;
        let master = ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x12, 0x34, 0x56]);

        client
            .apply_announce_bytes_at(
                &announce_packet(7, master, 128, 248, 0x22, 0xffff, 128),
                start,
            )
            .expect("valid announce should be accepted");
        assert_eq!(client.reference_clock_identity(), master);

        client.expire_master_candidates_at(start + Duration::from_secs(7));
        assert_eq!(client.reference_clock_identity(), local_identity);
        assert_eq!(client.get_stats().state, PtpState::Master);
        assert!(PtpClient::local_master_is_active(&client.stats));
    }

    #[test]
    fn external_announce_disables_local_master_fallback() {
        let client = PtpClient::new(PtpConfig {
            domain: 7,
            clock_identity: Some(ClockIdentity::from_bytes([
                0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01,
            ])),
            ..Default::default()
        });
        let local_identity = client.get_stats().local_identity;
        let master = ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x12, 0x34, 0x56]);

        client.expire_master_candidates_at(Instant::now());
        assert_eq!(client.reference_clock_identity(), local_identity);
        assert!(PtpClient::local_master_is_active(&client.stats));

        client
            .apply_announce_bytes(&announce_packet(7, master, 128, 248, 0x22, 0xffff, 128))
            .expect("external announce should be accepted");

        assert_eq!(client.reference_clock_identity(), master);
        assert!(!PtpClient::local_master_is_active(&client.stats));
    }

    #[test]
    fn timing_messages_are_filtered_to_selected_master() {
        let selected = ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x12, 0x34, 0x56]);
        let other = ClockIdentity::from_bytes([0x00, 0x1d, 0xc1, 0xff, 0xfe, 0xaa, 0xbb, 0xcc]);
        let stats = Arc::new(Mutex::new(PtpStats {
            master_identity: Some(selected),
            ..Default::default()
        }));

        assert!(PtpClient::timing_source_is_selected(
            &stats,
            &ptp_header_from_source(selected)
        ));
        assert!(!PtpClient::timing_source_is_selected(
            &stats,
            &ptp_header_from_source(other)
        ));

        stats.lock().unwrap().master_identity = None;
        assert!(PtpClient::timing_source_is_selected(
            &stats,
            &ptp_header_from_source(other)
        ));
    }

    fn ptp_header_from_source(source: ClockIdentity) -> PtpHeader {
        let mut source_port_identity = [0u8; 10];
        source_port_identity[0..8].copy_from_slice(&source.as_bytes());
        source_port_identity[8..10].copy_from_slice(&1u16.to_be_bytes());

        PtpHeader {
            message_type: MessageType::Sync,
            version: 2,
            domain_number: 7,
            correction_field: 0,
            source_port_identity,
            sequence_id: 1,
            control_field: 0,
            log_message_interval: 1,
        }
    }

    fn announce_packet(
        domain: u8,
        grandmaster_identity: ClockIdentity,
        priority1: u8,
        clock_class: u8,
        clock_accuracy: u8,
        offset_scaled_log_variance: u16,
        priority2: u8,
    ) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[0] = 0x0b;
        bytes[1] = 0x02;
        bytes[4] = domain;
        bytes[33] = 1;
        bytes[47] = priority1;
        bytes[48] = clock_class;
        bytes[49] = clock_accuracy;
        bytes[50..52].copy_from_slice(&offset_scaled_log_variance.to_be_bytes());
        bytes[52] = priority2;
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
