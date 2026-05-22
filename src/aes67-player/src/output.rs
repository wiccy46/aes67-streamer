use anyhow::{anyhow, Result};
use config::PlayerOutput;

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
}

pub trait AudioOutput {
    fn write_interleaved(&mut self, samples: &[f32], channels: u16) -> Result<usize>;
    fn write_silence(&mut self, frames: u32, channels: u16) -> Result<()>;
    fn stats(&self) -> OutputStats;
}

pub fn build_output(mode: OutputMode) -> Result<Box<dyn AudioOutput + Send>> {
    match mode {
        OutputMode::Null => Ok(Box::new(NullOutput::default())),
        OutputMode::Cpal => Err(anyhow!(
            "CPAL output is not implemented yet; use --output null for the current player MVP"
        )),
    }
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
    }

    #[test]
    fn null_output_rejects_invalid_interleaved_shape() {
        let mut output = NullOutput::default();

        assert!(output.write_interleaved(&[0.0, 0.1, 0.2], 2).is_err());
        assert!(output.write_interleaved(&[0.0], 0).is_err());
        assert!(output.write_silence(48, 0).is_err());
    }
}
