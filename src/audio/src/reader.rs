use std::fs::File;
use std::path::Path;
use std::time::Duration;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

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
    /// Interleaved audio data: [L, R, L, R, ...] for stereo
    /// or [L, R, C, LFE, LS, RS, ...] for surround
    pub data: Vec<f32>,
    pub channels: u32,
    pub sample_rate: u32,
    /// Number of frames (samples per channel)
    pub frames: usize,
}

pub struct AudioReader {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    info: AudioInfo,
}

impl AudioReader {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path.as_ref())?;
        let media_source = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(extension) = path.as_ref().extension() {
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

        let format_reader = probed.format;

        // Find the default audio track
        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            .ok_or_else(|| anyhow::anyhow!("No audio tracks found"))?;

        let track_id = track.id;

        let decoder_opts = DecoderOptions::default();
        let decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

        let codec_params = &track.codec_params;
        
        // Validate essential audio properties - fail explicitly if missing
        let sample_rate = codec_params.sample_rate
            .ok_or_else(|| anyhow::anyhow!("Audio file is missing sample rate information - file may be corrupted or unsupported"))?;
            
        let channels = codec_params.channels
            .map(|ch| ch.count() as u32)
            .ok_or_else(|| anyhow::anyhow!("Audio file is missing channel information - file may be corrupted or unsupported"))?;
            
        // Validate reasonable values
        if sample_rate == 0 {
            return Err(anyhow::anyhow!("Invalid sample rate (0) - audio file is corrupted"));
        }
        if channels == 0 {
            return Err(anyhow::anyhow!("Invalid channel count (0) - audio file is corrupted"));
        }
        if channels > 64 {
            return Err(anyhow::anyhow!("Unsupported channel count ({}) - maximum 64 channels supported", channels));
        }
        
        let info = AudioInfo {
            sample_rate,
            channels,
            duration: codec_params.time_base.and_then(|tb| {
                codec_params.n_frames.map(|frames| {
                    Duration::from_secs_f64(frames as f64 / tb.denom as f64 * tb.numer as f64)
                })
            }),
            bit_depth: codec_params.bits_per_sample,
            format: format!("{:?}", codec_params.codec),
        };

        log::info!(
            "Loaded audio file - Sample Rate: {} Hz, Channels: {}, Duration: {}, Format: {}",
            info.sample_rate,
            info.channels,
            info.duration
                .map(|d| format!("{:.2}s", d.as_secs_f64()))
                .unwrap_or_else(|| "Unknown".to_string()),
            info.format
        );

        Ok(AudioReader {
            format_reader,
            decoder,
            track_id,
            info,
        })
    }

    pub fn info(&self) -> &AudioInfo {
        &self.info
    }

    pub fn read_next_frame(&mut self) -> Result<Option<AudioSample>> {
        // Get the next packet from the format reader
        let packet = match self.format_reader.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };

        // Skip packets that don't belong to our track
        if packet.track_id() != self.track_id {
            return self.read_next_frame();
        }

        // Decode the packet
        let decoded = self.decoder.decode(&packet)?;

        // Convert to f32 samples
        let audio_sample = Self::convert_audio_buffer(decoded)?;

        Ok(Some(audio_sample))
    }

    fn convert_audio_buffer(buffer: AudioBufferRef) -> Result<AudioSample> {
        let spec = *buffer.spec();
        let channels = spec.channels.count() as u32;
        let frames = buffer.capacity();
        let sample_rate = spec.rate;

        // Pre-allocate interleaved buffer: [L, R, L, R, ...]
        let mut interleaved_samples = Vec::with_capacity(frames * channels as usize);

        // Helper macro to convert and interleave samples
        macro_rules! convert_and_interleave {
            ($buf:expr, $convert:expr) => {
                for frame_idx in 0..frames {
                    for channel_idx in 0..channels {
                        let sample = $buf.chan(channel_idx as usize)[frame_idx];
                        let normalized = $convert(sample);
                        interleaved_samples.push(normalized);
                    }
                }
            };
        }

        match buffer {
            AudioBufferRef::U8(buf) => {
                convert_and_interleave!(buf, |sample| (sample as f32 - 128.0) / 128.0);
            }
            AudioBufferRef::U16(buf) => {
                convert_and_interleave!(buf, |sample| (sample as f32 - 32768.0) / 32768.0);
            }
            AudioBufferRef::U24(buf) => {
                for frame_idx in 0..frames {
                    for channel_idx in 0..channels {
                        let sample = buf.chan(channel_idx as usize)[frame_idx];
                        let sample_val = sample.inner();
                        let normalized = (sample_val as f32 - 8388608.0) / 8388608.0;
                        interleaved_samples.push(normalized);
                    }
                }
            }
            AudioBufferRef::U32(buf) => {
                convert_and_interleave!(buf, |sample| (sample as f32 - 2147483648.0)
                    / 2147483648.0);
            }
            AudioBufferRef::S8(buf) => {
                convert_and_interleave!(buf, |sample| sample as f32 / 128.0);
            }
            AudioBufferRef::S16(buf) => {
                convert_and_interleave!(buf, |sample| sample as f32 / 32768.0);
            }
            AudioBufferRef::S24(buf) => {
                for frame_idx in 0..frames {
                    for channel_idx in 0..channels {
                        let sample = buf.chan(channel_idx as usize)[frame_idx];
                        let sample_val = sample.inner();
                        let normalized = sample_val as f32 / 8388608.0;
                        interleaved_samples.push(normalized);
                    }
                }
            }
            AudioBufferRef::S32(buf) => {
                convert_and_interleave!(buf, |sample| sample as f32 / 2147483648.0);
            }
            AudioBufferRef::F32(buf) => {
                // F32 is already normalized, just interleave
                for frame_idx in 0..frames {
                    for channel_idx in 0..channels {
                        let sample = buf.chan(channel_idx as usize)[frame_idx];
                        interleaved_samples.push(sample);
                    }
                }
            }
            AudioBufferRef::F64(buf) => {
                convert_and_interleave!(buf, |sample| sample as f32);
            }
        }

        Ok(AudioSample {
            data: interleaved_samples,
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
            data: vec![0.0, 0.5, -0.5, 1.0], // 2 frames, 2 channels interleaved
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
