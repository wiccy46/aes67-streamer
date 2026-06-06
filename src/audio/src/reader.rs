use std::fs::File;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use hound::{SampleFormat, WavSpec, WavWriter};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::utils::flat_noninterleaved_to_channels;
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

        // Export resampled audio for verification (optional) - only in debug mode
        #[cfg(debug_assertions)]
        if let Some(input_path) = path.as_ref().file_stem().and_then(|s| s.to_str()) {
            if let Ok(()) = std::fs::create_dir_all("tmp") {
                let output_path = format!("tmp/{}_resampled_48k.wav", input_path);
                if let Err(e) = reader.export_to_wav(&output_path) {
                    log::warn!("Failed to export resampled audio to {}: {}", output_path, e);
                }
            }
        }

        Ok(reader)
    }

    pub fn get_info(&self) -> &AudioInfo {
        &self.info
    }

    pub fn rewind(&mut self) {
        self.read_position = 0;
    }

    pub fn can_read_full_packet(&self) -> bool {
        let channels = self.info.channels as usize;
        let frames_per_channel = self.audio_data.len() / channels;
        frames_per_channel >= self.samples_per_packet
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
        let mut decoder =
            symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

        let codec_params = &track.codec_params;
        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| anyhow::anyhow!("Audio file is missing sample rate"))?;
        let channels = codec_params
            .channels
            .map(|ch| ch.count() as u32)
            .ok_or_else(|| anyhow::anyhow!("Audio file is missing channel information"))?;

        log::info!(
            "Loading audio file: {} Hz, {} channels",
            sample_rate,
            channels
        );

        // Collect all audio data properly in non-interleaved format
        let mut channel_buffers: Vec<Vec<f32>> = vec![Vec::new(); channels as usize];

        loop {
            let packet = match format_reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break
                }
                Err(e) => return Err(e.into()),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = decoder.decode(&packet)?;
            let audio_sample = Self::convert_audio_buffer(decoded)?;

            // The audio_sample.data is in non-interleaved format: [Ch0_frames..., Ch1_frames...]
            let frames_per_channel = audio_sample.frames;

            for (ch_idx, channel_buffer) in channel_buffers.iter_mut().enumerate() {
                let start_idx = ch_idx * frames_per_channel;
                let end_idx = start_idx + frames_per_channel;
                channel_buffer.extend_from_slice(&audio_sample.data[start_idx..end_idx]);
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
            log::info!(
                "Resampling from {} Hz to {} Hz (AES67 compliance)",
                sample_rate,
                target_sample_rate
            );
            all_samples = Self::resample_audio_data_simple(
                all_samples,
                sample_rate,
                target_sample_rate,
                channels,
            )?;
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
        channels: u32,
    ) -> Result<Vec<f32>> {
        use rubato::{FastFixedIn, PolynomialDegree, Resampler};

        let ratio = to_rate as f64 / from_rate as f64;
        let frames = data.len() / channels as usize;
        let expected_output_frames = (frames as f64 * ratio).round() as usize;

        log::info!(
            "High-quality resampling with Rubato: {} frames, ratio {:.6}",
            frames,
            ratio
        );
        log::info!("Input: {}Hz -> Output: {}Hz", from_rate, to_rate);

        // Convert to channel format for rubato
        let input_channels = flat_noninterleaved_to_channels(&data, channels as usize, frames);

        // Create resampler
        // FastFixedIn is good for synchronous resampling
        let mut resampler = FastFixedIn::<f32>::new(
            ratio,
            1.0,                      // Max relative ratio difference
            PolynomialDegree::Septic, // High quality
            1024,                     // Chunk size
            channels as usize,
        )?;

        // Rubato requires input to be chunks of specific size, but FastFixedIn can handle arbitrary input
        // if we process it correctly or use the `process` method which handles buffering?
        // Actually FastFixedIn requires fixed input size.
        // Let's use `process` with chunks.

        let mut output_channels = vec![Vec::new(); channels as usize];
        let chunk_size = resampler.input_frames_next();

        // Process in chunks
        let mut input_pos = 0;
        while input_pos < frames {
            let end = (input_pos + chunk_size).min(frames);
            let len = end - input_pos;

            let mut chunk_input = vec![Vec::with_capacity(chunk_size); channels as usize];

            for (ch_idx, ch_data) in input_channels.iter().enumerate() {
                chunk_input[ch_idx].extend_from_slice(&ch_data[input_pos..end]);
                // Pad with zeros if last chunk is smaller
                if len < chunk_size {
                    chunk_input[ch_idx].resize(chunk_size, 0.0);
                }
            }

            let chunk_output = resampler.process(&chunk_input, None)?;

            for (ch_idx, ch_out) in chunk_output.iter().enumerate() {
                output_channels[ch_idx].extend_from_slice(ch_out);
            }

            input_pos += chunk_size;
        }

        for ch_buffer in &mut output_channels {
            ch_buffer.truncate(expected_output_frames);
        }

        // Flatten back to non-interleaved format
        let mut output_data = Vec::with_capacity(expected_output_frames * channels as usize);
        for ch_buffer in output_channels {
            output_data.extend(ch_buffer);
        }

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

        // Our data is stored as: [Ch0_all_frames..., Ch1_all_frames..., Ch2_all_frames...]
        // WAV needs: [Ch0_F0, Ch1_F0, Ch0_F1, Ch1_F1, Ch0_F2, Ch1_F2, ...]
        for frame_idx in 0..frames_per_channel {
            for ch_idx in 0..self.info.channels as usize {
                let sample_idx = ch_idx * frames_per_channel + frame_idx;
                let sample = self.audio_data[sample_idx];

                writer.write_sample(sample).with_context(|| {
                    format!(
                        "Failed to write sample at frame {}, channel {}",
                        frame_idx, ch_idx
                    )
                })?;
            }
        }

        writer
            .finalize()
            .with_context(|| format!("Failed to finalize WAV file: {}", path.display()))?;

        log::info!(
            "Successfully exported {} frames ({:.2}s) to WAV file",
            frames_per_channel,
            frames_per_channel as f64 / self.info.sample_rate as f64
        );

        Ok(())
    }

    /// Read exactly samples_per_packet frames (e.g., 48 samples for 1ms at 48kHz)
    pub fn read_next_frame(&mut self) -> Result<Option<AudioSample>> {
        let mut sample = AudioSample {
            data: Vec::with_capacity(self.samples_per_packet * self.info.channels as usize),
            channels: self.info.channels,
            sample_rate: self.info.sample_rate,
            frames: self.samples_per_packet,
        };

        if self.read_next_frame_into(&mut sample)? {
            Ok(Some(sample))
        } else {
            Ok(None)
        }
    }

    pub fn read_next_frame_into(&mut self, output: &mut AudioSample) -> Result<bool> {
        let channels = self.info.channels as usize;
        let frames_per_channel = self.audio_data.len() / channels;

        // Check if we have enough frames left
        if self.read_position + self.samples_per_packet > frames_per_channel {
            return Ok(false); // End of file
        }

        // Reuse the pre-allocated buffer to avoid allocations
        output.data.clear();

        for ch in 0..channels {
            let channel_start = ch * frames_per_channel;
            let frame_start = channel_start + self.read_position;
            let frame_end = frame_start + self.samples_per_packet;

            output
                .data
                .extend_from_slice(&self.audio_data[frame_start..frame_end]);
        }

        // Advance read position
        self.read_position += self.samples_per_packet;
        output.channels = self.info.channels;
        output.sample_rate = self.info.sample_rate;
        output.frames = self.samples_per_packet;

        Ok(true)
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

        // Pre-allocate non-interleaved buffer: [ch1_samples..., ch2_samples..., ch3_samples...]
        let mut noninterleaved_samples = Vec::with_capacity(frames * channels as usize);

        // Helper macro to convert to non-interleaved format - optimized version
        macro_rules! convert_to_noninterleaved {
            ($buf:expr, $normalize:expr) => {
                for channel_idx in 0..channels {
                    let channel_data = $buf.chan(channel_idx as usize);
                    let channel_len = channel_data.len().min(frames);
                    for frame_idx in 0..channel_len {
                        let sample = channel_data[frame_idx];
                        noninterleaved_samples.push($normalize(sample));
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
                convert_to_noninterleaved!(buf, |sample| {
                    // Avoid overflow by using saturating arithmetic
                    let normalized = sample as f64 / u32::MAX as f64;
                    (normalized * 2.0 - 1.0) as f32
                });
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
                    for sample in channel_data.iter().take(channel_data.len().min(frames)) {
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
                    for sample in channel_data.iter().take(channel_data.len().min(frames)) {
                        noninterleaved_samples.push(*sample);
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
    use std::fs;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Helper function to create mock stereo audio data in non-interleaved format
    fn create_mock_audio_data(frames: usize, channels: u32) -> Vec<f32> {
        let mut data = Vec::with_capacity(frames * channels as usize);

        // Generate a simple sine wave for each channel
        for ch in 0..channels {
            for frame in 0..frames {
                let frequency = 440.0 + (ch as f32 * 220.0); // Different freq per channel
                let sample =
                    (2.0 * std::f32::consts::PI * frequency * frame as f32 / 48000.0).sin();
                data.push(sample * 0.5); // Reduce amplitude to avoid clipping
            }
        }
        data
    }

    // Helper to create a simple WAV file for testing
    fn create_test_wav_file(sample_rate: u32, channels: u16, frames: usize) -> NamedTempFile {
        let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");

        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        let mut writer =
            hound::WavWriter::new(&mut temp_file, spec).expect("Failed to create WAV writer");

        // Write interleaved audio data
        for frame in 0..frames {
            for ch in 0..channels {
                let frequency = 440.0 + (ch as f32 * 220.0);
                let sample = (2.0 * std::f32::consts::PI * frequency * frame as f32
                    / sample_rate as f32)
                    .sin();
                writer
                    .write_sample(sample * 0.5)
                    .expect("Failed to write sample");
            }
        }

        writer.finalize().expect("Failed to finalize WAV");
        temp_file
    }

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
    fn test_resample_upsampling() {
        // Test upsampling from 44.1kHz to 48kHz
        let input_frames = 4096; // Use larger size for rubato
        let channels = 2;
        let input_data = create_mock_audio_data(input_frames, channels);

        let result = AudioReader::resample_audio_data_simple(input_data, 44100, 48000, channels)
            .expect("Resampling failed");

        let expected_output_frames = ((input_frames as f64) * (48000.0 / 44100.0)).round() as usize;
        let expected_total_samples = expected_output_frames * channels as usize;

        // Rubato might produce slightly different number of frames due to padding/chunking
        // So we check if it's close enough (within 1 chunk size)
        assert!(result.len() >= expected_total_samples.saturating_sub(2048));
        assert!(result.len() <= expected_total_samples + 2048);

        // Check that samples are in reasonable range
        for sample in &result {
            assert!(sample.abs() <= 1.5, "Sample out of range: {}", sample);
        }
    }

    #[test]
    fn test_resample_downsampling() {
        // Test downsampling from 48kHz to 44.1kHz
        let input_frames = 4096;
        let channels = 2;
        let input_data = create_mock_audio_data(input_frames, channels);

        let result = AudioReader::resample_audio_data_simple(input_data, 48000, 44100, channels)
            .expect("Resampling failed");

        let expected_output_frames = ((input_frames as f64) * (44100.0 / 48000.0)).round() as usize;
        let expected_total_samples = expected_output_frames * channels as usize;

        assert!(result.len() >= expected_total_samples.saturating_sub(2048));
        assert!(result.len() <= expected_total_samples + 2048);

        // Check that samples are in reasonable range
        for sample in &result {
            assert!(sample.abs() <= 1.5, "Sample out of range: {}", sample);
        }
    }

    #[test]
    fn test_resample_trims_final_padding() {
        let input_frames = 1500;
        let channels = 2;
        let input_data = create_mock_audio_data(input_frames, channels);

        let result = AudioReader::resample_audio_data_simple(input_data, 44100, 48000, channels)
            .expect("Resampling failed");

        let expected_output_frames = ((input_frames as f64) * (48000.0 / 44100.0)).round() as usize;

        assert_eq!(
            result.len(),
            expected_output_frames * channels as usize,
            "resampler should not keep zero-padding from the final fixed-size input chunk"
        );
    }

    #[test]
    fn test_resample_no_change() {
        // Test when input and output sample rates are the same
        // Note: rubato might still process it, or we might skip it in the caller
        // In our implementation, we call rubato regardless if we call this function
        // But the caller `load_audio_file` checks for equality before calling

        let input_frames = 1024;
        let channels = 2;
        let input_data = create_mock_audio_data(input_frames, channels);

        let result =
            AudioReader::resample_audio_data_simple(input_data.clone(), 48000, 48000, channels)
                .expect("Resampling failed");

        // Rubato with ratio 1.0 should preserve length approximately
        assert!(result.len() >= input_data.len() - 2048);

        // Should be similar
        // We can't expect exact equality with rubato even at 1.0 ratio due to filtering
    }

    #[test]
    fn test_read_next_frame_sequential() {
        let temp_file = create_test_wav_file(48000, 2, 144); // 3ms of audio at 48kHz
        let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");

        let mut frames_read = 0;
        let mut last_sample_data: Option<Vec<f32>> = None;

        // Should be able to read exactly 3 frames (144 samples / 48 samples per frame)
        while let Some(sample) = reader.read_next_frame().expect("Read failed") {
            assert_eq!(sample.frames, 48);
            assert_eq!(sample.channels, 2);
            assert_eq!(sample.sample_rate, 48000);
            assert_eq!(sample.data.len(), 48 * 2); // 48 frames × 2 channels

            // Verify data is different from previous frame (unless it's a DC signal)
            if let Some(ref prev_data) = last_sample_data {
                let _is_different = sample
                    .data
                    .iter()
                    .zip(prev_data.iter())
                    .any(|(a, b)| (a - b).abs() > 1e-6);
                // Note: for sine waves, consecutive frames should be different
                // This check could be expanded for regression testing
            }

            last_sample_data = Some(sample.data.clone());
            frames_read += 1;
        }

        assert_eq!(frames_read, 3, "Should read exactly 3 frames");

        // Next read should return None
        assert!(reader.read_next_frame().expect("Read failed").is_none());
    }

    #[test]
    fn test_read_next_frame_end_of_file() {
        // Create a very short file (only 24 samples = 0.5ms at 48kHz)
        let temp_file = create_test_wav_file(48000, 2, 24);
        let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");

        // Should not be able to read a full frame since file is too short
        let result = reader.read_next_frame().expect("Read failed");
        assert!(result.is_none(), "Should return None for insufficient data");
    }

    #[test]
    fn test_rewind_resets_reader_to_first_packet() {
        let temp_file = create_test_wav_file(48000, 2, 144);
        let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");

        let first = reader
            .read_next_frame()
            .expect("Read failed")
            .expect("Should have first packet");
        let second = reader
            .read_next_frame()
            .expect("Read failed")
            .expect("Should have second packet");

        assert_ne!(
            first.data, second.data,
            "test data should advance between packets"
        );

        reader.rewind();

        let first_after_rewind = reader
            .read_next_frame()
            .expect("Read failed")
            .expect("Should have first packet after rewind");

        assert_eq!(first.data, first_after_rewind.data);
    }

    #[test]
    fn test_read_next_frame_returns_independent_owned_samples() {
        let temp_file = create_test_wav_file(48000, 2, 96); // 2ms of audio
        let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");

        // Read first frame
        let sample1 = reader
            .read_next_frame()
            .expect("Read failed")
            .expect("Should have data");

        // Read second frame
        let sample2 = reader
            .read_next_frame()
            .expect("Read failed")
            .expect("Should have data");

        // Verify both samples have correct structure
        assert_eq!(sample1.data.len(), 96); // 48 frames × 2 channels
        assert_eq!(sample2.data.len(), 96);
        assert_eq!(sample1.frames, 48);
        assert_eq!(sample2.frames, 48);
    }

    #[test]
    fn test_read_next_frame_into_reuses_output_allocation() {
        let temp_file = create_test_wav_file(48000, 2, 96);
        let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");
        let mut output = AudioSample {
            data: Vec::with_capacity(96),
            channels: 0,
            sample_rate: 0,
            frames: 0,
        };

        assert!(reader
            .read_next_frame_into(&mut output)
            .expect("Read failed"));
        let first_ptr = output.data.as_ptr();
        assert_eq!(output.channels, 2);
        assert_eq!(output.sample_rate, 48000);
        assert_eq!(output.frames, 48);
        assert_eq!(output.data.len(), 96);

        assert!(reader
            .read_next_frame_into(&mut output)
            .expect("Read failed"));

        assert_eq!(
            output.data.as_ptr(),
            first_ptr,
            "output allocation should be reused across packet reads"
        );
        assert_eq!(output.data.len(), 96);
    }

    #[test]
    fn test_wav_export_roundtrip() {
        let temp_input = create_test_wav_file(48000, 2, 96);
        let reader = AudioReader::with_resampling(temp_input.path(), 48000, 48)
            .expect("Failed to create reader");

        // Export to another temp file
        let temp_output = NamedTempFile::new().expect("Failed to create temp output");
        reader
            .export_to_wav(temp_output.path())
            .expect("Export failed");

        // Verify the exported file exists and has content
        let metadata = fs::metadata(temp_output.path()).expect("Can't read exported file metadata");
        assert!(metadata.len() > 0, "Exported file should not be empty");

        // Try to read the exported file back
        let reader2 = AudioReader::with_resampling(temp_output.path(), 48000, 48)
            .expect("Failed to read exported file");

        let info = reader2.get_info();
        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.channels, 2);
    }

    #[test]
    fn test_mono_audio_handling() {
        let temp_file = create_test_wav_file(48000, 1, 48); // Mono audio
        let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");

        let sample = reader
            .read_next_frame()
            .expect("Read failed")
            .expect("Should have data");

        assert_eq!(sample.channels, 1);
        assert_eq!(sample.frames, 48);
        assert_eq!(sample.data.len(), 48); // 48 frames × 1 channel
    }

    #[test]
    fn test_multichannel_audio_handling() {
        let temp_file = create_test_wav_file(48000, 8, 48); // 8-channel audio
        let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");

        let sample = reader
            .read_next_frame()
            .expect("Read failed")
            .expect("Should have data");

        assert_eq!(sample.channels, 8);
        assert_eq!(sample.frames, 48);
        assert_eq!(sample.data.len(), 48 * 8); // 48 frames × 8 channels
    }

    #[test]
    fn test_different_packet_sizes() {
        let temp_file = create_test_wav_file(48000, 2, 240); // 5ms of audio

        // Test different packet sizes
        for &packet_size in &[12, 24, 48, 96] {
            // 0.25ms, 0.5ms, 1ms, 2ms at 48kHz
            let mut reader = AudioReader::with_resampling(temp_file.path(), 48000, packet_size)
                .expect("Failed to create reader");

            let sample = reader.read_next_frame().expect("Read failed");
            if let Some(sample) = sample {
                assert_eq!(sample.frames, packet_size);
                assert_eq!(sample.data.len(), packet_size * 2); // 2 channels
            }
        }
    }

    #[test]
    fn test_audio_reader_info_consistency() {
        let temp_file = create_test_wav_file(48000, 2, 96);
        let reader = AudioReader::with_resampling(temp_file.path(), 48000, 48)
            .expect("Failed to create reader");

        let info = reader.get_info();
        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bit_depth, Some(24)); // AES67 standard
        assert_eq!(info.format, "Loaded");
        assert!(info.duration.is_some());

        // Duration should be reasonable for our test data
        let duration = info.duration.unwrap();
        assert!(duration.as_millis() > 0);
        assert!(duration.as_millis() < 100); // Very short test file
    }

    #[test]
    fn test_error_handling_nonexistent_file() {
        let result = AudioReader::new("nonexistent_file.wav");
        assert!(result.is_err(), "Should fail for nonexistent file");
    }

    #[test]
    fn test_error_handling_invalid_file() {
        let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
        temp_file
            .write_all(b"This is not a valid audio file")
            .expect("Write failed");
        temp_file.flush().expect("Flush failed");

        let result = AudioReader::new(temp_file.path());
        assert!(result.is_err(), "Should fail for invalid audio file");
    }

    #[test]
    #[ignore] // Requires test file
    fn test_audio_reader_with_real_file() {
        // This test can be run with: cargo test -- --ignored
        let test_file = "../../tests/piano_freesound.wav";

        if std::path::Path::new(test_file).exists() {
            let mut reader = AudioReader::new(test_file).expect("Failed to open test file");

            let info = reader.get_info();
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
                        assert!(!sample.data.is_empty());
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
