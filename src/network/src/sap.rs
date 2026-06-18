//! SAP announcement and discovery support for AES67 SDP payloads.
//!
//! The Session Announcement Protocol (SAP) is used here for two complementary
//! workflows:
//!
//! - `SapAnnouncer` periodically sends the streamer's generated SDP to the
//!   standard SAP multicast group.
//! - `SapBrowser` receives SAP datagrams and `parse_sap_packet` turns them into
//!   structured messages that can be cached by `SapRegistry`.
//!
//! The parser focuses on the AES67 release target: IPv4 SAP version 1 messages
//! carrying `application/sdp` payloads. Unsupported SAP features such as IPv6
//! origin sources, encrypted payloads, and compressed payloads are rejected with
//! explicit errors so callers can log or ignore them safely.

use anyhow::{anyhow, Context, Result};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

/// DSCP value used for outgoing SAP discovery/control packets.
///
/// SAP is marked CS3 (`24`) to align with the project's fixed professional QoS
/// policy. This only sets the packet field; network devices must still be
/// configured to honor DSCP markings.
const SAP_DSCP: u8 = 24;

/// SAP protocol version supported by this implementation.
///
/// RFC 2974 SAP version 1 is the interoperable target for the current AES67
/// announcement/browser workflow.
const SAP_VERSION: u8 = 1;

/// Standard IPv4 SAP multicast group used for global-scope announcements.
///
/// AES67 devices commonly announce SDP payloads to `239.255.255.255:9875`.
/// Browser tools should listen on this group unless they are running a test or
/// intentionally targeting a non-standard SAP domain.
pub const SAP_MULTICAST_ADDRESS: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 255);

/// Standard UDP port for SAP announcements.
///
/// `SapAnnouncer` sends to this port and `SapBrowserConfig::new` uses it as the
/// default browser port.
pub const SAP_PORT: u16 = 9875;

/// Stable identity for a SAP announcement stream.
///
/// SAP identifies a message by the tuple of the IPv4 origin source from the SAP
/// header and the 16-bit message hash. `SapRegistry` uses this key to decide
/// whether an incoming announcement creates a new stream, refreshes an existing
/// stream, updates an existing stream, or removes it through a deletion packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SapMessageKey {
    /// IPv4 origin source encoded in the SAP header.
    ///
    /// This is not necessarily the same as the UDP sender address. It is the
    /// source identity chosen by the announcer when the SAP packet was built.
    pub origin_source: Ipv4Addr,

    /// SAP message hash from the packet header.
    ///
    /// The hash groups announcements and deletion packets for the same logical
    /// SAP message. The project announcer derives it from the origin source and
    /// SDP payload.
    pub message_hash: u16,
}

/// Type of SAP message represented by the packet.
///
/// SAP deletion packets are used to withdraw an existing announcement. All
/// non-deletion packets are treated as current announcements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SapMessageType {
    /// A current SAP announcement carrying stream metadata.
    Announcement,

    /// A SAP deletion packet that withdraws an existing announcement.
    Deletion,
}

/// Parsed SAP packet content.
///
/// This is the reusable, owned representation returned by
/// `parse_sap_packet`. For AES67 SDP announcements, both the raw SDP text and a
/// parsed `Aes67SessionDescription` are populated. If the SAP payload is not an
/// SDP payload, or the SDP is not within the supported AES67 subset, `session`
/// is `None` and callers can decide whether to ignore or display the raw
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SapMessage {
    /// Stable key for this SAP message.
    pub key: SapMessageKey,

    /// Whether the message announces or deletes a stream.
    pub message_type: SapMessageType,

    /// MIME type declared in the SAP payload header.
    ///
    /// The parser also accepts legacy packets that omit the MIME type and start
    /// directly with SDP text; those are normalized to `application/sdp`.
    pub payload_type: Option<String>,

    /// Raw SDP payload when the SAP message carries `application/sdp`.
    pub sdp: Option<String>,

    /// Parsed AES67 session metadata extracted from `sdp`.
    ///
    /// This is present only when the SDP matches the currently supported
    /// release target: L24 audio, 48 kHz sample rate, dynamic RTP payload type,
    /// and 1-8 channels.
    pub session: Option<crate::sdp::Aes67SessionDescription>,
}

