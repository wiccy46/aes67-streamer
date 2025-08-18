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

        Ok(AudioReader {
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
        })
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

        // Collect all audio data
        let mut all_samples = Vec::new();
        
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
            all_samples.extend_from_slice(&audio_sample.data);
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
        use rubato::{SincFixedIn, WindowFunction, SincInterpolationType, SincInterpolationParameters};
        
        let ratio = to_rate as f64 / from_rate as f64;
        let frames = data.len() / channels as usize;
        
        log::info!("Simple resampling: {} frames, ratio {:.3}", frames, ratio);
        
        // Convert to channel format for rubato
        let input_channels = flat_noninterleaved_to_channels(&data, channels as usize, frames);
        
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        
        // Use SincFixedIn which is more flexible with input buffer sizes
        let chunk_size = 1024.min(frames); // Process in chunks, but handle small files
        let mut resampler = SincFixedIn::<f32>::new(
            ratio,
            2.0,
            params,
            chunk_size,
            channels as usize,
        ).context("Failed to create simple resampler")?;

        log::info!("Created resampler: ratio={:.3}, chunk_size={}, channels={}", ratio, chunk_size, channels);
        log::info!("Input channels: count={}, frames_per_channel={:?}", 
                   input_channels.len(), 
                   input_channels.iter().map(|ch| ch.len()).collect::<Vec<_>>());
        
        // Process all data in chunks
        let mut output_channels: Vec<Vec<f32>> = vec![Vec::new(); channels as usize];
        let mut input_pos = 0;
        
        while input_pos < frames {
            let chunk_frames = (frames - input_pos).min(chunk_size);
            let mut chunk_channels: Vec<Vec<f32>> = Vec::new();
            
            // Extract chunk for each channel
            for ch_idx in 0..channels as usize {
                let mut chunk = input_channels[ch_idx][input_pos..input_pos + chunk_frames].to_vec();
                
                // Pad chunk to chunk_size if it's the last partial chunk
                if chunk.len() < chunk_size {
                    log::debug!("Padding final chunk from {} to {} frames", chunk.len(), chunk_size);
                    chunk.resize(chunk_size, 0.0);
                }
                
                chunk_channels.push(chunk);
            }
            
            log::debug!("Processing chunk: frames={}, position={}, padded_size={}", 
                       chunk_frames, input_pos, chunk_channels[0].len());
            
            // Process chunk
            let chunk_output = match resampler.process(&chunk_channels, None) {
                Ok(output) => {
                    log::debug!("Chunk processed successfully, output frames: {}", output[0].len());
                    output
                },
                Err(e) => {
                    log::error!("Chunk processing failed at position {}: {:?}", input_pos, e);
                    return Err(anyhow::anyhow!("Chunk resampling failed: {:?}", e));
                }
            };
            
            // For the final chunk, we may need to trim padding-induced extra output
            if input_pos + chunk_frames >= frames {
                // This is the final chunk - calculate how much of the output is valid
                let ratio = to_rate as f64 / from_rate as f64;
                let expected_output_frames = (chunk_frames as f64 * ratio).round() as usize;
                
                log::debug!("Final chunk: trimming output from {} to {} frames", 
                           chunk_output[0].len(), expected_output_frames);
                
                // Append only the valid portion of the final chunk output
                for (ch_idx, channel_output) in chunk_output.iter().enumerate() {
                    let valid_output = &channel_output[..expected_output_frames.min(channel_output.len())];
                    output_channels[ch_idx].extend_from_slice(valid_output);
                }
            } else {
                // Append full chunk output for non-final chunks
                for (ch_idx, channel_output) in chunk_output.iter().enumerate() {
                    output_channels[ch_idx].extend_from_slice(channel_output);
                }
            }
            
            input_pos += chunk_frames;
        }

        log::info!("Finished resampling successfully");
        log::info!("Simple resampling complete: {} output frames", output_channels[0].len());
        
        Ok(channels_to_flat_noninterleaved(&output_channels))
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
        let frames = buffer.capacity();
        let sample_rate = spec.rate;

        // Pre-allocate non-interleaved buffer: [ch1_samples..., ch2_samples..., ch3_samples...]
        let mut noninterleaved_samples = Vec::with_capacity(frames * channels as usize);

        // Helper macro to convert to non-interleaved format
        macro_rules! convert_to_noninterleaved {
            ($buf:expr, $convert:expr) => {
                // Convert channel by channel (non-interleaved layout)
                for channel_idx in 0..channels {
                    for frame_idx in 0..frames {
                        let sample = $buf.chan(channel_idx as usize)[frame_idx];
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
                    for frame_idx in 0..frames {
                        let sample = buf.chan(channel_idx as usize)[frame_idx];
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
                    for frame_idx in 0..frames.min(channel_data.len()) {
                        let sample = channel_data[frame_idx];
                        noninterleaved_samples.push(sample);
                    }
                }
            }
            AudioBufferRef::F64(buf) => {
                convert_to_noninterleaved!(buf, |sample| sample as f32);
            }
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
