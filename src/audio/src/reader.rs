use std::fs::File;
use std::path::Path;
use std::time::Duration;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use rubato::Resampler;
use anyhow::Context;
use hound::{WavWriter, WavSpec, SampleFormat};

use crate::utils::{flat_noninterleaved_to_channels, channels_to_flat_noninterleaved};
use crate::Result;

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub duration: Option<Duration>,
    pub bit_depth: Option<u32>,
    pub format: String,
}

#[derive(Debug, Clone)]
pub struct AudioSample {
    /// Non-interleaved audio data: [ch1_samples..., ch2_samples..., ch3_samples...] 
    /// For stereo: [L1, L2, L3, ..., R1, R2, R3, ...]
    /// More efficient for channel-based processing
    pub data: Vec<f32>,
    pub channels: u32,
    pub sample_rate: u32,
    /// Number of frames (samples per channel)
    pub frames: usize,
}

pub struct AudioReader {
    /// Pre-loaded audio data
    audio_data: Vec<f32>,
    /// Current read position in frames
    read_position: usize,
    /// Audio file information
    info: AudioInfo,
    /// Samples per packet (e.g., 48 for 1ms at 48kHz)
    samples_per_packet: usize,
}

impl AudioReader {
    /// Create new audio reader - always targets 48kHz for AES67 compliance
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::with_resampling(path, 48000, 48)
    }
    
    /// Create new audio reader with target sample rate and packet size
    pub fn with_resampling<P: AsRef<Path>>(
        path: P, 
        target_sample_rate: u32,
        samples_per_packet: usize,
    ) -> Result<Self> {
        // Load entire audio file into memory first
        let (audio_data, channels) = Self::load_audio_file(path.as_ref(), target_sample_rate)?;
        
        // Calculate duration
        let total_frames = audio_data.len() / channels as usize;
        let duration = Duration::from_secs_f64(total_frames as f64 / target_sample_rate as f64);
        
        log::info!(
            "Audio file loaded: {} samples total, {} channels, {:.2}s duration, {} samples per packet",
            audio_data.len(),
            channels,
            duration.as_secs_f64(),
            samples_per_packet
        );

        let reader = AudioReader {
            audio_data,
            read_position: 0,
            info: AudioInfo {
                sample_rate: target_sample_rate,
                channels,
                duration: Some(duration),
                bit_depth: Some(24),
                format: "Loaded".to_string(),
            },
            samples_per_packet,
        };

        // Export resampled audio for verification (optional)
        if let Some(input_path) = path.as_ref().file_stem().and_then(|s| s.to_str()) {
            let output_path = format!("tmp/{}_resampled_48k.wav", input_path);
            if let Err(e) = reader.export_to_wav(&output_path) {
                log::warn!("Failed to export resampled audio to {}: {}", output_path, e);
            }
        }

        Ok(reader)
    }

    pub fn info(&self) -> &AudioInfo {
        &self.info
    }

    /// Load entire audio file into memory with optional resampling
    fn load_audio_file(path: &Path, target_sample_rate: u32) -> Result<(Vec<f32>, u32)> {
        let file = File::open(path)?;
        let media_source = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(extension) = path.extension() {
            if let Some(ext_str) = extension.to_str() {
                hint.with_extension(ext_str);
            }
        }

        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();
        let probed = symphonia::default::get_probe().format(
            &hint,
            media_source,
            &format_opts,
            &metadata_opts,
        )?;

        let mut format_reader = probed.format;

        // Find the default audio track
        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            .ok_or_else(|| anyhow::anyhow!("No audio tracks found"))?;

        let track_id = track.id;
        let decoder_opts = DecoderOptions::default();
        let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

        let codec_params = &track.codec_params;
        let sample_rate = codec_params.sample_rate
            .ok_or_else(|| anyhow::anyhow!("Audio file is missing sample rate"))?;
        let channels = codec_params.channels
            .map(|ch| ch.count() as u32)
            .ok_or_else(|| anyhow::anyhow!("Audio file is missing channel information"))?;

        log::info!("Loading audio file: {} Hz, {} channels", sample_rate, channels);

        // Collect all audio data properly in non-interleaved format
        let mut channel_buffers: Vec<Vec<f32>> = vec![Vec::new(); channels as usize];
        
        loop {
            let packet = match format_reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = decoder.decode(&packet)?;
            let audio_sample = Self::convert_audio_buffer(decoded)?;
            
            // The audio_sample.data is in non-interleaved format: [Ch0_frames..., Ch1_frames...]
            let frames_per_channel = audio_sample.frames;
            
            for ch_idx in 0..channels as usize {
                let start_idx = ch_idx * frames_per_channel;
                let end_idx = start_idx + frames_per_channel;
                channel_buffers[ch_idx].extend_from_slice(&audio_sample.data[start_idx..end_idx]);
            }
        }
        
        // Concatenate all channel buffers into final non-interleaved format
        let mut all_samples = Vec::new();
        for channel_buffer in &channel_buffers {
            all_samples.extend_from_slice(channel_buffer);
        }

        log::info!("Loaded {} samples from file", all_samples.len());


        // Apply resampling if needed (AES67 requires 48kHz)
        if target_sample_rate != sample_rate {
            log::info!("Resampling from {} Hz to {} Hz (AES67 compliance)", sample_rate, target_sample_rate);
            all_samples = Self::resample_audio_data_simple(all_samples, sample_rate, target_sample_rate, channels)?;
            log::info!("Resampled to {} samples", all_samples.len());
        } else {
            log::info!("No resampling needed: already at {} Hz", sample_rate);
        };

        Ok((all_samples, channels))
    }

    fn resample_audio_data_simple(
        data: Vec<f32>, 
        from_rate: u32, 
        to_rate: u32, 
        channels: u32
    ) -> Result<Vec<f32>> {
        
        let ratio = to_rate as f64 / from_rate as f64;
        let frames = data.len() / channels as usize;
        
        log::info!("Simple resampling: {} frames, ratio {:.6}", frames, ratio);
        log::info!("Input: {}Hz -> Output: {}Hz", from_rate, to_rate);
        
        // Convert to channel format for rubato
        let input_channels = flat_noninterleaved_to_channels(&data, channels as usize, frames);
        
        log::info!("Input channels: count={}, frames_per_channel={:?}", 
                   input_channels.len(), 
                   input_channels.iter().map(|ch| ch.len()).collect::<Vec<_>>());
        
        // The FFT and Sinc resamplers are having issues, use reliable linear interpolation
        log::info!("Using linear interpolation for reliable resampling...");
        Self::resample_linear_interpolation(data, from_rate, to_rate, channels)
    }
    
    /// Fallback linear interpolation resampling (simple but reliable)
    fn resample_linear_interpolation(
        data: Vec<f32>, 
        from_rate: u32, 
        to_rate: u32, 
        channels: u32
    ) -> Result<Vec<f32>> {
        let ratio = to_rate as f64 / from_rate as f64;
        let input_frames = data.len() / channels as usize;
        let output_frames = (input_frames as f64 * ratio).round() as usize;
        
        log::info!("Using linear interpolation fallback: {} -> {} frames", input_frames, output_frames);
        
        let mut output_data = Vec::with_capacity(output_frames * channels as usize);
        
        // Process each channel separately
        for ch in 0..channels as usize {
            let input_start = ch * input_frames;
            let input_end = input_start + input_frames;
            let input_channel = &data[input_start..input_end];
            
            // Linear interpolation for this channel
            for out_idx in 0..output_frames {
                let input_pos = out_idx as f64 / ratio;
                let input_idx = input_pos.floor() as usize;
                let frac = input_pos - input_idx as f64;
                
                let sample = if input_idx + 1 < input_frames {
                    // Linear interpolation between two samples
                    let s0 = input_channel[input_idx];
                    let s1 = input_channel[input_idx + 1];
                    s0 + (s1 - s0) * frac as f32
                } else if input_idx < input_frames {
                    // Use last sample
                    input_channel[input_idx]
                } else {
                    // Beyond input, use silence
                    0.0
                };
                
                output_data.push(sample);
            }
        }
        
        log::info!("Linear interpolation complete: {} output samples", output_data.len());
        Ok(output_data)
    }

    /// Export the current audio data to a WAV file for verification
    pub fn export_to_wav<P: AsRef<Path>>(&self, output_path: P) -> Result<()> {
        let path = output_path.as_ref();
        
        log::info!("Exporting audio to WAV file: {}", path.display());
        
        let spec = WavSpec {
            channels: self.info.channels as u16,
            sample_rate: self.info.sample_rate,
            bits_per_sample: 32, // Use 32-bit float
            sample_format: SampleFormat::Float,
        };
        
        let mut writer = WavWriter::create(path, spec)
            .with_context(|| format!("Failed to create WAV file: {}", path.display()))?;
        
        // Convert from non-interleaved to interleaved format for WAV
        let frames_per_channel = self.audio_data.len() / self.info.channels as usize;
        
        log::debug!("WAV export: {} frames per channel, {} channels, {} total samples", 
                   frames_per_channel, self.info.channels, self.audio_data.len());
        
        // Our data is stored as: [Ch0_all_frames..., Ch1_all_frames..., Ch2_all_frames...]
        // WAV needs: [Ch0_F0, Ch1_F0, Ch0_F1, Ch1_F1, Ch0_F2, Ch1_F2, ...]
        for frame_idx in 0..frames_per_channel {
            for ch_idx in 0..self.info.channels as usize {
                let sample_idx = ch_idx * frames_per_channel + frame_idx;
                let sample = self.audio_data[sample_idx];
                
                // Debug first few samples
                if frame_idx < 4 {
                    log::debug!("WAV sample[{}][{}] = {:.3} (idx={})", ch_idx, frame_idx, sample, sample_idx);
                }
                
                writer.write_sample(sample)
                    .with_context(|| format!("Failed to write sample at frame {}, channel {}", frame_idx, ch_idx))?;
            }
        }
        
        writer.finalize()
            .with_context(|| format!("Failed to finalize WAV file: {}", path.display()))?;
        
        log::info!("Successfully exported {} frames ({:.2}s) to WAV file", 
                   frames_per_channel, 
                   frames_per_channel as f64 / self.info.sample_rate as f64);
        
        Ok(())
    }

    /// Read exactly samples_per_packet frames (e.g., 48 samples for 1ms at 48kHz)
    pub fn read_next_frame(&mut self) -> Result<Option<AudioSample>> {
        let channels = self.info.channels as usize;
        let total_samples_needed = self.samples_per_packet * channels;
        let frames_per_channel = self.audio_data.len() / channels;
        
        // Check if we have enough frames left
        if self.read_position + self.samples_per_packet > frames_per_channel {
            return Ok(None); // End of file
        }
        
        // Extract exactly samples_per_packet frames from our buffer
        let mut packet_data = Vec::with_capacity(total_samples_needed);
        
        for ch in 0..channels {
            let channel_start = ch * frames_per_channel;
            let frame_start = channel_start + self.read_position;
            let frame_end = frame_start + self.samples_per_packet;
            
            packet_data.extend_from_slice(&self.audio_data[frame_start..frame_end]);
        }
        
        // Advance read position
        self.read_position += self.samples_per_packet;
        
        Ok(Some(AudioSample {
            data: packet_data,
            channels: self.info.channels,
            sample_rate: self.info.sample_rate,
            frames: self.samples_per_packet,
        }))
    }



    fn convert_audio_buffer(buffer: AudioBufferRef) -> Result<AudioSample> {
        let spec = *buffer.spec();
        let channels = spec.channels.count() as u32;
        let sample_rate = spec.rate;

        // Get actual frames from the first channel (all channels should have same length)
        let frames = match &buffer {
            AudioBufferRef::U8(buf) => buf.chan(0).len(),
            AudioBufferRef::U16(buf) => buf.chan(0).len(),
            AudioBufferRef::U24(buf) => buf.chan(0).len(),
            AudioBufferRef::U32(buf) => buf.chan(0).len(),
            AudioBufferRef::S8(buf) => buf.chan(0).len(),
            AudioBufferRef::S16(buf) => buf.chan(0).len(),
            AudioBufferRef::S24(buf) => buf.chan(0).len(),
            AudioBufferRef::S32(buf) => buf.chan(0).len(),
            AudioBufferRef::F32(buf) => buf.chan(0).len(),
            AudioBufferRef::F64(buf) => buf.chan(0).len(),
        };

        log::debug!("Converting audio buffer: {} channels, {} frames, {} Hz", channels, frames, sample_rate);

        // Pre-allocate non-interleaved buffer: [ch1_samples..., ch2_samples..., ch3_samples...]
        let mut noninterleaved_samples = Vec::with_capacity(frames * channels as usize);

        // Helper macro to convert to non-interleaved format
        macro_rules! convert_to_noninterleaved {
            ($buf:expr, $convert:expr) => {
                // Convert channel by channel (non-interleaved layout)
                for channel_idx in 0..channels {
                    let channel_data = $buf.chan(channel_idx as usize);
                    for frame_idx in 0..channel_data.len().min(frames) {
                        let sample = channel_data[frame_idx];
                        let normalized = $convert(sample);
                        noninterleaved_samples.push(normalized);
                    }
                }
            };
        }

        match buffer {
            AudioBufferRef::U8(buf) => {
                convert_to_noninterleaved!(buf, |sample| (sample as f32 - 128.0) / 128.0);
            }
            AudioBufferRef::U16(buf) => {
                convert_to_noninterleaved!(buf, |sample| (sample as f32 - 32768.0) / 32768.0);
            }
            AudioBufferRef::U24(buf) => {
                // Convert channel by channel (non-interleaved layout)
                for channel_idx in 0..channels {
                    for frame_idx in 0..frames {
                        let sample = buf.chan(channel_idx as usize)[frame_idx];
                        let sample_val = sample.inner();
                        let normalized = (sample_val as f32 - 8388608.0) / 8388608.0;
                        noninterleaved_samples.push(normalized);
                    }
                }
            }
            AudioBufferRef::U32(buf) => {
                convert_to_noninterleaved!(buf, |sample| (sample as f32 - 2147483648.0)
                    / 2147483648.0);
            }
            AudioBufferRef::S8(buf) => {
                convert_to_noninterleaved!(buf, |sample| sample as f32 / 128.0);
            }
            AudioBufferRef::S16(buf) => {
                convert_to_noninterleaved!(buf, |sample| sample as f32 / 32768.0);
            }
            AudioBufferRef::S24(buf) => {
                // Convert channel by channel (non-interleaved layout)
                for channel_idx in 0..channels {
                    let channel_data = buf.chan(channel_idx as usize);
                    for frame_idx in 0..channel_data.len().min(frames) {
                        let sample = channel_data[frame_idx];
                        let sample_val = sample.inner();
                        let normalized = sample_val as f32 / 8388608.0;
                        noninterleaved_samples.push(normalized);
                    }
                }
            }
            AudioBufferRef::S32(buf) => {
                convert_to_noninterleaved!(buf, |sample| sample as f32 / 2147483648.0);
            }
            AudioBufferRef::F32(buf) => {
                // F32 is already normalized, just convert to non-interleaved
                for channel_idx in 0..channels {
                    let channel_data = buf.chan(channel_idx as usize);
                    for frame_idx in 0..channel_data.len().min(frames) {
                        let sample = channel_data[frame_idx];
                        noninterleaved_samples.push(sample);
                    }
                }
            }
            AudioBufferRef::F64(buf) => {
                convert_to_noninterleaved!(buf, |sample| sample as f32);
            }
        }

        log::debug!("Converted buffer result: {} total samples, {} frames, expected total: {}", 
                   noninterleaved_samples.len(), frames, frames * channels as usize);
        
        // Verify first few samples for debugging
        if noninterleaved_samples.len() >= 4 {
            log::debug!("First few samples: [{:.3}, {:.3}, {:.3}, {:.3}]", 
                       noninterleaved_samples[0], noninterleaved_samples[1], 
                       noninterleaved_samples[2], noninterleaved_samples[3]);
        }

        Ok(AudioSample {
            data: noninterleaved_samples,
            channels,
            sample_rate,
            frames,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_info_creation() {
        let info = AudioInfo {
            sample_rate: 48000,
            channels: 2,
            duration: Some(Duration::from_secs(30)),
            bit_depth: Some(24),
            format: "PCM".to_string(),
        };

        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bit_depth, Some(24));
    }

    #[test]
    fn test_audio_sample_creation() {
        let sample = AudioSample {
            data: vec![0.0, -0.5, 0.5, 1.0], // 2 frames, 2 channels planar: [L1, L2, R1, R2]
            channels: 2,
            sample_rate: 44100,
            frames: 2,
        };

        assert_eq!(sample.data.len(), 4);
        assert_eq!(sample.channels, 2);
        assert_eq!(sample.sample_rate, 44100);
        assert_eq!(sample.frames, 2);
        assert_eq!(sample.data.len(), sample.frames * sample.channels as usize);
    }

    #[test]
    #[ignore] // Requires test file
    fn test_audio_reader_with_real_file() {
        // This test can be run with: cargo test -- --ignored
        let test_file = "../../tests/piano_freesound.wav";

        if std::path::Path::new(test_file).exists() {
            let mut reader = AudioReader::new(test_file).expect("Failed to open test file");

            let info = reader.info();
            let expected_channels = info.channels;
            let expected_sample_rate = info.sample_rate;

            assert!(expected_sample_rate > 0);
            assert!(expected_channels > 0);

            // Try to read a few frames
            let mut frame_count = 0;
            let max_frames = 3;

            while frame_count < max_frames {
                match reader.read_next_frame().expect("Failed to read frame") {
                    Some(sample) => {
                        assert!(sample.data.len() > 0);
                        assert_eq!(sample.channels, expected_channels);
                        assert_eq!(sample.sample_rate, expected_sample_rate);
                        frame_count += 1;
                    }
                    None => break,
                }
            }

            assert!(frame_count > 0, "Should have read at least one frame");
        }
    }
}