/// SAP datagram received from the network.
///
/// Combines the parsed `SapMessage` with the UDP source socket address reported
/// by the OS. The source address is useful for diagnostics and filtering, while
/// `SapMessage::key.origin_source` remains the protocol-level SAP identity.
#[derive(Debug, Clone)]
pub struct ReceivedSapMessage {
    /// Parsed SAP message.
    pub message: SapMessage,

    /// UDP source address that sent the datagram.
    pub source: SocketAddr,
}

/// Cached state for a discovered SAP stream.
///
/// A `SapStream` is created and maintained by `SapRegistry`. It keeps the most
/// recent SAP message for a stream plus first/last observation times so browser
/// tools can render stable output and expire announcements that stop refreshing.
#[derive(Debug, Clone)]
pub struct SapStream {
    /// Stable registry key for the stream.
    pub key: SapMessageKey,

    /// Most recent parsed SAP message for the stream.
    pub message: SapMessage,

    /// Most recent UDP source address that sent the announcement.
    pub source: SocketAddr,

    /// Time when this stream first appeared in the registry.
    pub first_seen: Instant,

    /// Time when this stream was most recently announced or updated.
    pub last_seen: Instant,
}

/// Lifecycle event emitted by `SapRegistry`.
///
/// Browser applications should use these events to update their display or
/// notify callers. Repeated identical SAP refreshes do not emit events; they
/// only update `SapStream::last_seen` internally.
#[derive(Debug, Clone)]
pub enum SapRegistryEvent {
    /// A previously unseen stream was discovered.
    Added(SapStream),

    /// A known stream changed SDP content, parsed session metadata, payload
    /// type, or UDP source.
    Updated(SapStream),

    /// A known stream was withdrawn by a SAP deletion packet.
    Removed(SapStream),

    /// A known stream stopped refreshing for longer than the configured expiry
    /// duration.
    Expired(SapStream),
}

/// In-memory cache of SAP-discovered AES67 streams.
///
/// `SapRegistry` is deliberately independent of sockets and async runtime. Feed
/// it parsed `SapMessage` values from `SapBrowser::recv_message` or
/// `parse_sap_packet`, then call `expire` periodically using the caller's clock.
/// Only messages with parsed AES67 session metadata are added to the registry;
/// unsupported or non-SDP SAP packets are ignored by returning `None`.
#[derive(Debug, Clone)]
pub struct SapRegistry {
    expiry: Duration,
    streams: HashMap<SapMessageKey, SapStream>,
}

impl SapRegistry {
    /// Create an empty registry with the provided stale-stream expiry duration.
    ///
    /// A stream is considered expired when `now - last_seen >= expiry` during a
    /// call to `expire`.
    pub fn new(expiry: Duration) -> Self {
        Self {
            expiry,
            streams: HashMap::new(),
        }
    }

    /// Apply one parsed SAP message to the registry.
    ///
    /// Returns:
    ///
    /// - `Some(SapRegistryEvent::Added)` when a supported stream is first seen.
    /// - `Some(SapRegistryEvent::Updated)` when an existing stream changes.
    /// - `Some(SapRegistryEvent::Removed)` when a deletion packet withdraws a
    ///   known stream.
    /// - `None` for identical refreshes, deletion packets for unknown streams,
    ///   and SAP messages that do not contain supported AES67 SDP metadata.
    ///
    /// `source` should be the UDP sender address and `now` should be the time
    /// the datagram was received.
    pub fn apply_message(
        &mut self,
        message: SapMessage,
        source: SocketAddr,
        now: Instant,
    ) -> Option<SapRegistryEvent> {
        if message.message_type == SapMessageType::Deletion {
            return self
                .streams
                .remove(&message.key)
                .map(SapRegistryEvent::Removed);
        }

        message.session.as_ref()?;

        match self.streams.get_mut(&message.key) {
            Some(existing) => {
                let changed = existing.message.sdp != message.sdp
                    || existing.message.session != message.session
                    || existing.message.payload_type != message.payload_type
                    || existing.source != source;

                existing.last_seen = now;
                if changed {
                    existing.message = message;
                    existing.source = source;
                    Some(SapRegistryEvent::Updated(existing.clone()))
                } else {
                    None
                }
            }
            None => {
                let stream = SapStream {
                    key: message.key,
                    message,
                    source,
                    first_seen: now,
                    last_seen: now,
                };
                self.streams.insert(stream.key, stream.clone());
                Some(SapRegistryEvent::Added(stream))
            }
        }
    }

