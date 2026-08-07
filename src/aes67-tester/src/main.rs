use anyhow::{anyhow, Context, Result};
use audio::{AudioInfo, AudioSample};
use config::TesterArgs;
use network::{
    decode_l24_payload_interleaved, parse_stream_address, resolve_interface_ip, RtpPacket,
    RtpReceiveSocket, RtpReceiveSocketConfig,
};
use serde::Deserialize;
use std::array;
use std::net::Ipv4Addr;
use std::time::Duration;
use streamer_core::{Aes67Streamer, StreamAudioSource, StreamConfig};
use tokio::time::{self, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 8;
const FREQUENCY_HZ: f32 = 100.0;
const PACKET_TIME_MS: u32 = 1;
const FRAMES_PER_PACKET: usize = 48;
const TONE_PERIOD_SAMPLES: u32 = 480;
const MINIMUM_MEASUREMENT_DURATION_SECONDS: f64 = TONE_PERIOD_SAMPLES as f64 / SAMPLE_RATE as f64;

#[tokio::main]
async fn main() {
    let args = match config::parse_tester_args() {
        Ok(args) => args,
        Err(error) => {
            if config::is_display_control_error(&error) {
                print!("{error}");
                return;
            }
            eprintln!("Error parsing arguments: {error}");
            std::process::exit(1);
        }
    };

    let default_log_level = if args.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_log_level))
        .init();

    if let Err(error) = run(args).await {
        log::error!("AES67 tester failed: {error:#}");
        std::process::exit(1);
    }
}

async fn run(args: TesterArgs) -> Result<()> {
    let mut tester_config = TesterConfig::load(&args.config_file)?;
    if let Some(duration_seconds) = args.duration_seconds {
        tester_config.runtime.duration_seconds = Some(duration_seconds);
    }
    tester_config.runtime.verbose |= args.verbose;
    tester_config.validate()?;
    tester_config.warn_if_receiver_may_observe_transmitter();

    let receiver = build_receiver(&tester_config.receiver)?;
    let source = SineSource::new(tester_config.signal.amplitude);
    let stream_config = StreamConfig {
        target_sample_rate: SAMPLE_RATE,
        packet_time_ms: PACKET_TIME_MS,
        gain_db: 0.0,
        ptp_domain: tester_config.transmitter.ptp_domain,
        verbose: tester_config.runtime.verbose,
        duration: None,
        loop_playback: true,
        ttl: tester_config.transmitter.ttl,
        sap: tester_config.transmitter.sap,
        payload_type: tester_config.transmitter.payload_type,
        ssrc: tester_config.transmitter.ssrc,
        session_name: tester_config.transmitter.session_name.clone(),
    };
    let mut streamer = Aes67Streamer::new_with_audio_source(
        Box::new(source),
        &tester_config.transmitter.address,
        tester_config.transmitter.port,
        Some(&tester_config.transmitter.interface),
        stream_config,
    )
    .await
    .context("failed to create tester transmitter")?;
    let transmitter_rtp_base_timestamp = streamer.get_rtp_base_timestamp();

    log::info!(
        "AES67 tester started: 100 Hz sine, {} Hz, {} channels, {} ms packets",
        SAMPLE_RATE,
        CHANNELS,
        PACKET_TIME_MS
    );
    log::info!(
        "Transmit {}:{}; receive {}:{}",
        tester_config.transmitter.address,
        tester_config.transmitter.port,
        tester_config.receiver.address,
        tester_config.receiver.port
    );
    log::info!(
        "Latency is reported as sine phase delay modulo 10.000 ms; exact end-to-end latency requires an unambiguous marker and a shared PTP clock."
    );

    let shutdown = CancellationToken::new();
    let transmitter_shutdown = shutdown.child_token();
    let transmitter =
        tokio::spawn(async move { streamer.run_until_cancelled(transmitter_shutdown).await });

    let result = monitor_receiver(
        receiver,
        &tester_config,
        transmitter_rtp_base_timestamp,
        shutdown.clone(),
    )
    .await;
    shutdown.cancel();

    let transmitter_result = transmitter
        .await
        .map_err(|error| anyhow!("tester transmitter task failed: {error}"))?;
    transmitter_result.context("tester transmitter failed")?;
    result
}

