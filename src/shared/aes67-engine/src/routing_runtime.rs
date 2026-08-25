//! Live multi-stream sender runtime for the revisioned routing model.
//!
//! One runtime owns PTP for the process, decodes each routed file source once
//! per packet interval, and fans that packet out to independent RTP/SAP output
//! stages. User interfaces read low-rate snapshots; the packet loop never
//! waits for a frontend consumer.

use crate::routing::{
    RouteAssignment, RoutingSnapshot, SourceId, SourceInput, StreamConfig, StreamId,
};
use anyhow::{anyhow, bail, Context, Result};
use audio::{AudioNode, AudioReader, AudioSample, GainNode};
use network::{
    resolve_interface_ip, MulticastConfig, MulticastSocket, RtpPacketizer, SapAnnouncer,
};
use ptp::{ClockIdentity, PtpClient, PtpConfig, PtpState};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;

const SAMPLE_RATE: u32 = 48_000;
const PACKET_TIME_MS: u32 = 1;
const SAMPLES_PER_PACKET: usize = 48;
const PAYLOAD_TYPE: u8 = 97;
const DEFAULT_TTL: u8 = 32;
const L24_BYTES_PER_SAMPLE: usize = 3;
const MAX_RTP_PAYLOAD_BYTES: usize = 1_460;
const STATS_INTERVAL: Duration = Duration::from_millis(250);
const METER_FLOOR_DBFS: f32 = -120.0;

#[derive(Debug, Clone)]
pub struct RoutingRuntimeConfig {
    pub interface: String,
    pub ptp_domain: u8,
    pub sap: bool,
}