    /// Remove stale streams and return one `Expired` event for each removal.
    ///
    /// Call this periodically from a browser loop. This method does not inspect
    /// the network; it only compares the supplied `now` value with each
    /// stream's `last_seen` timestamp.
    pub fn expire(&mut self, now: Instant) -> Vec<SapRegistryEvent> {
        let expired_keys = self
            .streams
            .iter()
            .filter_map(|(key, stream)| {
                now.checked_duration_since(stream.last_seen)
                    .is_some_and(|age| age >= self.expiry)
                    .then_some(*key)
            })
            .collect::<Vec<_>>();

        expired_keys
            .into_iter()
            .filter_map(|key| self.streams.remove(&key))
            .map(SapRegistryEvent::Expired)
            .collect()
    }

    /// Return the currently tracked streams in stable display order.
    ///
    /// The returned vector is cloned from registry state and sorted by session
    /// name, origin source, and message hash. Mutating the returned streams does
    /// not affect the registry.
    pub fn get_streams(&self) -> Vec<SapStream> {
        let mut streams = self.streams.values().cloned().collect::<Vec<_>>();
        streams.sort_by_key(|stream| {
            (
                stream
                    .message
                    .session
                    .as_ref()
                    .and_then(|session| session.session_name.clone())
                    .unwrap_or_default(),
                stream.key.origin_source,
                stream.key.message_hash,
            )
        });
        streams
    }
}

/// Configuration for a SAP browser receive socket.
///
/// Use `SapBrowserConfig::new(interface)` for normal AES67 SAP discovery. Tests
/// can override `address` and `port` to bind a loopback unicast socket while
/// still exercising the same parser and registry path.
#[derive(Debug, Clone)]
pub struct SapBrowserConfig {
    /// SAP multicast or test listen address.
    ///
    /// Multicast addresses bind the socket to `0.0.0.0` and join the group on
    /// `interface`. Non-multicast addresses bind directly to this address.
    pub address: Ipv4Addr,

    /// UDP port to listen on.
    pub port: u16,

    /// Local IPv4 interface used when joining a multicast SAP group.
    pub interface: Ipv4Addr,

    /// UDP receive buffer size requested for the socket.
    pub recv_buffer_size: usize,
}

impl SapBrowserConfig {
    /// Build the standard SAP browser configuration for an interface.
    ///
    /// Defaults to `SAP_MULTICAST_ADDRESS`, `SAP_PORT`, and a 64 KiB receive
    /// buffer.
    pub fn new(interface: Ipv4Addr) -> Self {
        Self {
            address: SAP_MULTICAST_ADDRESS,
            port: SAP_PORT,
            interface,
            recv_buffer_size: 65_536,
        }
    }
}

/// Async SAP datagram receiver.
///
/// `SapBrowser` owns a nonblocking Tokio UDP socket configured for SAP browsing.
/// It handles binding and multicast group membership, then exposes
/// `recv_message` to receive and parse one SAP datagram at a time.
pub struct SapBrowser {
    socket: UdpSocket,
}

impl SapBrowser {
    /// Create and bind a SAP browser socket from the provided configuration.
    ///
    /// For multicast `config.address`, this binds to `0.0.0.0:config.port` and
    /// joins the multicast group on `config.interface`. For non-multicast
    /// addresses, this binds directly to `config.address:config.port`, which is
    /// primarily useful for deterministic loopback tests.
    pub fn new(config: SapBrowserConfig) -> Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        crate::socket::apply_udp_socket_defaults(
            &socket,
            crate::socket::UdpSocketDefaults {
                reuse_address: true,
                reuse_port: true,
                multicast_loop_v4: false,
                send_buffer_size: None,
                recv_buffer_size: Some(config.recv_buffer_size),
            },
        )?;

