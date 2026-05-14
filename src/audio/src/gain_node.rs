use crate::node::{AudioNode, BaseAudioNode};
use crate::{AudioSample, Result};

/// Gain node for volume control with clipping protection
pub struct GainNode {
    /// Base node functionality
    base: BaseAudioNode,
    /// Gain in linear scale (1.0 = unity, 2.0 = +6dB, 0.5 = -6dB)
    gain_linear: f32,
    /// Gain in decibels for convenience
    gain_db: f32,
    /// Enable soft clipping protection
    clip_protection: bool,
    /// Peak level tracking for metering
    peak_level: f32,
    /// RMS level tracking for metering
    rms_level: f32,
    /// Sample counter for RMS calculation
    sample_count: usize,
    /// RMS accumulator
    rms_accumulator: f64,
}

impl GainNode {
    /// Create new gain node with gain in decibels
    pub fn new_db(gain_db: f32) -> Self {
        let gain_linear = Self::db_to_linear(gain_db);
        Self {
            base: BaseAudioNode::new("GainNode"),
            gain_linear,
            gain_db,
            clip_protection: true,
            peak_level: 0.0,
            rms_level: 0.0,
            sample_count: 0,
            rms_accumulator: 0.0,
        }
    }

    /// Create new gain node with linear gain
    pub fn new_linear(gain_linear: f32) -> Self {
        let gain_db = Self::linear_to_db(gain_linear);
        Self {
            base: BaseAudioNode::new("GainNode"),
            gain_linear,
            gain_db,
            clip_protection: true,
            peak_level: 0.0,
            rms_level: 0.0,
            sample_count: 0,
            rms_accumulator: 0.0,
        }
    }

    /// Set gain in decibels
    pub fn set_gain_db(&mut self, gain_db: f32) {
        self.gain_db = gain_db;
        self.gain_linear = Self::db_to_linear(gain_db);
    }

    /// Set gain in linear scale
    pub fn set_gain_linear(&mut self, gain_linear: f32) {
        self.gain_linear = gain_linear;
        self.gain_db = Self::linear_to_db(gain_linear);
    }

    /// Get current gain in decibels
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Get current gain in linear scale
    pub fn gain_linear(&self) -> f32 {
        self.gain_linear
    }

    /// Enable/disable clipping protection
    pub fn set_clip_protection(&mut self, enabled: bool) {
        self.clip_protection = enabled;
    }

    /// Get peak level since last reset (0.0 to 1.0+)
    pub fn peak_level(&self) -> f32 {
        self.peak_level
    }

    /// Get RMS level since last reset (0.0 to 1.0+)
    pub fn rms_level(&self) -> f32 {
        self.rms_level
    }

    /// Convert decibels to linear gain
    fn db_to_linear(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }

    /// Convert linear gain to decibels
    fn linear_to_db(linear: f32) -> f32 {
        if linear <= 0.0 {
            -100.0 // Represent silence as -100dB
        } else {
            20.0 * linear.log10()
        }
    }

    /// Soft clipping function (tanh-based)
    fn soft_clip(sample: f32) -> f32 {
        if sample.abs() <= 1.0 {
            sample
        } else {
            sample.signum() * (1.0 - (-sample.abs() + 1.0).exp())
        }
    }

    /// Update level meters
    fn update_meters(&mut self, sample: f32) {
        // Update peak level
        let abs_sample = sample.abs();
        if abs_sample > self.peak_level {
            self.peak_level = abs_sample;
        }

        // Update RMS calculation
        self.rms_accumulator += (sample * sample) as f64;
        self.sample_count += 1;

        // Calculate RMS every 1024 samples to avoid overflow
        if self.sample_count >= 1024 {
            self.rms_level = (self.rms_accumulator / self.sample_count as f64).sqrt() as f32;
            self.rms_accumulator = 0.0;
            self.sample_count = 0;
        }
    }
}

impl AudioNode for GainNode {
    fn process(&mut self, sample: &mut AudioSample) -> Result<bool> {
        if !self.is_enabled() {
            return Ok(false);
        }

        // Process non-interleaved audio data efficiently (channel by channel)
        let frames_per_channel = sample.frames;
        let channels = sample.channels as usize;

        // Process each channel separately for better cache efficiency
        for ch_idx in 0..channels {
            let channel_start = ch_idx * frames_per_channel;
            let channel_end = (channel_start + frames_per_channel).min(sample.data.len());

            // Skip this channel if we don't have enough data
            if channel_start >= sample.data.len() {
                log::warn!(
                    "Channel {} starts beyond available data ({} >= {})",
                    ch_idx,
                    channel_start,
                    sample.data.len()
                );
                continue;
            }

            // Process this channel's samples (with bounds protection)
            for value in &mut sample.data[channel_start..channel_end] {
                // Apply gain
                *value *= self.gain_linear;

                // Update meters before clipping
                self.update_meters(*value);

                // Apply clipping protection if enabled
                if self.clip_protection {
                    *value = Self::soft_clip(*value);
                }
            }
        }

        Ok(true)
    }