impl Default for RoutingRuntimeConfig {
    fn default() -> Self {
        Self {
            interface: "127.0.0.1".to_string(),
            ptp_domain: 0,
            sap: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingRuntimeLifecycle {
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamRuntimeLifecycle {
    Starting,
    Live,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct PtpRuntimeStats {
    pub state: String,
    pub offset_ns: i64,
    pub master_identity: Option<String>,
}

impl Default for PtpRuntimeStats {
    fn default() -> Self {
        Self {
            state: "stopped".to_string(),
            offset_ns: 0,
            master_identity: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamRuntimeStats {
    pub stream_id: StreamId,
    pub lifecycle: StreamRuntimeLifecycle,
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub packets_per_second: f64,
    pub megabits_per_second: f64,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
    pub late_packets: u64,
    pub sdp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutingRuntimeSnapshot {
    pub lifecycle: RoutingRuntimeLifecycle,
    pub interface: Option<String>,
    pub uptime_seconds: f64,
    pub ptp: PtpRuntimeStats,
    pub streams: Vec<StreamRuntimeStats>,
    pub error: Option<String>,
}

impl Default for RoutingRuntimeSnapshot {
    fn default() -> Self {
        Self {
            lifecycle: RoutingRuntimeLifecycle::Stopped,
            interface: None,
            uptime_seconds: 0.0,
            ptp: PtpRuntimeStats::default(),
            streams: Vec::new(),
            error: None,
        }
    }
}

#[derive(Debug, Default)]
struct RuntimeControl {
    preparing: bool,
    shutdown: Option<CancellationToken>,
    task: Option<JoinHandle<()>>,
}

/// Process-level sender service used by CLI, TUI, and desktop adapters.
#[derive(Debug, Clone, Default)]
pub struct RoutingRuntime {
    control: Arc<tokio::sync::Mutex<RuntimeControl>>,
    snapshot: Arc<Mutex<RoutingRuntimeSnapshot>>,
}

impl RoutingRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_snapshot(&self) -> RoutingRuntimeSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn get_stream_sdp(&self, stream_id: StreamId) -> Option<String> {
        self.get_snapshot()
            .streams
            .into_iter()
            .find(|stream| stream.stream_id == stream_id)
            .map(|stream| stream.sdp)
    }

    pub async fn start(
        &self,
        routing: RoutingSnapshot,
        config: RoutingRuntimeConfig,
    ) -> Result<RoutingRuntimeSnapshot> {
        let finished_task = {
            let mut control = self.control.lock().await;
            if control.preparing {
                bail!("stream runtime is already starting");
            }
            if control
                .task
                .as_ref()
                .is_some_and(|task| !task.is_finished())
            {
                bail!("stream runtime is already running");
            }
            control.preparing = true;
            control.task.take()
        };
        if let Some(task) = finished_task {
            let _ = task.await;
        }

        self.replace_snapshot(RoutingRuntimeSnapshot {
            lifecycle: RoutingRuntimeLifecycle::Starting,
            interface: Some(config.interface.clone()),
            ..RoutingRuntimeSnapshot::default()
        });

        let prepared = match PreparedRuntime::new(&routing, &config).await {
            Ok(prepared) => prepared,
            Err(error) => {
                let message = format!("{error:#}");
                self.replace_snapshot(RoutingRuntimeSnapshot {
                    lifecycle: RoutingRuntimeLifecycle::Failed,
                    interface: Some(config.interface),
                    error: Some(message.clone()),
                    ..RoutingRuntimeSnapshot::default()
                });
                self.control.lock().await.preparing = false;
                return Err(anyhow!(message));
            }
        };

        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let shared_snapshot = Arc::clone(&self.snapshot);
        let initial_snapshot = prepared.initial_snapshot();
        self.replace_snapshot(initial_snapshot);

        let task = tokio::spawn(async move {
            let interface = prepared.interface.to_string();
            if let Err(error) = prepared
                .run(task_shutdown, Arc::clone(&shared_snapshot))
                .await
            {
                let mut snapshot = shared_snapshot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                snapshot.lifecycle = RoutingRuntimeLifecycle::Failed;
                snapshot.error = Some(format!("{error:#}"));
                snapshot.interface = Some(interface);
                for stream in &mut snapshot.streams {
                    stream.lifecycle = StreamRuntimeLifecycle::Failed;
                }
            }
        });

        let mut control = self.control.lock().await;
        control.preparing = false;
        control.shutdown = Some(shutdown);
        control.task = Some(task);
        drop(control);

        Ok(self.get_snapshot())
    }

    pub async fn stop(&self) -> RoutingRuntimeSnapshot {
        let (shutdown, task) = {
            let mut control = self.control.lock().await;
            (control.shutdown.take(), control.task.take())
        };
        if let Some(shutdown) = shutdown {
            shutdown.cancel();
        }
        if let Some(task) = task {
            let _ = task.await;
        }
        self.get_snapshot()
    }

    fn replace_snapshot(&self, next: RoutingRuntimeSnapshot) {
        *self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }
}

struct PreparedSource {
    reader: AudioReader,
    packet: AudioSample,
}

struct PreparedStream {
    id: StreamId,
    source_id: SourceId,
    gain: GainNode,
    packetizer: RtpPacketizer,
    socket: MulticastSocket,
    sap: Option<SapAnnouncer>,
    output_sample: AudioSample,
    packet_buffer: Vec<u8>,
    stats: StreamRuntimeStats,
    published_packets: u64,
    published_bytes: u64,
}

struct PreparedRuntime {
    interface: std::net::Ipv4Addr,
    ptp: PtpClient,
    sources: BTreeMap<SourceId, PreparedSource>,
    streams: Vec<PreparedStream>,
}

impl PreparedRuntime {
    async fn new(routing: &RoutingSnapshot, config: &RoutingRuntimeConfig) -> Result<Self> {
        if routing.routes.is_empty() {
            bail!("create at least one source-to-stream route before starting");
        }

        let interface = resolve_interface_ip(&config.interface)
            .with_context(|| format!("invalid send interface {}", config.interface))?;
        let sources = prepare_sources(routing)?;

        let ptp = PtpClient::new(PtpConfig {
            domain: config.ptp_domain,
            interface_ip: interface,
            ..PtpConfig::default()
        });
        ptp.start()
            .await
            .context("failed to start shared PTP runtime")?;
        let streams = match prepare_streams(routing, config, interface, &sources, &ptp).await {
            Ok(streams) => streams,
            Err(error) => {
                ptp.shutdown().await;
                return Err(error);
            }
        };

        Ok(Self {
            interface,
            ptp,
            sources,
            streams,
        })
    }

    fn initial_snapshot(&self) -> RoutingRuntimeSnapshot {
        RoutingRuntimeSnapshot {
            lifecycle: RoutingRuntimeLifecycle::Running,
            interface: Some(self.interface.to_string()),
            uptime_seconds: 0.0,
            ptp: ptp_snapshot(&self.ptp),
            streams: self
                .streams
                .iter()
                .map(|stream| {
                    let mut stats = stream.stats.clone();
                    stats.lifecycle = StreamRuntimeLifecycle::Live;
                    stats
                })
                .collect(),
            error: None,
        }
    }

    async fn run(
        mut self,
        shutdown: CancellationToken,
        shared_snapshot: Arc<Mutex<RoutingRuntimeSnapshot>>,
    ) -> Result<()> {
        for stream in &mut self.streams {
            stream.stats.lifecycle = StreamRuntimeLifecycle::Live;
        }

        let started = Instant::now();
        let packet_duration = Duration::from_millis(PACKET_TIME_MS as u64);
        let mut packet_index = 0u32;
        let mut published_at = started;
        let run_result = async {
            loop {
                if shutdown.is_cancelled() {
                    break;
                }

                for source in self.sources.values_mut() {
                    if !source
                        .reader
                        .read_next_frame_into(&mut source.packet)
                        .context("failed to decode routed audio source")?
                    {
                        source.reader.rewind();
                        if !source
                            .reader
                            .read_next_frame_into(&mut source.packet)
                            .context("failed to restart routed audio source")?
                        {
                            bail!("routed audio source is shorter than one AES67 packet");
                        }
                    }
                }

                let target_time = started + packet_duration * packet_index;
                for stream in &mut self.streams {
                    let source = self.sources.get(&stream.source_id).ok_or_else(|| {
                        anyhow!(
                            "runtime source {} disappeared",
                            stream.source_id.get_value()
                        )
                    })?;
                    stream.output_sample.channels = source.packet.channels;
                    stream.output_sample.sample_rate = source.packet.sample_rate;
                    stream.output_sample.frames = source.packet.frames;
                    stream.output_sample.data.clone_from(&source.packet.data);
                    stream
                        .gain
                        .process(&mut stream.output_sample)
                        .context("failed to apply stream gain")?;

                    let (peak_dbfs, rms_dbfs) = measure_dbfs(&stream.output_sample.data);
                    stream.stats.peak_dbfs = peak_dbfs;
                    stream.stats.rms_dbfs = rms_dbfs;
                    stream
                        .packetizer
                        .write_packet_into(&stream.output_sample, &mut stream.packet_buffer)
                        .context("failed to packetize routed audio")?;
                    let sent = stream
                        .socket
                        .send_packet(&stream.packet_buffer)
                        .with_context(|| {
                            format!("failed to send stream {}", stream.id.get_value())
                        })?;
                    stream.stats.packets_sent += 1;
                    stream.stats.bytes_sent += sent as u64;
                    if Instant::now() > target_time + packet_duration {
                        stream.stats.late_packets += 1;
                    }
                }

                packet_index = packet_index.wrapping_add(1);
                let now = Instant::now();
                if now.duration_since(published_at) >= STATS_INTERVAL {
                    let interval_seconds = now.duration_since(published_at).as_secs_f64();
                    for stream in &mut self.streams {
                        let packet_delta = stream.stats.packets_sent - stream.published_packets;
                        let byte_delta = stream.stats.bytes_sent - stream.published_bytes;
                        stream.stats.packets_per_second = packet_delta as f64 / interval_seconds;
                        stream.stats.megabits_per_second =
                            byte_delta as f64 * 8.0 / interval_seconds / 1_000_000.0;
                        stream.published_packets = stream.stats.packets_sent;
                        stream.published_bytes = stream.stats.bytes_sent;
                    }
                    publish_snapshot(
                        &shared_snapshot,
                        RoutingRuntimeLifecycle::Running,
                        self.interface,
                        started.elapsed(),
                        ptp_snapshot(&self.ptp),
                        &self.streams,
                        None,
                    );
                    published_at = now;
                }

                let next_packet_at = started + packet_duration * packet_index;
                if next_packet_at > Instant::now() {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        _ = time::sleep_until(next_packet_at.into()) => {}
                    }
                } else {
                    tokio::task::yield_now().await;
                }
            }
            Ok(())
        }
        .await;

        for stream in &mut self.streams {
            if let Some(sap) = &stream.sap {
                sap.shutdown().await;
            }
            stream.stats.lifecycle = if run_result.is_ok() {
                StreamRuntimeLifecycle::Stopped
            } else {
                StreamRuntimeLifecycle::Failed
            };
        }
        self.ptp.shutdown().await;
        publish_snapshot(
            &shared_snapshot,
            if run_result.is_ok() {
                RoutingRuntimeLifecycle::Stopped
            } else {
                RoutingRuntimeLifecycle::Failed
            },
            self.interface,
            started.elapsed(),
            ptp_snapshot(&self.ptp),
            &self.streams,
            run_result.as_ref().err().map(|error| format!("{error:#}")),
        );

        run_result
    }
}

fn prepare_sources(routing: &RoutingSnapshot) -> Result<BTreeMap<SourceId, PreparedSource>> {
    let routed_source_ids = routing
        .routes
        .iter()
        .map(|route| route.source_id)
        .collect::<BTreeSet<_>>();
    let mut sources = BTreeMap::new();

    for source_id in routed_source_ids {
        let source = routing
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| anyhow!("route references missing source {}", source_id.get_value()))?;
        let path = match &source.config.input {
            SourceInput::File { path } => path,
            SourceInput::LiveInput { device } => {
                bail!(
                    "live input '{device}' is not available in the sender runtime yet; choose an audio file"
                )
            }
        };
        let reader = AudioReader::with_resampling(path, SAMPLE_RATE, SAMPLES_PER_PACKET)
            .with_context(|| format!("failed to load source {} from {path}", source.config.name))?;
        let channels = reader.get_info().channels;
        validate_audio_channels(channels)?;
        sources.insert(
            source_id,
            PreparedSource {
                reader,
                packet: AudioSample {
                    data: Vec::with_capacity(SAMPLES_PER_PACKET * channels as usize),
                    channels,
                    sample_rate: SAMPLE_RATE,
                    frames: SAMPLES_PER_PACKET,
                },
            },
        );
    }

    Ok(sources)
}

async fn prepare_streams(
    routing: &RoutingSnapshot,
    config: &RoutingRuntimeConfig,
    interface: std::net::Ipv4Addr,
    sources: &BTreeMap<SourceId, PreparedSource>,
    ptp: &PtpClient,
) -> Result<Vec<PreparedStream>> {
    let clock_identity = ptp.get_reference_clock_identity();
    let base_timestamp = ptp
        .rtp_timestamp(SAMPLE_RATE)
        .context("failed to create PTP-derived RTP timestamp")?;
    let mut streams = Vec::with_capacity(routing.routes.len());

    for route in &routing.routes {
        let stream_config = routing
            .streams
            .iter()
            .find(|stream| stream.id == route.stream_id)
            .map(|stream| &stream.config)
            .ok_or_else(|| {
                anyhow!(
                    "route references missing stream {}",
                    route.stream_id.get_value()
                )
            })?;
        let source = sources.get(&route.source_id).ok_or_else(|| {
            anyhow!(
                "route references missing source {}",
                route.source_id.get_value()
            )
        })?;
        let channels = source.reader.get_info().channels;
        validate_audio_channels(channels)?;

        let multicast_config = MulticastConfig {
            ttl: DEFAULT_TTL,
            ..MulticastConfig::new(stream_config.address, stream_config.port, interface)
        };
        let socket = MulticastSocket::new(multicast_config).with_context(|| {
            format!(
                "failed to create RTP socket for stream {}",
                stream_config.name
            )
        })?;
        let mut packetizer = RtpPacketizer::new(PAYLOAD_TYPE, random_ssrc());
        packetizer.set_base_timestamp(base_timestamp);
        let sdp = build_stream_sdp(
            routing.revision,
            route.stream_id,
            stream_config,
            interface,
            channels,
            clock_identity,
            config.ptp_domain,
        );

        streams.push(PreparedStream {
            id: route.stream_id,
            source_id: route.source_id,
            gain: match stream_config.gain_db {
                Some(gain_db) => GainNode::new_db(gain_db),
                None => GainNode::new_linear(0.0),
            },
            packetizer,
            socket,
            sap: None,
            output_sample: AudioSample {
                data: Vec::with_capacity(SAMPLES_PER_PACKET * channels as usize),
                channels,
                sample_rate: SAMPLE_RATE,
                frames: SAMPLES_PER_PACKET,
            },
            packet_buffer: Vec::with_capacity(
                12 + SAMPLES_PER_PACKET * channels as usize * L24_BYTES_PER_SAMPLE,
            ),
            stats: StreamRuntimeStats {
                stream_id: route.stream_id,
                lifecycle: StreamRuntimeLifecycle::Starting,
                packets_sent: 0,
                bytes_sent: 0,
                packets_per_second: 0.0,
                megabits_per_second: 0.0,
                peak_dbfs: METER_FLOOR_DBFS,
                rms_dbfs: METER_FLOOR_DBFS,
                late_packets: 0,
                sdp,
            },
            published_packets: 0,
            published_bytes: 0,
        });
    }

    if config.sap {
        for index in 0..streams.len() {
            let announcer = match SapAnnouncer::new(streams[index].stats.sdp.clone(), interface) {
                Ok(announcer) => announcer,
                Err(error) => {
                    shutdown_sap(&mut streams).await;
                    return Err(error).context("failed to create SAP announcer");
                }
            };
            if let Err(error) = announcer.start().await {
                shutdown_sap(&mut streams).await;
                return Err(error).context("failed to start SAP announcer");
            }
            streams[index].sap = Some(announcer);
        }
    }

    Ok(streams)
}

fn validate_audio_channels(channels: u32) -> Result<()> {
    if !(1..=8).contains(&channels) {
        bail!("AES67 routing supports 1 to 8 channels, got {channels}");
    }
    let payload_bytes = SAMPLES_PER_PACKET * channels as usize * L24_BYTES_PER_SAMPLE;
    if payload_bytes > MAX_RTP_PAYLOAD_BYTES {
        bail!("RTP payload would be {payload_bytes} bytes, above {MAX_RTP_PAYLOAD_BYTES}");
    }
    Ok(())
}

async fn shutdown_sap(streams: &mut [PreparedStream]) {
    for stream in streams {
        if let Some(sap) = &stream.sap {
            sap.shutdown().await;
        }
    }
}

fn publish_snapshot(
    shared: &Arc<Mutex<RoutingRuntimeSnapshot>>,
    lifecycle: RoutingRuntimeLifecycle,
    interface: std::net::Ipv4Addr,
    uptime: Duration,
    ptp: PtpRuntimeStats,
    streams: &[PreparedStream],
    error: Option<String>,
) {
    *shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = RoutingRuntimeSnapshot {
        lifecycle,
        interface: Some(interface.to_string()),
        uptime_seconds: uptime.as_secs_f64(),
        ptp,
        streams: streams.iter().map(|stream| stream.stats.clone()).collect(),
        error,
    };
}

fn ptp_snapshot(ptp: &PtpClient) -> PtpRuntimeStats {
    let stats = ptp.get_stats();
    PtpRuntimeStats {
        state: match stats.state {
            PtpState::Initializing => "initializing",
            PtpState::Listening => "listening",
            PtpState::Uncalibrated => "uncalibrated",
            PtpState::Slave => "slave",
            PtpState::Master => "master",
            PtpState::Passive => "passive",
        }
        .to_string(),
        offset_ns: stats.offset_ns,
        master_identity: stats.master_identity.map(|identity| identity.to_string()),
    }
}

pub fn preview_stream_sdp(
    routing: &RoutingSnapshot,
    stream_id: StreamId,
    config: &RoutingRuntimeConfig,
) -> Result<String> {
    let interface = resolve_interface_ip(&config.interface)
        .with_context(|| format!("invalid send interface {}", config.interface))?;
    let stream = routing
        .streams
        .iter()
        .find(|stream| stream.id == stream_id)
        .ok_or_else(|| anyhow!("unknown routing stream {}", stream_id.get_value()))?;
    let route = routing
        .routes
        .iter()
        .find(|route| route.stream_id == stream_id)
        .ok_or_else(|| anyhow!("stream {} has no source route", stream_id.get_value()))?;
    let channels = source_channel_count(routing, route)?;
    let ptp = PtpClient::new(PtpConfig {
        domain: config.ptp_domain,
        interface_ip: interface,
        ..PtpConfig::default()
    });

    Ok(build_stream_sdp(
        routing.revision,
        stream_id,
        &stream.config,
        interface,
        channels,
        ptp.get_reference_clock_identity(),
        config.ptp_domain,
    ))
}

fn source_channel_count(routing: &RoutingSnapshot, route: &RouteAssignment) -> Result<u32> {
    let source = routing
        .sources
        .iter()
        .find(|source| source.id == route.source_id)
        .ok_or_else(|| {
            anyhow!(
                "route references missing source {}",
                route.source_id.get_value()
            )
        })?;
    match &source.config.input {
        SourceInput::File { path } => {
            let reader = AudioReader::with_resampling(path, SAMPLE_RATE, SAMPLES_PER_PACKET)
                .with_context(|| {
                    format!("failed to load source {} from {path}", source.config.name)
                })?;
            validate_audio_channels(reader.get_info().channels)?;
            Ok(reader.get_info().channels)
        }
        SourceInput::LiveInput { device } => {
            bail!("live input '{device}' has no negotiated channel count yet; choose an audio file")
        }
    }
}

fn build_stream_sdp(
    revision: u64,
    stream_id: StreamId,
    stream: &StreamConfig,
    local_ip: std::net::Ipv4Addr,
    channels: u32,
    clock_identity: ClockIdentity,
    ptp_domain: u8,
) -> String {
    format!(
        "v=0\r\n\
         o=- {} {} IN IP4 {}\r\n\
         s={}\r\n\
         c=IN IP4 {}/{}\r\n\
         t=0 0\r\n\
         m=audio {} RTP/AVP {}\r\n\
         a=rtpmap:{} L24/{}/{}\r\n\
         a=ptime:{}\r\n\
         a=ts-refclk:ptp=IEEE1588-2008:{}:{}\r\n\
         a=mediaclk:direct=0\r\n\
         a=sendonly\r\n",
        stream_id.get_value(),
        revision,
        local_ip,
        stream.name,
        stream.address,
        DEFAULT_TTL,
        stream.port,
        PAYLOAD_TYPE,
        PAYLOAD_TYPE,
        SAMPLE_RATE,
        channels,
        PACKET_TIME_MS,
        clock_identity,
        ptp_domain,
    )
}

fn measure_dbfs(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (METER_FLOOR_DBFS, METER_FLOOR_DBFS);
    }
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let rms = (samples
        .iter()
        .map(|sample| (*sample as f64) * (*sample as f64))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt() as f32;
    (linear_to_dbfs(peak), linear_to_dbfs(rms))
}

fn linear_to_dbfs(value: f32) -> f32 {
    if value <= 0.0 {
        METER_FLOOR_DBFS
    } else {
        (20.0 * value.log10()).clamp(METER_FLOOR_DBFS, 0.0)
    }
}

fn random_ssrc() -> u32 {
    loop {
        let candidate = rand::random::<u32>();
        if candidate != 0 {
            return candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_values_are_finite_and_floor_silence() {
        assert_eq!(measure_dbfs(&[0.0, 0.0]), (-120.0, -120.0));
        let (peak, rms) = measure_dbfs(&[1.0, -0.5]);
        assert_eq!(peak, 0.0);
        assert!(rms.is_finite());
        assert!(rms < 0.0);
    }
}