        let bind_ip = if config.address.is_multicast() {
            Ipv4Addr::UNSPECIFIED
        } else {
            config.address
        };
        let bind_addr = SocketAddrV4::new(bind_ip, config.port);
        socket
            .bind(&bind_addr.into())
            .with_context(|| format!("Failed to bind SAP browser socket to {bind_addr}"))?;

        if config.address.is_multicast() {
            socket
                .join_multicast_v4(&config.address, &config.interface)
                .with_context(|| {
                    format!(
                        "Failed to join SAP multicast group {} on interface {}",
                        config.address, config.interface
                    )
                })?;
        }

        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(socket.into())?;

        Ok(Self { socket })
    }

    /// Receive and parse one SAP datagram.
    ///
    /// `buffer` is caller-provided scratch storage and should be large enough
    /// for the expected SAP packet. A 64 KiB buffer is sufficient for normal
    /// UDP datagrams. The returned `ReceivedSapMessage` owns the parsed message
    /// data, so the buffer may be reused immediately after this call returns.
    pub async fn recv_message(&self, buffer: &mut [u8]) -> Result<ReceivedSapMessage> {
        let (len, source) = self
            .socket
            .recv_from(buffer)
            .await
            .context("Failed to receive SAP packet")?;
        let message = parse_sap_packet(&buffer[..len])
            .with_context(|| format!("Failed to parse SAP packet from {source}"))?;

        Ok(ReceivedSapMessage { message, source })
    }

    /// Return the OS-assigned local socket address.
    ///
    /// This is mostly useful in tests that bind to port `0` and need to discover
    /// the actual port selected by the OS.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.socket
            .local_addr()
            .context("Failed to get SAP browser socket local address")
    }
}

