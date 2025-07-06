use crate::{AudioReader, AudioSample, AudioNodeChain, ResamplerQuality, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use anyhow::Context;

/// Configuration for the audio processing pipeline
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Target sample rate for streaming
    pub target_sample_rate: u32,
    /// Audio buffer size in frames (per channel)
    pub audio_buffer_frames: usize,
    /// RTP packet queue size
    pub rtp_queue_size: usize,
    /// Network thread priority (0-3, 3 = highest)
    pub network_priority: u8,
    /// Audio thread priority (0-3, 3 = highest)  
    pub audio_priority: u8,
    /// Enable verbose logging
    pub verbose: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            target_sample_rate: 48000,
            audio_buffer_frames: 1024, // ~21ms at 48kHz
            rtp_queue_size: 128,
            network_priority: 3, // High priority for network
            audio_priority: 3,   // High priority for audio
            verbose: false,
        }
    }
}

/// Audio sample with metadata for pipeline processing
#[derive(Debug, Clone)]
pub struct PipelineSample {
    /// Audio data
    pub sample: AudioSample,
    /// Timestamp when sample was read
    pub timestamp: Instant,
    /// Sequence number for ordering
    pub sequence: u64,
}

/// Multi-threaded audio processing pipeline
pub struct AudioPipeline {
    /// Audio thread handle
    audio_thread: Option<JoinHandle<Result<()>>>,
    /// Processing thread handle  
    processing_thread: Option<JoinHandle<Result<()>>>,
    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
    /// Configuration
    config: PipelineConfig,
}

impl AudioPipeline {
    /// Create new streaming pipeline
    pub fn new(
        audio_file: &str,
        resampler_quality: ResamplerQuality,
        audio_chain: AudioNodeChain,
        config: PipelineConfig,
    ) -> Result<(Self, Receiver<PipelineSample>)> {
        let shutdown = Arc::new(AtomicBool::new(false));
        
        // Create lock-free channels for audio pipeline
        let (audio_tx, audio_rx) = bounded::<PipelineSample>(config.audio_buffer_frames);
        let (processed_tx, processed_rx) = bounded::<PipelineSample>(config.rtp_queue_size);
        
        // Start audio reading thread
        let audio_thread = Self::spawn_audio_thread(
            audio_file.to_string(),
            config.target_sample_rate,
            resampler_quality,
            audio_tx,
            shutdown.clone(),
            config.clone(),
        )?;
        
        // Start audio processing thread
        let processing_thread = Self::spawn_processing_thread(
            audio_rx,
            processed_tx,
            audio_chain,
            shutdown.clone(),
            config.clone(),
        )?;
        
        Ok((
            Self {
                audio_thread: Some(audio_thread),
                processing_thread: Some(processing_thread),
                shutdown,
                config,
            },
            processed_rx,
        ))
    }
    
    /// Spawn audio reading thread (high priority)
    fn spawn_audio_thread(
        audio_file: String,
        target_sample_rate: u32,
        quality: ResamplerQuality,
        sender: Sender<PipelineSample>,
        shutdown: Arc<AtomicBool>,
        config: PipelineConfig,
    ) -> Result<JoinHandle<Result<()>>> {
        let handle = thread::Builder::new()
            .name("audio-reader".to_string())
            .spawn(move || -> Result<()> {
                // Set thread priority if possible (platform-specific)
                #[cfg(target_os = "linux")]
                Self::set_thread_priority(config.audio_priority);
                
                // Load audio file with resampling
                let mut reader = AudioReader::with_resampling(
                    &audio_file,
                    Some(target_sample_rate),
                    quality,
                ).context("Failed to load audio file in audio thread")?;
                
                let mut sequence = 0u64;
                
                log::info!("Audio thread started: reading from {}", audio_file);
                
                while !shutdown.load(Ordering::Relaxed) {
                    let start_time = Instant::now();
                    
                    match reader.read_next_frame()? {
                        Some(sample) => {
                            let pipeline_sample = PipelineSample {
                                sample,
                                timestamp: start_time,
                                sequence,
                            };
                            
                            // Try to send, but don't block if queue is full
                            match sender.try_send(pipeline_sample) {
                                Ok(_) => {
                                    sequence += 1;
                                    if config.verbose && sequence % 1000 == 0 {
                                        log::debug!("Audio thread: read {} frames", sequence);
                                    }
                                }
                                Err(crossbeam_channel::TrySendError::Full(_)) => {
                                    log::warn!("Audio buffer full, dropping frame {}", sequence);
                                    sequence += 1;
                                }
                                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                    log::info!("Audio thread: receiver disconnected");
                                    break;
                                }
                            }
                        }
                        None => {
                            log::info!("Audio thread: end of file reached");
                            break;
                        }
                    }
                    
                    // Yield to prevent busy waiting
                    thread::yield_now();
                }
                
                log::info!("Audio thread finished");
                Ok(())
            })
            .context("Failed to spawn audio thread")?;
        
