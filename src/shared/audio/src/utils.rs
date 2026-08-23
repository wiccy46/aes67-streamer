/// Convert interleaved audio to non-interleaved format
/// Input: [L1, R1, L2, R2, L3, R3, ...] (interleaved)
/// Output: [L1, L2, L3, ..., R1, R2, R3, ...] (non-interleaved)
pub fn interleaved_to_noninterleaved(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if interleaved.is_empty() || channels == 0 {
        return Vec::new();
    }

    let frames = interleaved.len() / channels;
    let mut noninterleaved = vec![0.0f32; interleaved.len()];

    // Convert interleaved to planar format
    for frame_idx in 0..frames {
        for ch_idx in 0..channels {
            let interleaved_idx = frame_idx * channels + ch_idx;
            let planar_idx = ch_idx * frames + frame_idx;
            noninterleaved[planar_idx] = interleaved[interleaved_idx];
        }
    }

    noninterleaved
}

/// Convert non-interleaved audio to interleaved format
/// Input: [L1, L2, L3, ..., R1, R2, R3, ...] (non-interleaved)
/// Output: [L1, R1, L2, R2, L3, R3, ...] (interleaved)
pub fn noninterleaved_to_interleaved(noninterleaved: &[f32], channels: usize) -> Vec<f32> {
    if noninterleaved.is_empty() || channels == 0 {
        return Vec::new();
    }

    let frames = noninterleaved.len() / channels;
    let mut interleaved = vec![0.0f32; noninterleaved.len()];

    // Convert planar to interleaved format
    for frame_idx in 0..frames {
        for ch_idx in 0..channels {
            let planar_idx = ch_idx * frames + frame_idx;
            let interleaved_idx = frame_idx * channels + ch_idx;
            interleaved[interleaved_idx] = noninterleaved[planar_idx];
        }
    }

    interleaved
}

/// Convert non-interleaved flat array to channels vector for processing
/// Input: [ch1_samples..., ch2_samples..., ch3_samples...]
/// Output: [[ch1_samples], [ch2_samples], [ch3_samples]]
pub fn flat_noninterleaved_to_channels(
    flat_noninterleaved: &[f32],
    channels: usize,
    frames: usize,
) -> Vec<Vec<f32>> {
    let mut channel_vecs = Vec::with_capacity(channels);

    // Calculate actual frames available per channel
    let available_frames = flat_noninterleaved.len().checked_div(channels).unwrap_or(0);
    let actual_frames = frames.min(available_frames);

    for ch_idx in 0..channels {
        let start = ch_idx * actual_frames;
        let end = start + actual_frames;
        if end <= flat_noninterleaved.len() {
            channel_vecs.push(flat_noninterleaved[start..end].to_vec());
        } else {
            // Handle case where data is incomplete - pad with zeros
            let mut channel_data = Vec::with_capacity(actual_frames);
            let available_end = flat_noninterleaved.len().min(end);
            if start < available_end {
                channel_data.extend_from_slice(&flat_noninterleaved[start..available_end]);
            }
            // Pad with zeros if needed
            channel_data.resize(actual_frames, 0.0);
            channel_vecs.push(channel_data);
        }
    }

    channel_vecs
}

/// Convert channels vector back to flat non-interleaved format
/// Input: [[ch1_samples], [ch2_samples], [ch3_samples]]
/// Output: [ch1_samples..., ch2_samples..., ch3_samples...]
pub fn channels_to_flat_noninterleaved(channels: &[Vec<f32>]) -> Vec<f32> {
    if channels.is_empty() {
        return Vec::new();
    }

    let mut flat_noninterleaved = Vec::with_capacity(channels.len() * channels[0].len());

    // Concatenate all channels
    for channel in channels {
        flat_noninterleaved.extend_from_slice(channel);
    }

    flat_noninterleaved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interleaved_to_noninterleaved() {
        // Test stereo: [L1, R1, L2, R2] → [L1, L2, R1, R2]
        let interleaved = vec![1.0, 2.0, 3.0, 4.0];
        let noninterleaved = interleaved_to_noninterleaved(&interleaved, 2);

        assert_eq!(noninterleaved, vec![1.0, 3.0, 2.0, 4.0]);
    }

    #[test]
    fn test_noninterleaved_to_interleaved() {
        // Test stereo: [L1, L2, R1, R2] → [L1, R1, L2, R2]
        let noninterleaved = vec![1.0, 3.0, 2.0, 4.0];
        let interleaved = noninterleaved_to_interleaved(&noninterleaved, 2);

        assert_eq!(interleaved, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_round_trip_conversion() {
        let original = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]; // 3 frames, 2 channels

        // Convert to non-interleaved and back
        let noninterleaved = interleaved_to_noninterleaved(&original, 2);
        let converted_back = noninterleaved_to_interleaved(&noninterleaved, 2);

        assert_eq!(original, converted_back);
    }

    #[test]
    fn test_flat_noninterleaved_conversion() {
        // Test stereo flat non-interleaved: [L1, L2, R1, R2]
        let flat_noninterleaved = vec![1.0, 3.0, 2.0, 4.0];
        let channels = flat_noninterleaved_to_channels(&flat_noninterleaved, 2, 2);

        assert_eq!(channels.len(), 2); // 2 channels
        assert_eq!(channels[0], vec![1.0, 3.0]); // Left channel
        assert_eq!(channels[1], vec![2.0, 4.0]); // Right channel

        // Convert back
        let converted_back = channels_to_flat_noninterleaved(&channels);
        assert_eq!(converted_back, flat_noninterleaved);
    }

    #[test]
    fn test_mono_conversion() {
        let mono_interleaved = vec![1.0, 2.0, 3.0];
        let mono_noninterleaved = interleaved_to_noninterleaved(&mono_interleaved, 1);

        // For mono, should be identical
        assert_eq!(mono_interleaved, mono_noninterleaved);

        let converted_back = noninterleaved_to_interleaved(&mono_noninterleaved, 1);
        assert_eq!(mono_interleaved, converted_back);
    }

    #[test]
    fn test_multichannel_conversion() {
        // Test 4-channel: [1, 2, 3, 4, 5, 6, 7, 8] → [1, 5, 2, 6, 3, 7, 4, 8]
        let interleaved = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]; // 2 frames, 4 channels
        let noninterleaved = interleaved_to_noninterleaved(&interleaved, 4);

        assert_eq!(noninterleaved, vec![1.0, 5.0, 2.0, 6.0, 3.0, 7.0, 4.0, 8.0]);

        let converted_back = noninterleaved_to_interleaved(&noninterleaved, 4);
        assert_eq!(interleaved, converted_back);
    }
}
