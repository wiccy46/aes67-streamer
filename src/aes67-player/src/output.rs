use anyhow::{anyhow, Result};
use config::PlayerOutput;

#[cfg(feature = "cpal-output")]
use cpal_backend::CpalOutput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Null,
    Cpal,
}

impl From<PlayerOutput> for OutputMode {
    fn from(value: PlayerOutput) -> Self {
        match value {
            PlayerOutput::Null => Self::Null,
            PlayerOutput::Cpal => Self::Cpal,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutputStats {
    pub frames_written: u64,
    pub samples_written: u64,
    pub silence_frames: u64,
    pub dropped_samples: u64,
}

pub trait AudioOutput {
    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn write_interleaved(&mut self, samples: &[f32], channels: u16) -> Result<usize>;
    fn write_silence(&mut self, frames: u32, channels: u16) -> Result<()>;
    fn stats(&self) -> OutputStats;
}

pub fn build_output(
    mode: OutputMode,
    sample_rate: u32,
    channels: u16,
    latency_ms: u32,
    output_device: Option<&str>,
) -> Result<Box<dyn AudioOutput + Send>> {
    #[cfg(not(feature = "cpal-output"))]
    let _ = (sample_rate, channels, latency_ms, output_device);

    match mode {
        OutputMode::Null => Ok(Box::new(NullOutput::default())),
        #[cfg(feature = "cpal-output")]
        OutputMode::Cpal => Ok(Box::new(CpalOutput::new(
            sample_rate,
            channels,
            latency_ms,
            output_device,
        )?)),
        #[cfg(not(feature = "cpal-output"))]
        OutputMode::Cpal => Err(anyhow!(
            "CPAL output is not enabled in this build; use --output null or rebuild with --features cpal-output"
        )),
    }
}

#[cfg(feature = "cpal-output")]
pub fn list_output_devices() -> Result<String> {
    cpal_backend::list_output_devices()
}

#[cfg(not(feature = "cpal-output"))]
pub fn list_output_devices() -> Result<String> {
    Err(anyhow!(
        "CPAL output is not enabled in this build; rebuild with --features cpal-output to list audio devices"
    ))
}

#[derive(Debug, Default)]
pub struct NullOutput {
    stats: OutputStats,
}

impl AudioOutput for NullOutput {
    fn write_interleaved(&mut self, samples: &[f32], channels: u16) -> Result<usize> {
        if channels == 0 {
            return Err(anyhow!("channel count must be greater than zero"));
        }
        if samples.len() % channels as usize != 0 {
            return Err(anyhow!(
                "interleaved sample count {} is not divisible by channel count {channels}",
                samples.len()
            ));
        }

        let frames = samples.len() / channels as usize;
        self.stats.frames_written += frames as u64;
        self.stats.samples_written += samples.len() as u64;
        Ok(frames)
    }

    fn write_silence(&mut self, frames: u32, channels: u16) -> Result<()> {
        if channels == 0 {
            return Err(anyhow!("channel count must be greater than zero"));
        }

        self.stats.frames_written += frames as u64;
        self.stats.samples_written += frames as u64 * channels as u64;
        self.stats.silence_frames += frames as u64;
        Ok(())
    }

    fn stats(&self) -> OutputStats {
        self.stats
    }
}

#[cfg(feature = "cpal-output")]
mod cpal_backend {
    use super::{output_buffer_capacity_samples, AudioOutput, OutputStats};
    use anyhow::{anyhow, Context, Result};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
    use ringbuf::{traits::*, HeapCons, HeapProd, HeapRb};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    pub(super) fn list_output_devices() -> Result<String> {
        let host = cpal::default_host();
        let mut output = String::from("Audio output devices:\n");
        let mut found = false;

        for (index, device) in host
            .output_devices()
            .context("failed to query output devices")?
            .enumerate()
        {
            found = true;
            let name = device_name(&device);
            output.push_str(&format!("  [{index}] {name}\n"));

            match device.supported_output_configs() {
                Ok(configs) => {
                    for config in configs {
                        output.push_str(&format!(
                            "      {}ch {:?} {}-{} Hz\n",
                            config.channels(),
                            config.sample_format(),
                            config.min_sample_rate(),
                            config.max_sample_rate()
                        ));
                    }
                }
                Err(error) => {
                    output.push_str(&format!(
                        "      failed to query supported configs: {error}\n"
                    ));
                }
            }
        }

        if !found {
            output.push_str("  no output devices found\n");
        }

        Ok(output)
    }

    pub(super) struct CpalOutput {
        producer: HeapProd<f32>,
        stream: cpal::Stream,
        stats: Arc<CpalStats>,
        channels: u16,
        started: bool,
    }

    impl CpalOutput {
        pub(super) fn new(
            sample_rate: u32,
            channels: u16,
            latency_ms: u32,
            output_device: Option<&str>,
        ) -> Result<Self> {
            if sample_rate == 0 {
                return Err(anyhow!("sample rate must be greater than zero"));
            }
            if channels == 0 {
                return Err(anyhow!("channel count must be greater than zero"));
            }

            let host = cpal::default_host();
            let device = select_output_device(&host, output_device)?;
            let device_name = device_name(&device);
            let supported_config = select_output_config(&device, sample_rate, channels)
                .with_context(|| format!("failed to select output config for {device_name}"))?;
            let sample_format = supported_config.sample_format();
            let stream_config: StreamConfig = supported_config.into();
            let capacity = output_buffer_capacity_samples(sample_rate, channels, latency_ms);
            let ring = HeapRb::<f32>::new(capacity);
            let (producer, consumer) = ring.split();
            let stats = Arc::new(CpalStats::default());
            let stream = build_stream_for_format(
                &device,
                &stream_config,
                sample_format,
                consumer,
                stats.clone(),
            )?;

            log::info!(
                "Created CPAL output on '{}' at {} Hz, {} channels, {:?}, {} sample buffer",
                device_name,
                stream_config.sample_rate,
                stream_config.channels,
                sample_format,
                capacity
            );

            Ok(Self {
                producer,
                stream,
                stats,
                channels,
                started: false,
            })
        }
    }

    fn select_output_device(
        host: &cpal::Host,
        output_device: Option<&str>,
    ) -> Result<cpal::Device> {
        let Some(selector) = output_device
            .map(str::trim)
            .filter(|selector| !selector.is_empty())
        else {
            return host
                .default_output_device()
                .ok_or_else(|| anyhow!("no default output device is available"));
        };

        let devices = host
            .output_devices()
            .context("failed to query output devices")?
            .enumerate()
            .map(|(index, device)| {
                let name = device_name(&device);
                (index, name, device)
            })
            .collect::<Vec<_>>();

        if let Ok(index) = selector.parse::<usize>() {
            return devices
                .into_iter()
                .find_map(|(device_index, _, device)| (device_index == index).then_some(device))
                .ok_or_else(|| anyhow!("no output device found at index {index}"));
        }

        let exact_match = devices
            .iter()
            .find(|(_, name, _)| name == selector)
            .map(|(_, _, device)| device.clone());
        if let Some(device) = exact_match {
            return Ok(device);
        }

        let selector_lower = selector.to_lowercase();
        let mut partial_matches = devices
            .iter()
            .filter(|(_, name, _)| name.to_lowercase().contains(&selector_lower))
            .map(|(_, name, device)| (name.clone(), device.clone()))
            .collect::<Vec<_>>();

        match partial_matches.len() {
            1 => Ok(partial_matches.remove(0).1),
            0 => Err(anyhow!("no output device found matching '{selector}'")),
            _ => Err(anyhow!(
                "output device selector '{selector}' is ambiguous: {}",
                partial_matches
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    fn device_name(device: &cpal::Device) -> String {
        device
            .description()
            .map(|description| description.name().to_string())
            .unwrap_or_else(|_| "unknown output device".to_string())
    }

    impl AudioOutput for CpalOutput {
        fn start(&mut self) -> Result<()> {
            if !self.started {
                self.stream
                    .play()
                    .context("failed to start CPAL output stream")?;
                self.started = true;
            }

            Ok(())
        }

        fn write_interleaved(&mut self, samples: &[f32], channels: u16) -> Result<usize> {
            if channels != self.channels {
                return Err(anyhow!(
                    "output channel count changed from {} to {channels}",
                    self.channels
                ));
            }
            if samples.len() % channels as usize != 0 {
                return Err(anyhow!(
                    "interleaved sample count {} is not divisible by channel count {channels}",
                    samples.len()
                ));
            }

            let mut dropped = 0u64;
            for sample in samples {
                if self.producer.try_push(*sample).is_err() {
                    dropped += 1;
                }
            }

            if dropped > 0 {
                self.stats
                    .dropped_samples
                    .fetch_add(dropped, Ordering::Relaxed);
            }

            Ok(samples.len() / channels as usize)
        }

        fn write_silence(&mut self, frames: u32, channels: u16) -> Result<()> {
            if channels != self.channels {
                return Err(anyhow!(
                    "output channel count changed from {} to {channels}",
                    self.channels
                ));
            }

            let samples = frames as usize * channels as usize;
            let mut dropped = 0u64;
            for _ in 0..samples {
                if self.producer.try_push(0.0).is_err() {
                    dropped += 1;
                }
            }

            if dropped > 0 {
                self.stats
                    .dropped_samples
                    .fetch_add(dropped, Ordering::Relaxed);
            }

            Ok(())
        }

        fn stats(&self) -> OutputStats {
            self.stats.snapshot()
        }
    }

    #[derive(Debug, Default)]
    struct CpalStats {
        frames_written: AtomicU64,
        samples_written: AtomicU64,
        silence_frames: AtomicU64,
        dropped_samples: AtomicU64,
    }

    impl CpalStats {
        fn snapshot(&self) -> OutputStats {
            OutputStats {
                frames_written: self.frames_written.load(Ordering::Relaxed),
                samples_written: self.samples_written.load(Ordering::Relaxed),
                silence_frames: self.silence_frames.load(Ordering::Relaxed),
                dropped_samples: self.dropped_samples.load(Ordering::Relaxed),
            }
        }
    }

    fn select_output_config(
        device: &cpal::Device,
        sample_rate: u32,
        channels: u16,
    ) -> Result<cpal::SupportedStreamConfig> {
        let requested_rate = sample_rate;
        let mut selected = None;

        for config in device
            .supported_output_configs()
            .context("failed to query supported output configs")?
        {
            if config.channels() != channels
                || config.min_sample_rate() > requested_rate
                || config.max_sample_rate() < requested_rate
            {
                continue;
            }

            let Some(rank) = sample_format_rank(config.sample_format()) else {
                continue;
            };
            let replace = selected
                .as_ref()
                .is_none_or(|(_, selected_rank)| rank < *selected_rank);
            if replace {
                selected = Some((config.with_sample_rate(requested_rate), rank));
            }
        }

        selected.map(|(config, _)| config).ok_or_else(|| {
            anyhow!("default output device does not support {sample_rate} Hz/{channels}ch")
        })
    }

    fn sample_format_rank(format: SampleFormat) -> Option<u8> {
        match format {
            SampleFormat::F32 => Some(0),
            SampleFormat::I16 => Some(1),
            SampleFormat::U16 => Some(2),
            SampleFormat::F64 => Some(3),
            SampleFormat::I32 => Some(4),
            SampleFormat::U32 => Some(5),
            _ => None,
        }
    }

    fn build_stream_for_format(
        device: &cpal::Device,
        config: &StreamConfig,
        sample_format: SampleFormat,
        consumer: HeapCons<f32>,
        stats: Arc<CpalStats>,
    ) -> Result<cpal::Stream> {
        match sample_format {
            SampleFormat::F32 => build_stream::<f32>(device, config, consumer, stats),
            SampleFormat::I16 => build_stream::<i16>(device, config, consumer, stats),
            SampleFormat::U16 => build_stream::<u16>(device, config, consumer, stats),
            SampleFormat::F64 => build_stream::<f64>(device, config, consumer, stats),
            SampleFormat::I32 => build_stream::<i32>(device, config, consumer, stats),
            SampleFormat::U32 => build_stream::<u32>(device, config, consumer, stats),
            other => Err(anyhow!("unsupported CPAL sample format {other:?}")),
        }
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &StreamConfig,
        mut consumer: HeapCons<f32>,
        stats: Arc<CpalStats>,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = config.channels as usize;
        let callback_stats = stats.clone();
        let err_fn = |err| log::warn!("CPAL output stream error: {err}");

        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    write_output_callback(data, channels, &mut consumer, &callback_stats)
                },
                err_fn,
                None,
            )
            .context("failed to build CPAL output stream")
    }

    fn write_output_callback<T>(
        data: &mut [T],
        channels: usize,
        consumer: &mut HeapCons<f32>,
        stats: &CpalStats,
    ) where
        T: SizedSample + FromSample<f32>,
    {
        let mut silence_samples = 0usize;

        for sample in data.iter_mut() {
            let value = match consumer.try_pop() {
                Some(value) => value,
                None => {
                    silence_samples += 1;
                    0.0
                }
            };
            *sample = T::from_sample_(value);
        }

        stats
            .frames_written
            .fetch_add((data.len() / channels) as u64, Ordering::Relaxed);
        stats
            .samples_written
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        stats
            .silence_frames
            .fetch_add((silence_samples / channels) as u64, Ordering::Relaxed);
    }
}

#[cfg(any(feature = "cpal-output", test))]
fn output_buffer_capacity_samples(sample_rate: u32, channels: u16, latency_ms: u32) -> usize {
    let buffer_ms = latency_ms.max(125) * 4;
    ((sample_rate as u64 * channels as u64 * buffer_ms as u64) / 1000) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_output_counts_frames_and_silence() {
        let mut output = NullOutput::default();

        assert_eq!(
            output.write_interleaved(&[0.0, 0.1, 0.2, 0.3], 2).unwrap(),
            2
        );
        output.write_silence(48, 2).unwrap();

        let stats = output.stats();
        assert_eq!(stats.frames_written, 50);
        assert_eq!(stats.samples_written, 100);
        assert_eq!(stats.silence_frames, 48);
        assert_eq!(stats.dropped_samples, 0);
    }

    #[test]
    fn null_output_rejects_invalid_interleaved_shape() {
        let mut output = NullOutput::default();

        assert!(output.write_interleaved(&[0.0, 0.1, 0.2], 2).is_err());
        assert!(output.write_interleaved(&[0.0], 0).is_err());
        assert!(output.write_silence(48, 0).is_err());
    }

    #[test]
    fn output_buffer_capacity_has_large_internal_minimum() {
        assert_eq!(output_buffer_capacity_samples(48_000, 2, 50), 48_000);
        assert_eq!(output_buffer_capacity_samples(48_000, 2, 250), 96_000);
    }
}