        Ok(handle)
    }
    
    /// Spawn audio processing thread (high priority)
    fn spawn_processing_thread(
        receiver: Receiver<PipelineSample>,
        sender: Sender<PipelineSample>,
        mut audio_chain: AudioNodeChain,
        shutdown: Arc<AtomicBool>,
        config: PipelineConfig,
    ) -> Result<JoinHandle<Result<()>>> {
        let handle = thread::Builder::new()
            .name("audio-processor".to_string())
            .spawn(move || -> Result<()> {
                // Set thread priority if possible (platform-specific)
                #[cfg(target_os = "linux")]
                Self::set_thread_priority(config.audio_priority);
                
                log::info!("Audio processing thread started");
                let mut processed_count = 0u64;
                
                while !shutdown.load(Ordering::Relaxed) {
                    match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(mut pipeline_sample) => {
                            // Process audio through the chain
                            audio_chain.process(&mut pipeline_sample.sample)
                                .context("Failed to process audio in processing thread")?;
                            
                            // Forward processed sample
                            match sender.try_send(pipeline_sample) {
                                Ok(_) => {
                                    processed_count += 1;
                                    if config.verbose && processed_count % 1000 == 0 {
                                        log::debug!("Processing thread: processed {} frames", processed_count);
                                    }
                                }
                                Err(crossbeam_channel::TrySendError::Full(_)) => {
                                    log::warn!("RTP queue full, dropping processed frame");
                                }
                                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                    log::info!("Processing thread: receiver disconnected");
                                    break;
                                }
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            // Continue checking shutdown flag
                            continue;
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            log::info!("Processing thread: sender disconnected");
                            break;
                        }
                    }
                }
                
                log::info!("Processing thread finished: processed {} frames", processed_count);
                Ok(())
            })
            .context("Failed to spawn processing thread")?;
        
        Ok(handle)
    }
    
    /// Set thread priority (Linux specific implementation)
    #[cfg(target_os = "linux")]
    fn set_thread_priority(priority: u8) {
        use std::os::unix::thread::JoinHandleExt;
        
        // Map priority 0-3 to nice values (-19 to 0)
        let nice_value = match priority {
            3 => -19, // Highest priority
            2 => -10,
            1 => -5,
            _ => 0,   // Normal priority
        };
        
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, nice_value);
        }
        
        log::debug!("Set thread priority to nice value: {}", nice_value);
    }
    
    /// Placeholder for other platforms
    #[cfg(not(target_os = "linux"))]
    fn set_thread_priority(_priority: u8) {
        // Platform-specific implementation would go here
        log::debug!("Thread priority setting not implemented for this platform");
    }
    
    /// Shutdown the streaming pipeline gracefully
    pub fn shutdown(&mut self) -> Result<()> {
        log::info!("Shutting down streaming pipeline...");
        
        // Signal shutdown
        self.shutdown.store(true, Ordering::Relaxed);
        
        // Wait for threads to finish
        if let Some(audio_thread) = self.audio_thread.take() {
            audio_thread.join()
                .map_err(|_| anyhow::anyhow!("Audio thread panicked"))?
                .context("Audio thread error")?;
        }
        
        if let Some(processing_thread) = self.processing_thread.take() {
            processing_thread.join()
                .map_err(|_| anyhow::anyhow!("Processing thread panicked"))?
                .context("Processing thread error")?;
        }
        
        log::info!("Streaming pipeline shutdown complete");
        Ok(())
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_default() {
        let config = PipelineConfig::default();
        assert_eq!(config.target_sample_rate, 48000);
        assert_eq!(config.audio_buffer_frames, 1024);
        assert_eq!(config.rtp_queue_size, 128);
        assert_eq!(config.network_priority, 3);
        assert_eq!(config.audio_priority, 3);
        assert!(!config.verbose);
    }

    #[test]
    fn test_pipeline_sample_creation() {
        use crate::AudioSample;
        
        let sample = AudioSample {
            data: vec![0.1, 0.2, 0.3, 0.4],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        
        let pipeline_sample = PipelineSample {
            sample: sample.clone(),
            timestamp: Instant::now(),
            sequence: 42,
        };
        
        assert_eq!(pipeline_sample.sample.data, sample.data);
        assert_eq!(pipeline_sample.sequence, 42);
    }
}