    fn reset(&mut self) {
        self.peak_level = 0.0;
        self.rms_level = 0.0;
        self.sample_count = 0;
        self.rms_accumulator = 0.0;
        self.base.reset();
    }

    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_enabled(&self) -> bool {
        self.base.is_enabled()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.base.set_enabled(enabled);
    }

    fn set_next(&mut self, next: Box<dyn AudioNode>) {
        self.base.set_next(next);
    }

    fn process_chain(&mut self, sample: &mut AudioSample) -> Result<()> {
        // Process through this node first
        if self.is_enabled() {
            self.process(sample)?;
        }
        // Then process through next node
        self.base.process_next(sample)
    }

    fn has_next(&self) -> bool {
        self.base.has_next()
    }
}

/// Example function showing how to use gain node
pub fn apply_gain_example(sample: &mut AudioSample, gain_db: f32) -> Result<()> {
    let mut gain_node = GainNode::new_db(gain_db);
    gain_node.process(sample)?;

    log::info!(
        "Applied {}dB gain, Peak: {:.3}, RMS: {:.3}",
        gain_db,
        gain_node.peak_level(),
        gain_node.rms_level()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_conversion() {
        // Test dB to linear conversion
        assert!((GainNode::db_to_linear(0.0) - 1.0).abs() < 0.001);
        assert!((GainNode::db_to_linear(6.02) - 2.0).abs() < 0.01); // 6.02dB ≈ 2x
        assert!((GainNode::db_to_linear(-6.02) - 0.5).abs() < 0.01);

        // Test linear to dB conversion
        assert!((GainNode::linear_to_db(1.0) - 0.0).abs() < 0.001);
        assert!((GainNode::linear_to_db(2.0) - 6.02).abs() < 0.1);
        assert!((GainNode::linear_to_db(0.5) - (-6.02)).abs() < 0.1);
    }

    #[test]
    fn test_gain_processing() {
        let mut node = GainNode::new_linear(2.0); // 2x gain

        let mut sample = AudioSample {
            data: vec![0.1, 0.3, 0.2, 0.4], // Planar: [L1, L2, R1, R2]
            channels: 2,
            sample_rate: 44100,
            frames: 2,
        };

        node.process(&mut sample).unwrap();

        // Values should be doubled
        assert!((sample.data[0] - 0.2).abs() < 0.001); // L1
        assert!((sample.data[1] - 0.6).abs() < 0.001); // L2
        assert!((sample.data[2] - 0.4).abs() < 0.001); // R1
        assert!((sample.data[3] - 0.8).abs() < 0.001); // R2
    }

    #[test]
    fn test_clipping_protection() {
        let mut node = GainNode::new_linear(10.0); // High gain
        node.set_clip_protection(true);

        let mut sample = AudioSample {
            data: vec![0.5, -0.5, 0.8, -0.8],
            channels: 2,
            sample_rate: 44100,
            frames: 2,
        };

        node.process(&mut sample).unwrap();

        // All values should be clipped to reasonable range
        for value in &sample.data {
            assert!(
                value.abs() <= 1.0,
                "Sample {} exceeds clipping threshold",
                value
            );
        }
    }

    #[test]
    fn test_level_metering() {
        let mut node = GainNode::new_linear(1.0);

        // Create a larger sample to trigger RMS calculation
        let mut large_data = vec![0.0; 2048]; // More than 1024 samples
        for (i, value) in large_data.iter_mut().enumerate() {
            *value = if i % 2 == 0 { 0.5 } else { -0.5 };
        }

        let mut sample = AudioSample {
            data: large_data,
            channels: 2,
            sample_rate: 44100,
            frames: 1024,
        };

        node.process(&mut sample).unwrap();

        // Peak should be 0.5 (highest absolute value)
        assert!((node.peak_level() - 0.5).abs() < 0.001);

        // RMS should be calculated and non-zero
        assert!(node.rms_level() > 0.0);
        assert!(node.rms_level() <= 1.0);
    }

    #[test]
    fn test_apply_gain_example() {
        let mut sample = AudioSample {
            data: vec![0.1, 0.3, 0.2, 0.4], // Planar: [L1, L2, R1, R2]
            channels: 2,
            sample_rate: 44100,
            frames: 2,
        };

        apply_gain_example(&mut sample, 6.0).unwrap();

        // Should be doubled due to +6dB gain
        assert!((sample.data[0] - 0.2).abs() < 0.001); // L1
    }
}
