use audio::{AudioReader, GainNode, ChainableNode};

#[test]
fn test_audio_file_loading_integration() {
    // Test loading a real audio file
    let test_file = "../../tests/piano_freesound.wav";
    
    if std::path::Path::new(test_file).exists() {
        let reader = AudioReader::new(test_file).expect("Failed to load test audio file");
        let info = reader.info();
        
        // Verify expected properties
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channels, 2);
        assert!(info.duration.is_some());
        
        println!("✅ Audio file loaded: {} Hz, {} channels", info.sample_rate, info.channels);
    } else {
        panic!("Test audio file not found: {}", test_file);
    }
}

#[test]
fn test_audio_processing_integration() {
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
        println!("✅ Processed {} audio frames with gain node chain", frames_processed);
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
            
            // Process through chained nodes
            node_chain.process(&mut sample).unwrap();
            
            let processed_peak = sample.data.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
            
            // Should be close to original (within 1% due to floating point precision)
            assert!((processed_peak / original_peak - 1.0).abs() < 0.01);
            
            println!("✅ Chained node processing: original peak {:.3}, processed peak {:.3}", 
                original_peak, processed_peak);
        }
    }
}

#[test]
fn test_error_handling_integration() {
    // Test with non-existent file
    let result = AudioReader::new("nonexistent.wav");
    assert!(result.is_err());
    
    // Test with invalid file
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