async fn monitor_receiver(
    receiver: RtpReceiveSocket,
    config: &TesterConfig,
    transmitter_rtp_base_timestamp: u32,
    shutdown: CancellationToken,
) -> Result<()> {
    let duration = config.runtime.duration_seconds.map(Duration::from_secs_f64);
    let mut receive_buffer = vec![0_u8; 12 + FRAMES_PER_PACKET * CHANNELS as usize * 3];
    let mut monitor = StreamMonitor::new(
        config.receiver.payload_type,
        config.signal.amplitude,
        config.signal.minimum_detectable_amplitude,
        config.signal.discontinuity_multiplier,
        config.receiver.ssrc,
        transmitter_rtp_base_timestamp,
    );
    let mut report = time::interval(Duration::from_secs(config.runtime.report_interval_seconds));
    report.set_missed_tick_behavior(MissedTickBehavior::Skip);
    report.tick().await;
    let deadline = duration.map(|duration| time::Instant::now() + duration);
    let mut interrupt = Box::pin(tokio::signal::ctrl_c());

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            interrupt_result = &mut interrupt => {
                interrupt_result.context("failed to listen for Ctrl-C")?;
                log::info!("Interrupted; stopping AES67 tester");
                break;
            }
            _ = wait_until_deadline(deadline) => break,
            _ = report.tick() => monitor.log_report(false),
            received = receiver.recv_packet(&mut receive_buffer) => {
                let received = received.context("failed to receive configured tester stream")?;
                monitor.observe_packet(&received.packet)?;
            }
        }
    }

    monitor.log_report(true);
    if monitor.packets_received == 0 {
        return Err(anyhow!(
            "no RTP packets received from the configured return stream"
        ));
    }
    if monitor.has_failures() {
        return Err(anyhow!("tester detected RTP loss or audio discontinuities"));
    }
    Ok(())
}

async fn wait_until_deadline(deadline: Option<time::Instant>) {
    match deadline {
        Some(deadline) => time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}

fn build_receiver(config: &ReceiverConfig) -> Result<RtpReceiveSocket> {
    let address = parse_stream_address(&config.address).context("invalid receiver.address")?;
    let interface =
        resolve_interface_ip(&config.interface).context("invalid receiver.interface")?;
    let sender_filter = config
        .sender
        .as_deref()
        .map(str::parse::<Ipv4Addr>)
        .transpose()
        .context("invalid receiver.sender")?;
    let mut receiver_config = RtpReceiveSocketConfig::new(address, config.port, interface);
    receiver_config.sender_filter = sender_filter;
    RtpReceiveSocket::new(receiver_config).context("failed to create tester receiver")
}

#[derive(Debug, Deserialize)]
struct TesterConfig {
    transmitter: TransmitterConfig,
    receiver: ReceiverConfig,
    #[serde(default)]
    signal: SignalConfig,
    #[serde(default)]
    runtime: RuntimeConfig,
}

impl TesterConfig {
    fn load(path: &str) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read tester config {path}"))?;
        toml::from_str(&source).with_context(|| format!("failed to parse tester config {path}"))
    }

    fn validate(&self) -> Result<()> {
        self.transmitter.validate()?;
        self.receiver.validate()?;
        self.signal.validate()?;
        self.runtime.validate()?;
        Ok(())
    }

    fn warn_if_receiver_may_observe_transmitter(&self) {
        if self.transmitter.address == self.receiver.address
            && self.transmitter.port == self.receiver.port
            && self.receiver.sender.is_none()
        {
            log::warn!(
                "transmitter and receiver use the same address and port without receiver.sender; the tester can observe its own outbound packets instead of a return path"
            );
        }
    }
}

