use anyhow::Result;
use audio::{AudioInfo, AudioReader, AudioSample};
use std::time::Duration;

pub trait StreamAudioSource: Send {
    fn get_info(&self) -> &AudioInfo;
    fn read_next_frame_into(&mut self, output: &mut AudioSample) -> Result<bool>;
    fn rewind(&mut self);

    fn can_read_full_packet(&self) -> bool {
        true
    }
}

impl StreamAudioSource for AudioReader {
    fn get_info(&self) -> &AudioInfo {
        self.get_info()
    }

    fn read_next_frame_into(&mut self, output: &mut AudioSample) -> Result<bool> {
        self.read_next_frame_into(output)
    }

    fn rewind(&mut self) {
        self.rewind();
    }

    fn can_read_full_packet(&self) -> bool {
        self.can_read_full_packet()
    }
}

pub struct SilenceSource {
    info: AudioInfo,
    frames_per_packet: usize,
}

impl SilenceSource {
    pub fn new(channels: u32, sample_rate: u32, frames_per_packet: usize) -> Self {
        Self {
            info: AudioInfo {
                sample_rate,
                channels,
                duration: Some(Duration::ZERO),
                bit_depth: Some(24),
                format: "Silence".to_string(),
            },
            frames_per_packet,
        }
    }
}

impl StreamAudioSource for SilenceSource {
    fn get_info(&self) -> &AudioInfo {
        &self.info
    }

    fn read_next_frame_into(&mut self, output: &mut AudioSample) -> Result<bool> {
        output.channels = self.info.channels;
        output.sample_rate = self.info.sample_rate;
        output.frames = self.frames_per_packet;
        output
            .data
            .resize(self.frames_per_packet * self.info.channels as usize, 0.0);
        output.data.fill(0.0);
        Ok(true)
    }

    fn rewind(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_source_fills_requested_packet_shape() {
        let mut source = SilenceSource::new(2, 48_000, 48);
        let mut sample = AudioSample {
            data: Vec::new(),
            channels: 2,
            sample_rate: 48_000,
            frames: 48,
        };

        let read = source
            .read_next_frame_into(&mut sample)
            .expect("silence source should read");

        assert!(read);
        assert_eq!(sample.channels, 2);
        assert_eq!(sample.sample_rate, 48_000);
        assert_eq!(sample.frames, 48);
        assert_eq!(sample.data.len(), 96);
        assert!(sample.data.iter().all(|value| *value == 0.0));
    }
}