/// Periodic SAP announcer for SDP payloads.
///
/// `SapAnnouncer` is used by the streamer side. It sends the current SDP
/// payload to `SAP_MULTICAST_ADDRESS:SAP_PORT` every 30 seconds using DSCP 24
/// / CS3 marking and the configured multicast interface. The payload can be
/// updated while the announcer is running, which lets the streamer refresh SDP
/// metadata when its PTP reference changes.
pub struct SapAnnouncer {
    socket: Arc<UdpSocket>,
    sdp_payload: Arc<Mutex<String>>,
    origin_source: Ipv4Addr,
    shutdown: CancellationToken,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl SapAnnouncer {
    /// Create a SAP announcer for an SDP payload and local interface address.
    ///
    /// `interface_ip` is used both as the multicast interface and as the SAP
    /// origin source encoded in outgoing packets.
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

    /// Replace the SDP payload used for future SAP announcements.
    ///
    /// This does not send immediately; the new payload is picked up by the next
    /// interval tick in the background announcer task.
    pub fn update_sdp_payload(&self, sdp_payload: String) {
        *self.sdp_payload.lock().unwrap() = sdp_payload;
    }

    /// Return the currently configured SDP payload.
    ///
    /// This is mainly useful for tests and diagnostics.
    pub fn get_sdp_payload(&self) -> String {
        self.sdp_payload.lock().unwrap().clone()
    }

    /// Start the background SAP announcement task.
    ///
    /// The task sends one SAP packet on each 30-second interval tick until
    /// `stop` or `shutdown` is called.
    pub async fn start(&self) -> Result<()> {
        let sap_addr = SocketAddrV4::new(SAP_MULTICAST_ADDRESS, SAP_PORT);
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

    /// Signal the background announcer task to stop.
    ///
    /// This method is non-blocking. Use `shutdown` when the caller also needs to
    /// wait for the task to finish.
    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    /// Stop the announcer and wait for its background task to finish.
    ///
    /// Join failures are logged and otherwise ignored so shutdown paths do not
    /// mask the primary streaming result.
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

/// Parse a raw SAP datagram into an owned `SapMessage`.
///
/// Supported packets:
///
/// - SAP version 1.
/// - IPv4 origin source addresses.
/// - Unencrypted and uncompressed payloads.
/// - `application/sdp` payloads, including legacy packets that omit the MIME
///   type and begin directly with SDP text.
///
/// Unsupported SAP features return an error rather than a partial message. SDP
/// parse failures do not make the SAP parse fail; instead, the returned
/// `SapMessage` keeps the raw SDP text and sets `session` to `None`. This lets
/// browsers report or save unsupported SDP while still avoiding false AES67
/// discovery events in `SapRegistry`.
pub fn parse_sap_packet(packet: &[u8]) -> Result<SapMessage> {
    if packet.len() < 8 {
        return Err(anyhow!("SAP packet is too short"));
    }

    let flags = packet[0];
    let version = flags >> 5;
    if version != SAP_VERSION {
        return Err(anyhow!("unsupported SAP version {version}"));
    }
    if flags & 0x10 != 0 {
        return Err(anyhow!("SAP IPv6 origin sources are not supported"));
    }
    if flags & 0x02 != 0 {
        return Err(anyhow!("encrypted SAP payloads are not supported"));
    }
    if flags & 0x01 != 0 {
        return Err(anyhow!("compressed SAP payloads are not supported"));
    }

    let auth_len_bytes = packet[1] as usize * 4;
    let message_hash = u16::from_be_bytes([packet[2], packet[3]]);
    let origin_source = Ipv4Addr::new(packet[4], packet[5], packet[6], packet[7]);
    let payload_start = 8usize
        .checked_add(auth_len_bytes)
        .ok_or_else(|| anyhow!("SAP authentication length overflows packet size"))?;
    if packet.len() < payload_start {
        return Err(anyhow!("SAP packet authentication data is truncated"));
    }

    let payload = &packet[payload_start..];
    if payload.is_empty() {
        return Err(anyhow!("SAP packet payload is empty"));
    }

    let (payload_type, body) = parse_sap_payload(payload)?;
    let sdp = if payload_type.as_deref() == Some("application/sdp") {
        Some(
            std::str::from_utf8(body)
                .context("SAP SDP payload is not valid UTF-8")?
                .to_string(),
        )
    } else {
        None
    };
    let session = sdp
        .as_deref()
        .and_then(|sdp| crate::sdp::parse_sdp(sdp).ok());

    Ok(SapMessage {
        key: SapMessageKey {
            origin_source,
            message_hash,
        },
        message_type: if flags & 0x04 == 0 {
            SapMessageType::Announcement
        } else {
            SapMessageType::Deletion
        },
        payload_type,
        sdp,
        session,
    })
}

/// Split the SAP payload into MIME type and body bytes.
///
/// RFC-style SAP payloads start with a null-terminated MIME content type such
/// as `application/sdp`, followed by the payload body. Some real-world SAP
/// senders omit the MIME prefix and place SDP text directly after the SAP
/// header; those packets are accepted and normalized to `application/sdp`.
fn parse_sap_payload(payload: &[u8]) -> Result<(Option<String>, &[u8])> {
    if looks_like_sdp(payload) {
        return Ok((Some("application/sdp".to_string()), payload));
    }

    let Some(separator) = payload.iter().position(|byte| *byte == 0) else {
        return Err(anyhow!("SAP payload type is not null-terminated"));
    };
    let payload_type = std::str::from_utf8(&payload[..separator])
        .context("SAP payload type is not valid UTF-8")?
        .trim()
        .to_ascii_lowercase();
    if payload_type.is_empty() {
        return Err(anyhow!("SAP payload type is empty"));
    }

    Ok((Some(payload_type), &payload[separator + 1..]))
}

/// Return whether payload bytes appear to start directly with SDP text.
///
/// This heuristic supports legacy or minimal SAP senders that omit the
/// `application/sdp` MIME prefix. SDP commonly starts with `v=`, `o=`, or `s=`;
/// if one of those prefixes is present, the parser treats the entire payload as
/// SDP body bytes.
fn looks_like_sdp(payload: &[u8]) -> bool {
    payload.starts_with(b"v=") || payload.starts_with(b"o=") || payload.starts_with(b"s=")
}

/// Build a SAP version 1 IPv4 announcement packet for an SDP payload.
///
/// The packet uses an `application/sdp` MIME payload and a deterministic
/// project-local message hash. This helper is crate-private because the public
/// production API for sending announcements is `SapAnnouncer`.
pub(crate) fn build_sap_packet(sdp_payload: &str, origin_source: Ipv4Addr) -> Vec<u8> {
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

/// Derive the 16-bit SAP message hash used by this project's announcer.
///
/// The SAP RFC leaves hash generation to the announcer. This implementation
/// uses a deterministic FNV-style fold over the origin source and SDP payload so
/// identical announcements produce stable keys while payload or origin changes
/// produce different hashes in normal use. A zero folded value is remapped to
/// `1` so the generated hash is never zero.
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
    if folded == 0 {
        1
    } else {
        folded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{SocketAddr, UdpSocket as StdUdpSocket};
    use std::time::Instant;

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

    #[test]
    fn parses_application_sdp_sap_packet_into_aes67_session() {
        let origin_source = Ipv4Addr::new(192, 168, 1, 50);
        let packet = build_sap_packet(
            "v=0\r\n\
             o=- 0 0 IN IP4 192.168.1.50\r\n\
             s=Studio Main\r\n\
             c=IN IP4 239.69.83.1/32\r\n\
             t=0 0\r\n\
             m=audio 5004 RTP/AVP 97\r\n\
             a=rtpmap:97 L24/48000/2\r\n\
             a=ptime:1\r\n",
            origin_source,
        );

        let message = parse_sap_packet(&packet).expect("real SAP packet should parse");

        assert_eq!(message.key.origin_source, origin_source);
        assert_eq!(message.message_type, SapMessageType::Announcement);
        assert_eq!(message.payload_type.as_deref(), Some("application/sdp"));
        assert_eq!(
            message
                .session
                .as_ref()
                .and_then(|session| session.session_name.as_deref()),
            Some("Studio Main")
        );
        let session = message
            .session
            .as_ref()
            .expect("AES67 SDP should produce a session");
        assert_eq!(session.address, Ipv4Addr::new(239, 69, 83, 1));
        assert_eq!(session.port, 5004);
        assert_eq!(session.channels, 2);
    }

    #[test]
    fn parses_legacy_sap_packet_with_omitted_application_sdp_payload_type() {
        let origin_source = Ipv4Addr::new(192, 168, 1, 51);
        let sdp = "v=0\r\n\
             s=Legacy SAP\r\n\
             c=IN IP4 239.69.83.2/32\r\n\
             m=audio 5006 RTP/AVP 98\r\n\
             a=rtpmap:98 L24/48000/8\r\n";
        let mut packet = Vec::new();
        packet.push(0x20);
        packet.push(0x00);
        packet.extend_from_slice(&0x3456u16.to_be_bytes());
        packet.extend_from_slice(&origin_source.octets());
        packet.extend_from_slice(sdp.as_bytes());

        let message = parse_sap_packet(&packet).expect("legacy SAP packet should parse");

        assert_eq!(message.payload_type.as_deref(), Some("application/sdp"));
        let session = message
            .session
            .expect("legacy SDP should produce a session");
        assert_eq!(session.session_name.as_deref(), Some("Legacy SAP"));
        assert_eq!(session.payload_type, 98);
        assert_eq!(session.channels, 8);
    }

    #[test]
    fn sap_registry_reports_added_updated_removed_and_expired_streams() {
        let origin_source = Ipv4Addr::new(192, 168, 1, 52);
        let sender = SocketAddr::from(([192, 168, 1, 52], 9875));
        let mut registry = SapRegistry::new(Duration::from_secs(30));
        let now = Instant::now();

        let first = parse_sap_packet(&build_sap_packet(
            "v=0\r\n\
             s=Registry Stream\r\n\
             c=IN IP4 239.69.83.3/32\r\n\
             m=audio 5008 RTP/AVP 99\r\n\
             a=rtpmap:99 L24/48000/2\r\n",
            origin_source,
        ))
        .expect("first SAP packet should parse");
        let first_key = first.key;

        let event = registry
            .apply_message(first, sender, now)
            .expect("new stream should produce an event");
        assert!(matches!(event, SapRegistryEvent::Added(_)));
        assert_eq!(registry.get_streams().len(), 1);

        let refresh = parse_sap_packet(&build_sap_packet(
            "v=0\r\n\
             s=Registry Stream\r\n\
             c=IN IP4 239.69.83.3/32\r\n\
             m=audio 5008 RTP/AVP 99\r\n\
             a=rtpmap:99 L24/48000/2\r\n",
            origin_source,
        ))
        .expect("refresh SAP packet should parse");
        assert!(
            registry
                .apply_message(refresh, sender, now + Duration::from_secs(5))
                .is_none(),
            "unchanged refresh should only update last_seen"
        );

        let mut changed_packet = build_sap_packet(
            "v=0\r\n\
             s=Registry Stream Updated\r\n\
             c=IN IP4 239.69.83.3/32\r\n\
             m=audio 5008 RTP/AVP 99\r\n\
             a=rtpmap:99 L24/48000/2\r\n",
            origin_source,
        );
        changed_packet[2..4].copy_from_slice(&first_key.message_hash.to_be_bytes());
        let changed = parse_sap_packet(&changed_packet).expect("changed SAP packet should parse");
        let event = registry
            .apply_message(changed, sender, now + Duration::from_secs(10))
            .expect("changed stream should produce an event");
        assert!(matches!(event, SapRegistryEvent::Updated(_)));

        let expired = registry.expire(now + Duration::from_secs(41));
        assert_eq!(expired.len(), 1);
        assert!(matches!(expired[0], SapRegistryEvent::Expired(_)));
        assert!(registry.get_streams().is_empty());
    }

    #[test]
    fn sap_registry_removes_stream_on_deletion_packet() {
        let origin_source = Ipv4Addr::new(192, 168, 1, 53);
        let sender = SocketAddr::from(([192, 168, 1, 53], 9875));
        let mut registry = SapRegistry::new(Duration::from_secs(30));
        let now = Instant::now();
        let announcement = parse_sap_packet(&build_sap_packet(
            "v=0\r\n\
             s=Deleted Stream\r\n\
             c=IN IP4 239.69.83.4/32\r\n\
             m=audio 5010 RTP/AVP 100\r\n\
             a=rtpmap:100 L24/48000/2\r\n",
            origin_source,
        ))
        .expect("announcement should parse");
        let key = announcement.key;
        registry.apply_message(announcement, sender, now);

        let mut deletion = build_sap_packet("o=- 0 0 IN IP4 192.168.1.53\r\n", origin_source);
        deletion[0] |= 0x04;
        deletion[2..4].copy_from_slice(&key.message_hash.to_be_bytes());
        let deletion = parse_sap_packet(&deletion).expect("deletion SAP packet should parse");

        let event = registry
            .apply_message(deletion, sender, now + Duration::from_secs(1))
            .expect("deletion should produce a removed event");

        assert!(matches!(event, SapRegistryEvent::Removed(_)));
        assert!(registry.get_streams().is_empty());
    }

    #[tokio::test]
    async fn sap_browser_receives_real_sap_udp_datagram() {
        let browser = SapBrowser::new(SapBrowserConfig {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            interface: Ipv4Addr::LOCALHOST,
            recv_buffer_size: 65_536,
        })
        .expect("SAP browser should bind loopback UDP socket");
        let local_addr = browser.local_addr().expect("browser local addr");
        let sender = StdUdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("sender socket should bind");
        let packet = build_sap_packet(
            "v=0\r\n\
             s=UDP Stream\r\n\
             c=IN IP4 239.69.83.5/32\r\n\
             m=audio 5012 RTP/AVP 101\r\n\
             a=rtpmap:101 L24/48000/2\r\n",
            Ipv4Addr::LOCALHOST,
        );
        sender
            .send_to(&packet, local_addr)
            .expect("real SAP UDP datagram should send");

        let mut buffer = [0u8; 2048];
        let received =
            tokio::time::timeout(Duration::from_secs(2), browser.recv_message(&mut buffer))
                .await
                .expect("SAP datagram should arrive")
                .expect("SAP datagram should parse");

        assert_eq!(
            received
                .message
                .session
                .and_then(|session| session.session_name),
            Some("UDP Stream".to_string())
        );
    }
}