#[derive(Debug, Deserialize)]
struct TransmitterConfig {
    address: String,
    port: u16,
    interface: String,
    #[serde(default = "default_payload_type")]
    payload_type: u8,
    #[serde(default)]
    ssrc: Option<u32>,
    #[serde(default)]
    ptp_domain: u8,
    #[serde(default = "default_ttl")]
    ttl: u8,
    #[serde(default)]
    sap: bool,
    #[serde(default = "default_session_name")]
    session_name: String,
}

impl TransmitterConfig {
    fn validate(&self) -> Result<()> {
        parse_stream_address(&self.address).context("invalid transmitter.address")?;
        resolve_interface_ip(&self.interface).context("invalid transmitter.interface")?;
        validate_payload_type(self.payload_type, "transmitter.payload_type")
    }
}

#[derive(Debug, Deserialize)]
struct ReceiverConfig {
    address: String,
    port: u16,
    interface: String,
    #[serde(default = "default_payload_type")]
    payload_type: u8,
    #[serde(default)]
    sender: Option<String>,
    /// Optional expected RTP SSRC for the return stream.
    #[serde(default)]
    ssrc: Option<u32>,
}

impl ReceiverConfig {
    fn validate(&self) -> Result<()> {
        parse_stream_address(&self.address).context("invalid receiver.address")?;
        resolve_interface_ip(&self.interface).context("invalid receiver.interface")?;
        self.sender
            .as_deref()
            .map(str::parse::<Ipv4Addr>)
            .transpose()
            .context("invalid receiver.sender")?;
        validate_payload_type(self.payload_type, "receiver.payload_type")
    }
}

#[derive(Debug, Deserialize)]
struct SignalConfig {
    #[serde(default = "default_amplitude")]
    amplitude: f32,
    #[serde(default = "default_minimum_detectable_amplitude")]
    minimum_detectable_amplitude: f32,
    #[serde(default = "default_discontinuity_multiplier")]
    discontinuity_multiplier: f32,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            amplitude: default_amplitude(),
            minimum_detectable_amplitude: default_minimum_detectable_amplitude(),
            discontinuity_multiplier: default_discontinuity_multiplier(),
        }
    }
}

