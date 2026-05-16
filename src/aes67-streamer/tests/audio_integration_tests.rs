use audio::{AudioReader, GainNode, AudioNode};
use std::path::PathBuf;

#[test]
fn test_audio_file_loading_integration() {
    // Test loading a real audio file
    let test_file = "../../tests/piano_freesound.wav";
    
    if std::path::Path::new(test_file).exists() {
        let reader = AudioReader::new(test_file).expect("Failed to load test audio file");
        let info = reader.info();
        
        // Verify expected properties
        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.channels, 2);
        assert!(info.duration.is_some());
        
        println!("✅ Audio file loaded: {} Hz, {} channels", info.sample_rate, info.channels);
    } else {
        panic!("Test audio file not found: {test_file}");
    }
}

#[test]
fn test_common_audio_file_formats_load() {
    for filename in ["tone.wav", "tone.flac", "tone.mp3", "tone.aiff"] {
        let path = audio_format_resource(filename);
        let reader = AudioReader::with_resampling(&path, 48000, 48)
            .unwrap_or_else(|error| panic!("failed to load {filename}: {error:#}"));
        let info = reader.info();

        assert_eq!(info.sample_rate, 48000, "{filename} should target AES67 sample rate");
        assert_eq!(info.channels, 2, "{filename} should preserve stereo layout");
        assert!(
            info.duration.is_some_and(|duration| duration.as_millis() > 0),
            "{filename} should expose a non-empty duration"
        );
    }
}

fn audio_format_resource(filename: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/resources/audio-formats")
        .join(filename)
}

#[test]
fn test_audio_processing_integration() {
    // This file is 44.1kHz, should be converted
    let test_file = "../../tests/piano_freesound.wav";
    
    if std::path::Path::new(test_file).exists() {
        let mut reader = AudioReader::new(test_file).expect("Failed to load test audio file");
        let mut gain_node_chain = GainNode::new_db(-6.0).into_chain();
        
        // Process a few frames
        let mut frames_processed = 0;
        for _ in 0..3 {
            if let Some(mut sample) = reader.read_next_frame().unwrap() {
                let original_peak = sample.data.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
                
                // Apply gain processing through node chain
                let result = gain_node_chain.process(&mut sample);
                assert!(result.is_ok());
                
                let processed_peak = sample.data.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
                
                // Verify gain was applied (-6dB ≈ 0.5x)
                assert!(processed_peak < original_peak);
                assert!((processed_peak / original_peak - 0.5).abs() < 0.1);
                
                frames_processed += 1;
            }
        }
        
        assert!(frames_processed > 0, "Should have processed at least one frame");
        println!("✅ Processed {frames_processed} audio frames with gain node chain");
    }
}

#[test]
fn test_chained_nodes_integration() {
    let test_file = "../../tests/piano_freesound.wav";
    
    if std::path::Path::new(test_file).exists() {
        let mut reader = AudioReader::new(test_file).expect("Failed to load test audio file");
        
        // Create a chain: gain reduction followed by gain boost (should be close to original)
        let mut node_chain = GainNode::new_db(-6.0)  // Reduce by 6dB
            .into_chain()
            .chain(Box::new(GainNode::new_db(6.0)));  // Boost by 6dB
        
        if let Some(mut sample) = reader.read_next_frame().unwrap() {
            let original_peak = sample.data.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
            
            node_chain.process(&mut sample).unwrap();
            
            let processed_peak = sample.data.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
            
            // Should be close to original (within 1% due to floating point precision)
            assert!((processed_peak / original_peak - 1.0).abs() < 0.01);
            
            println!("✅ Chained node processing: original peak {original_peak:.3}, processed peak {processed_peak:.3}");
        }
    }
}

#[test]
fn test_error_handling_integration() {
    let result = AudioReader::new("nonexistent.wav");
    assert!(result.is_err());
    
    let result = AudioReader::new("../../Cargo.toml");
    assert!(result.is_err());
    
    println!("✅ Error handling works correctly");
}

#[test]
fn test_multi_channel_processing() {
    let test_file = "../../tests/piano_freesound.wav";
    
    if std::path::Path::new(test_file).exists() {
        let mut reader = AudioReader::new(test_file).expect("Failed to load test audio file");
        
        if let Some(sample) = reader.read_next_frame().unwrap() {
            // Verify interleaved format: data.len() = frames * channels
            let expected_len = sample.frames * sample.channels as usize;
            assert_eq!(sample.data.len(), expected_len);
            
            // For stereo: [L, R, L, R, ...]
            if sample.channels == 2 {
                assert!(sample.data.len() % 2 == 0);
            }
            
            println!("✅ Multi-channel format verified: {} frames, {} channels, {} samples", 
                sample.frames, sample.channels, sample.data.len());
        }
    }
}