impl SignalConfig {
    fn validate(&self) -> Result<()> {
        if !(0.0 < self.amplitude && self.amplitude <= 1.0) {
            return Err(anyhow!(
                "signal.amplitude must be greater than zero and at most 1.0"
            ));
        }
        if !(0.0 < self.minimum_detectable_amplitude && self.minimum_detectable_amplitude <= 1.0) {
            return Err(anyhow!(
                "signal.minimum_detectable_amplitude must be greater than zero and at most 1.0"
            ));
        }
        if self.minimum_detectable_amplitude > self.amplitude {
            return Err(anyhow!(
                "signal.minimum_detectable_amplitude must not exceed signal.amplitude"
            ));
        }
        if self.discontinuity_multiplier < 1.0 {
            return Err(anyhow!(
                "signal.discontinuity_multiplier must be at least 1.0"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct RuntimeConfig {
    #[serde(default)]
    duration_seconds: Option<f64>,
    #[serde(default = "default_report_interval_seconds")]
    report_interval_seconds: u64,
    #[serde(default)]
    verbose: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            duration_seconds: None,
            report_interval_seconds: default_report_interval_seconds(),
            verbose: false,
        }
    }
}

impl RuntimeConfig {
    fn validate(&self) -> Result<()> {
        if self
            .duration_seconds
            .is_some_and(|duration| duration < MINIMUM_MEASUREMENT_DURATION_SECONDS)
        {
            return Err(anyhow!(
                "runtime.duration_seconds must be at least {:.3} seconds to measure one 100 Hz tone period",
                MINIMUM_MEASUREMENT_DURATION_SECONDS
            ));
        }
        if self.report_interval_seconds == 0 {
            return Err(anyhow!(
                "runtime.report_interval_seconds must be greater than zero"
            ));
        }
        Ok(())
    }
}

fn default_payload_type() -> u8 {
    97
}
fn default_ttl() -> u8 {
    32
}
fn default_session_name() -> String {
    "AES67 Tester 100 Hz".to_string()
}
fn default_amplitude() -> f32 {
    0.5
}
fn default_minimum_detectable_amplitude() -> f32 {
    0.05
}
fn default_discontinuity_multiplier() -> f32 {
    1.25
}
fn default_report_interval_seconds() -> u64 {
    1
}

fn validate_payload_type(payload_type: u8, field: &str) -> Result<()> {
    if !(96..=127).contains(&payload_type) {
        return Err(anyhow!(
            "{field} must be an RTP dynamic payload type (96-127)"
        ));
    }
    Ok(())
}

struct SineSource {
    info: AudioInfo,
    sample_index: u64,
    amplitude: f32,
}

impl SineSource {
    fn new(amplitude: f32) -> Self {
        Self {
            info: AudioInfo {
                sample_rate: SAMPLE_RATE,
                channels: CHANNELS as u32,
                duration: None,
                bit_depth: Some(24),
                format: "100 Hz sine".to_string(),
            },
            sample_index: 0,
            amplitude,
        }
    }

    fn sample_at(&self, frame: u64) -> f32 {
        let angle =
            2.0 * std::f64::consts::PI * FREQUENCY_HZ as f64 * frame as f64 / SAMPLE_RATE as f64;
        (angle.sin() as f32) * self.amplitude
    }
}

impl StreamAudioSource for SineSource {
    fn get_info(&self) -> &AudioInfo {
        &self.info
    }

    fn read_next_frame_into(&mut self, output: &mut AudioSample) -> Result<bool> {
        output.channels = self.info.channels;
        output.sample_rate = self.info.sample_rate;
        output.frames = FRAMES_PER_PACKET;
        output
            .data
            .resize(FRAMES_PER_PACKET * CHANNELS as usize, 0.0);

        for channel in output.data.chunks_exact_mut(FRAMES_PER_PACKET) {
            for (frame_index, sample) in channel.iter_mut().enumerate() {
                *sample = self.sample_at(self.sample_index + frame_index as u64);
            }
        }
        self.sample_index = self.sample_index.wrapping_add(FRAMES_PER_PACKET as u64);
        Ok(true)
    }

    fn rewind(&mut self) {
        self.sample_index = 0;
    }
}

struct StreamMonitor {
    payload_type: u8,
    discontinuity_threshold: f32,
    expected_ssrc: Option<u32>,
    transmitter_rtp_base_timestamp: u32,
    expected_sequence: Option<u16>,
    expected_timestamp: Option<u32>,
    previous_samples: [Option<f32>; CHANNELS as usize],
    phase: PhaseEstimator,
    packets_received: u64,
    sequence_gap_packets: u64,
    reordered_packets: u64,
    timestamp_discontinuities: u64,
    audio_discontinuities: [u64; CHANNELS as usize],
}

impl StreamMonitor {
    fn new(
        payload_type: u8,
        amplitude: f32,
        minimum_detectable_amplitude: f32,
        discontinuity_multiplier: f32,
        expected_ssrc: Option<u32>,
        transmitter_rtp_base_timestamp: u32,
    ) -> Self {
        let normal_max_step =
            2.0 * amplitude * (std::f32::consts::PI * FREQUENCY_HZ / SAMPLE_RATE as f32).sin();
        Self {
            payload_type,
            discontinuity_threshold: normal_max_step * discontinuity_multiplier,
            expected_ssrc,
            transmitter_rtp_base_timestamp,
            expected_sequence: None,
            expected_timestamp: None,
            previous_samples: array::from_fn(|_| None),
            phase: PhaseEstimator::new(minimum_detectable_amplitude),
            packets_received: 0,
            sequence_gap_packets: 0,
            reordered_packets: 0,
            timestamp_discontinuities: 0,
            audio_discontinuities: [0; CHANNELS as usize],
        }
    }

    fn observe_packet(&mut self, packet: &RtpPacket) -> Result<()> {
        if packet.header.payload_type != self.payload_type {
            return Err(anyhow!(
                "received payload type {}; expected {}",
                packet.header.payload_type,
                self.payload_type
            ));
        }
        if self
            .expected_ssrc
            .is_some_and(|expected_ssrc| packet.header.ssrc != expected_ssrc)
        {
            return Err(anyhow!(
                "received RTP SSRC 0x{:08X}; expected 0x{:08X}",
                packet.header.ssrc,
                self.expected_ssrc.expect("SSRC was checked above")
            ));
        }

        let mut samples = Vec::new();
        let frames = decode_l24_payload_interleaved(&packet.payload, CHANNELS, &mut samples)?;
        if frames != FRAMES_PER_PACKET {
            return Err(anyhow!(
                "received {frames} frames per packet; tester requires {FRAMES_PER_PACKET} frames (1 ms at 48 kHz)"
            ));
        }

        let sequence_continuous = self.observe_rtp_continuity(packet);
        if !sequence_continuous {
            self.previous_samples.fill(None);
            self.phase.reset();
        }
        self.observe_audio_continuity(&samples);
        self.phase.observe(
            packet.header.timestamp,
            &samples,
            self.transmitter_rtp_base_timestamp,
        );
        self.packets_received += 1;
        Ok(())
    }

    fn observe_rtp_continuity(&mut self, packet: &RtpPacket) -> bool {
        let Some(expected_sequence) = self.expected_sequence else {
            self.expected_sequence = Some(packet.header.sequence_number.wrapping_add(1));
            self.expected_timestamp = Some(
                packet
                    .header
                    .timestamp
                    .wrapping_add(FRAMES_PER_PACKET as u32),
            );
            return true;
        };

        let sequence_delta = packet
            .header
            .sequence_number
            .wrapping_sub(expected_sequence);
        if sequence_delta >= 0x8000 {
            self.reordered_packets += 1;
            return false;
        }

        let mut continuous = sequence_delta == 0;
        if sequence_delta != 0 {
            self.sequence_gap_packets += sequence_delta as u64;
        }
        if self
            .expected_timestamp
            .is_some_and(|expected_timestamp| packet.header.timestamp != expected_timestamp)
        {
            self.timestamp_discontinuities += 1;
            continuous = false;
        }
        self.expected_sequence = Some(packet.header.sequence_number.wrapping_add(1));
        self.expected_timestamp = Some(
            packet
                .header
                .timestamp
                .wrapping_add(FRAMES_PER_PACKET as u32),
        );
        continuous
    }

    fn observe_audio_continuity(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(CHANNELS as usize) {
            for (channel_index, sample) in frame.iter().enumerate() {
                if self.previous_samples[channel_index].is_some_and(|previous| {
                    (sample - previous).abs() > self.discontinuity_threshold
                }) {
                    self.audio_discontinuities[channel_index] += 1;
                }
                self.previous_samples[channel_index] = Some(*sample);
            }
        }
    }

    fn has_failures(&self) -> bool {
        self.sequence_gap_packets > 0
            || self.timestamp_discontinuities > 0
            || self.audio_discontinuities.iter().any(|count| *count > 0)
            || self.phase.has_signal_failures()
    }

    fn log_report(&self, final_report: bool) {
        let label = if final_report { "final" } else { "live" };
        let discontinuities = self
            .audio_discontinuities
            .iter()
            .enumerate()
            .map(|(index, count)| format!("ch{}={count}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let phase_latency = self
            .phase
            .latency_samples()
            .map(|samples| format!("{:.3} ms", samples * 1000.0 / SAMPLE_RATE as f32))
            .unwrap_or_else(|| "awaiting stable 10 ms tone window".to_string());
        let low_signal_windows = self
            .phase
            .low_signal_windows()
            .iter()
            .enumerate()
            .map(|(index, count)| format!("ch{}={count}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        log::info!(
            "Tester {label}: packets={}, sequence_gap_packets={}, reordered={}, timestamp_discontinuities={}, phase_latency_mod_10ms={}, audio_discontinuities=[{}], low_signal_windows=[{}]",
            self.packets_received,
            self.sequence_gap_packets,
            self.reordered_packets,
            self.timestamp_discontinuities,
            phase_latency,
            discontinuities,
            low_signal_windows
        );
    }
}

struct PhaseEstimator {
    first_timestamp: Option<u32>,
    frames: usize,
    sine_projections: [f64; CHANNELS as usize],
    cosine_projections: [f64; CHANNELS as usize],
    phase_latency_samples: Option<f32>,
    minimum_detectable_amplitude: f32,
    completed_windows: u64,
    low_signal_windows: [u64; CHANNELS as usize],
}

impl PhaseEstimator {
    fn new(minimum_detectable_amplitude: f32) -> Self {
        Self {
            first_timestamp: None,
            frames: 0,
            sine_projections: [0.0; CHANNELS as usize],
            cosine_projections: [0.0; CHANNELS as usize],
            phase_latency_samples: None,
            minimum_detectable_amplitude,
            completed_windows: 0,
            low_signal_windows: [0; CHANNELS as usize],
        }
    }

    fn observe(&mut self, timestamp: u32, samples: &[f32], transmitter_rtp_base_timestamp: u32) {
        let first_timestamp = *self.first_timestamp.get_or_insert(timestamp);
        let expected_timestamp = first_timestamp.wrapping_add(self.frames as u32);
        if timestamp != expected_timestamp {
            self.reset();
            self.first_timestamp = Some(timestamp);
        }

        for frame in samples.chunks_exact(CHANNELS as usize) {
            let angle = 2.0 * std::f64::consts::PI * FREQUENCY_HZ as f64 * self.frames as f64
                / SAMPLE_RATE as f64;
            for (channel, sample) in frame.iter().enumerate() {
                self.sine_projections[channel] += *sample as f64 * angle.sin();
                self.cosine_projections[channel] += *sample as f64 * angle.cos();
            }
            self.frames += 1;
        }

        if self.frames >= TONE_PERIOD_SAMPLES as usize {
            let amplitudes: [f64; CHANNELS as usize] = array::from_fn(|channel| {
                2.0 * self.sine_projections[channel].hypot(self.cosine_projections[channel])
                    / self.frames as f64
            });
            for (channel, amplitude) in amplitudes.iter().enumerate() {
                if *amplitude < self.minimum_detectable_amplitude as f64 {
                    self.low_signal_windows[channel] += 1;
                }
            }

            if amplitudes[0] >= self.minimum_detectable_amplitude as f64 {
                let phase = self.cosine_projections[0]
                    .atan2(self.sine_projections[0])
                    .rem_euclid(2.0 * std::f64::consts::PI);
                let source_phase_samples = (phase * SAMPLE_RATE as f64
                    / (2.0 * std::f64::consts::PI * FREQUENCY_HZ as f64))
                    as f32;
                let timestamp_phase = first_timestamp.wrapping_sub(transmitter_rtp_base_timestamp)
                    % TONE_PERIOD_SAMPLES;
                self.phase_latency_samples = Some(
                    (timestamp_phase as f32 - source_phase_samples)
                        .rem_euclid(TONE_PERIOD_SAMPLES as f32),
                );
            }
            self.frames = 0;
            self.sine_projections = [0.0; CHANNELS as usize];
            self.cosine_projections = [0.0; CHANNELS as usize];
            self.first_timestamp = Some(timestamp.wrapping_add(FRAMES_PER_PACKET as u32));
            self.completed_windows += 1;
        }
    }

    fn latency_samples(&self) -> Option<f32> {
        self.phase_latency_samples
    }

    fn low_signal_windows(&self) -> &[u64; CHANNELS as usize] {
        &self.low_signal_windows
    }

    fn has_signal_failures(&self) -> bool {
        self.completed_windows == 0 || self.low_signal_windows.iter().any(|count| *count > 0)
    }

    fn reset(&mut self) {
        self.first_timestamp = None;
        self.frames = 0;
        self.sine_projections = [0.0; CHANNELS as usize];
        self.cosine_projections = [0.0; CHANNELS as usize];
        self.phase_latency_samples = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use network::RtpHeader;

    fn monitor(transmitter_rtp_base_timestamp: u32) -> StreamMonitor {
        StreamMonitor::new(97, 0.5, 0.05, 1.25, None, transmitter_rtp_base_timestamp)
    }

    #[test]
    fn sine_source_fills_all_eight_channels_with_the_same_continuous_tone() {
        let mut source = SineSource::new(0.5);
        let mut first = AudioSample {
            data: Vec::new(),
            channels: 0,
            sample_rate: 0,
            frames: 0,
        };
        let mut second = first.clone();
        source.read_next_frame_into(&mut first).unwrap();
        source.read_next_frame_into(&mut second).unwrap();

        assert_eq!(first.frames, FRAMES_PER_PACKET);
        assert_eq!(first.channels, CHANNELS as u32);
        assert_eq!(first.data.len(), FRAMES_PER_PACKET * CHANNELS as usize);
        for channel in first.data.chunks_exact(FRAMES_PER_PACKET) {
            assert_eq!(channel, &first.data[..FRAMES_PER_PACKET]);
        }
        assert!((second.data[0] - source.sample_at(FRAMES_PER_PACKET as u64)).abs() < 1e-6);
    }

    #[test]
    fn monitor_counts_packet_loss_and_resets_audio_boundary_check() {
        let mut monitor = monitor(0);
        monitor.observe_packet(&tone_packet(0, 0, 0)).unwrap();
        monitor.observe_packet(&tone_packet(2, 96, 96)).unwrap();

        assert_eq!(monitor.sequence_gap_packets, 1);
        assert_eq!(monitor.timestamp_discontinuities, 1);
        assert!(monitor
            .audio_discontinuities
            .iter()
            .all(|count| *count == 0));
    }

    #[test]
    fn monitor_detects_unexpected_audio_step() {
        let mut monitor = monitor(0);
        let mut packet = tone_packet(0, 0, 0);
        packet.payload = [0x7f, 0xff, 0xff].repeat(FRAMES_PER_PACKET * CHANNELS as usize);
        monitor.observe_packet(&packet).unwrap();
        assert!(monitor
            .audio_discontinuities
            .iter()
            .all(|count| *count == 0));

        let mut next = tone_packet(1, 48, 48);
        next.payload = [0x80, 0x00, 0x00].repeat(FRAMES_PER_PACKET * CHANNELS as usize);
        monitor.observe_packet(&next).unwrap();
        assert!(monitor.audio_discontinuities.iter().all(|count| *count > 0));
    }

    #[test]
    fn phase_estimator_reports_known_delay_modulo_one_tone_period() {
        let mut monitor = monitor(1_000);
        let delay = 137_u32;
        for packet_index in 0..10 {
            let source_frame = packet_index * FRAMES_PER_PACKET;
            monitor
                .observe_packet(&tone_packet(
                    packet_index as u16,
                    1_000 + delay + source_frame as u32,
                    source_frame,
                ))
                .unwrap();
        }
        let measured = monitor.phase.latency_samples().unwrap();
        assert!((measured - delay as f32).abs() < 0.1, "measured {measured}");
    }

    #[test]
    fn monitor_reports_a_sequence_gap_once_when_a_packet_arrives_late() {
        let mut monitor = monitor(0);
        monitor.observe_packet(&tone_packet(0, 0, 0)).unwrap();
        monitor.observe_packet(&tone_packet(2, 96, 96)).unwrap();
        monitor.observe_packet(&tone_packet(1, 48, 48)).unwrap();
        monitor.observe_packet(&tone_packet(3, 144, 144)).unwrap();

        assert_eq!(monitor.sequence_gap_packets, 1);
        assert_eq!(monitor.reordered_packets, 1);
        assert_eq!(monitor.timestamp_discontinuities, 1);
    }

    #[test]
    fn monitor_requires_a_detectable_tone_on_every_channel() {
        let mut monitor = monitor(0);
        for packet_index in 0..10 {
            let mut packet = tone_packet(
                packet_index,
                packet_index as u32 * FRAMES_PER_PACKET as u32,
                packet_index as usize * FRAMES_PER_PACKET,
            );
            packet.payload.fill(0);
            monitor.observe_packet(&packet).unwrap();
        }

        assert!(monitor
            .phase
            .low_signal_windows()
            .iter()
            .all(|count| *count == 1));
        assert!(monitor.phase.latency_samples().is_none());
        assert!(monitor.has_failures());
    }

    #[test]
    fn monitor_rejects_unexpected_ssrc() {
        let mut monitor = StreamMonitor::new(97, 0.5, 0.05, 1.25, Some(2), 0);
        let error = monitor.observe_packet(&tone_packet(0, 0, 0)).unwrap_err();

        assert!(error.to_string().contains("expected 0x00000002"));
    }

    #[tokio::test]
    async fn monitor_stops_at_duration_when_no_packets_arrive() {
        let receiver = RtpReceiveSocket::new(RtpReceiveSocketConfig::new(
            Ipv4Addr::LOCALHOST,
            0,
            Ipv4Addr::LOCALHOST,
        ))
        .unwrap();
        let config = tester_config_for_test(Some(0.02));
        let started = time::Instant::now();

        let error = monitor_receiver(receiver, &config, 0, CancellationToken::new())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("no RTP packets received"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn tester_config_rejects_non_positive_duration() {
        let config: TesterConfig = toml::from_str(
            r#"
                [transmitter]
                address = "127.0.0.1"
                port = 5004
                interface = "127.0.0.1"

                [receiver]
                address = "127.0.0.1"
                port = 5005
                interface = "127.0.0.1"

                [runtime]
                duration_seconds = 0
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn tester_config_rejects_too_short_measurement_duration() {
        let mut config = tester_config_for_test(Some(0.005));

        let error = config.validate().unwrap_err();

        assert!(error.to_string().contains("at least 0.010 seconds"));
        config.runtime.duration_seconds = Some(0.01);
        assert!(config.validate().is_ok());
    }

    fn tester_config_for_test(duration_seconds: Option<f64>) -> TesterConfig {
        TesterConfig {
            transmitter: TransmitterConfig {
                address: "127.0.0.1".to_string(),
                port: 5004,
                interface: "127.0.0.1".to_string(),
                payload_type: 97,
                ssrc: None,
                ptp_domain: 0,
                ttl: 32,
                sap: false,
                session_name: default_session_name(),
            },
            receiver: ReceiverConfig {
                address: "127.0.0.1".to_string(),
                port: 5005,
                interface: "127.0.0.1".to_string(),
                payload_type: 97,
                sender: None,
                ssrc: None,
            },
            signal: SignalConfig::default(),
            runtime: RuntimeConfig {
                duration_seconds,
                report_interval_seconds: 1,
                verbose: false,
            },
        }
    }

    fn tone_packet(sequence_number: u16, timestamp: u32, source_frame: usize) -> RtpPacket {
        let mut payload = Vec::with_capacity(FRAMES_PER_PACKET * CHANNELS as usize * 3);
        for frame in 0..FRAMES_PER_PACKET {
            let sample =
                (2.0 * std::f32::consts::PI * FREQUENCY_HZ * (source_frame + frame) as f32
                    / SAMPLE_RATE as f32)
                    .sin()
                    * 0.5;
            let encoded = encode_l24(sample);
            for _ in 0..CHANNELS {
                payload.extend_from_slice(&encoded);
            }
        }
        RtpPacket {
            header: RtpHeader {
                version: 2,
                padding: false,
                extension: false,
                csrc_count: 0,
                marker: false,
                payload_type: 97,
                sequence_number,
                timestamp,
                ssrc: 1,
            },
            payload,
        }
    }

    fn encode_l24(sample: f32) -> [u8; 3] {
        let value = (sample * 8_388_607.0) as i32;
        let bytes = value.to_be_bytes();
        [bytes[1], bytes[2], bytes[3]]
    }
}
