# AES67 Audio Streamer - Project Implementation

## Project Overview

Build a cross-platform (Linux/macOS/Windows) CLI tool in Rust for streaming audio files over RTP networks on multicast with AES67 copliance.

## Core Requirements

### Audio Processing
- **File formats**: WAV, MP3, AIFF support via `symphonia` crate
- **Sample rates**: Multiple rates with conversion via `rubato` crate
- **Channels**: Support 1-64 channels (typically 2 or 8)
- **Bit depth**: 16/24/32-bit support
- **Real-time processing**: For now no latency requirements, we will focus on stability and reliability, before opitmising for real-time.

### Network Streaming
- **Protocol**: RTP over UDP multicast (AES67 compliant)
- **Packet format**: Proper RTP headers with sequence numbers and timestamps
- **PTP Clock Synchronization**: Discover and sync with existing PTP master using `statime` crate, timestamp should be accurate and synchronized with PTP master clock
- **Real-time transmission**: Online packetization during streaming (NOT pre-buffering)
- **Network interface**: Support streaming over wlan or ethernet.
- **Network discovery**: For the first step: user provide interface name, future to implement interface dicovery.

### PTP Clock Synchronization
- **PTP client**: Discover and sync with existing PTP master using `statime` crate
- **PTP master fallback**: Become PTP master if none found in network
- **Clock discipline**: Maintain microsecond-precision timing
- **AES67 compliance**: IEEE 1588-2008 PTPv2 support

### Cross-Platform Support
- **Platforms**: Linux, macOS, Windows with platform-specific optimizations. But first focus on macOS.
- **Network interfaces**: Platform-specific interface discovery and configuration
- **High-resolution timing**: Platform-specific clock implementations
- **Real-time scheduling**: Best-effort RT thread priorities per platform

## System Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                   Platform Abstraction Layer                │
│          (handles OS-specific networking & timing)          │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                      CLI Interface                          │
│                    (clap + config)                         │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                  Main Controller                            │
│              (orchestrates all modules)                     │
└─────┬─────────┬─────────┬─────────┬─────────┬──────────────┘
      │         │         │         │         │
      ▼         ▼         ▼         ▼         ▼
┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────────┐
│ Audio   │ │   PTP   │ │   RTP   │ │Platform │ │   Clock     │
│Decoder  │ │ Client  │ │Streamer │ │Network  │ │ Discipline  │
│         │ │         │ │         │ │Manager  │ │             │
└─────────┘ └─────────┘ └─────────┘ └─────────┘ └─────────────┘
```

## Threading Model & Data Flow

```
Audio File → Decoder → [Sample Buffer] → RTP Packetizer → [Packet Queue] → Network
                         ↑                    ↑               ↑
                    Audio Thread        RTP Thread      Network Thread
                   (High Priority)   (High Priority)  (RT Priority)
```

### Thread Responsibilities
1. **Audio Thread**: File reading, decoding, sample rate conversion, channel mapping
2. **RTP Thread**: PTP timestamp acquisition, RTP packet creation, timing control
3. **Network Thread**: UDP multicast transmission, socket management
4. **PTP Thread**: Clock synchronization, master/slave logic


## Key Implementation Details

### Multi-Layer Buffering Strategy
- **Audio Sample Buffer**: 1s for dropout protection, use ring buffer (user `ringbuf` crate)
- **RTP Packet Queue**: First try with a generous size.
- **Network Socket Buffers**: OS-managed UDP send buffers
- **Lock-free queues**: Use `crossbeam` for thread communication

### timestamp

The timestmap in the RTP packet should be based on sample. So the timestamp increment becomes the frame increment. e.g. ts1 = 0, ts2 = 8. These two timestamps difference are 8 frames in the interleaved audio stream.

### Platform Abstraction
```rust
// Network interface trait
pub trait PlatformNetwork {
    fn discover_interfaces(&self) -> Result<Vec<NetworkInterface>>;
    fn setup_multicast_socket(&self, config: &MulticastConfig) -> Result<UdpSocket>;
    fn set_socket_priority(&self, socket: &UdpSocket, priority: u8) -> Result<()>;
}

// Timing interface
pub trait PlatformClock {
    fn now_nanos() -> Result<u64>;
    fn sleep_until(target_nanos: u64) -> Result<()>;
    fn set_thread_priority(priority: ThreadPriority) -> Result<()>;
}
```

### CLI Interface Design
```bash
# Basic usage
aes67-streamer --file audio.wav --address 239.192.1.1 --port 5004 --interface eth0

# Advanced usage
aes67-streamer \
  --file audio.wav \
  --address 239.192.1.1 \
  --port 5004 \
  --interface eth0 \
  --sample-rate 48000 \
  --ptp-domain 0 \
  --config config.toml \
  --verbose
```

### Configuration File Format (Implement in the future. not during prototype)
```toml
[audio]
file_path = "audio.wav"
sample_rate = 48000
channels = 2
bit_depth = 24
loop_playback = false

[network]
multicast_address = "239.192.1.1"
port = 5004
interface = "auto"  # auto-detect or specify by name/IP
ttl = 32
buffer_size = 65536

[rtp]
payload_type = 97
ssrc = 0x12345678
session_name = "AES67 Stream"
packet_time_us = 1000  # 1ms packets

[ptp]
domain = 0
priority1 = 128
priority2 = 128
announce_interval_ms = 1000
sync_interval_ms = 125
clock_source = "auto"

[performance]
enable_rt_scheduling = true
audio_thread_priority = "high"
network_thread_priority = "high"
buffer_size_ms = 15
```

## Critical Implementation Requirements

### Real-Time Performance
- Use **lock-free data structures** (`crossbeam::queue::SegQueue`)
- Implement **real-time thread priorities** per platform
- **Pre-allocate memory pools** for RTP packets
- **Avoid heap allocation** in audio processing threads

### PTP Synchronization
- Use `statime` crate for IEEE 1588-2008 PTPv2 implementation
- Implement **best master clock algorithm** (BMCA)
- Handle **master/slave role transitions** gracefully
- Maintain **microsecond-precision** timing discipline

### Network Optimization
- Implement **platform-specific socket options** (SO_REUSEADDR, SO_REUSEPORT, etc.)
- Use **DSCP marking** for QoS (AF41 for audio, EF for PTP)
- Handle **multicast group management** properly
- Implement **graceful error handling** for network issues

### Audio Processing
- Support **multiple audio formats** via `symphonia`
- Implement **sample rate conversion** with `rubato`
- Handle **channel mapping** for 1-64 channels
- Provide **bit-perfect audio quality**

## Testing Strategy

### Unit Tests
- Audio decoding and processing
- RTP packet creation and validation
- PTP message handling
- Platform-specific network functions

### Integration Tests
- End-to-end streaming scenarios
- PTP synchronization with external clocks
- Multi-platform compatibility
- Network failure recovery

### Performance Tests
- Latency measurements
- Dropout detection under load
- Memory allocation profiling
- CPU usage optimization

## Development Phases

### Phase 1: Core Framework
1. Project structure and rust workspace steup
2. CLI argument parsing with `clap`
3. Configuration management

### Phase 2: Audio Processing
1. Audio file decoding with `symphonia`
2. Sample rate conversion with `rubato`
3. Multi-threaded audio buffering
4. Lock-free data structures

### Phase 3: Network & RTP
1. Platform-specific network interface discovery
2. Multicast socket setup and configuration
3. RTP packet creation and transmission
4. Network thread implementation

### Phase 4: PTP Implementation
1. PTP client with `statime` crate
2. Clock synchronization and discipline
3. PTP master fallback logic
4. Timing precision optimization

### Phase 5: Integration & Optimization
1. End-to-end streaming pipeline
2. Real-time performance tuning
3. Cross-platform testing
4. Documentation and examples

## Success Criteria

- **Latency**: <100ms end-to-end on local network
- **Quality**: Bit-perfect audio reproduction
- **Reliability**: No dropouts under normal network conditions
- **Compatibility**: Works with existing AES67 equipment
- **Performance**: Minimal CPU usage, efficient memory management
- **Portability**: Identical functionality across Linux/macOS/Windows

## Additional Notes

- Use rust workspace.
- Always implement a small piece of code at a time.
- Follow AES67 standard specifications precisely
- Prioritize real-time performance over feature completeness
- Use industry-standard crates where possible
- Design for extensibility (future features like multiple streams)
- Document all platform-specific behaviors
- Provide clear error messages and debugging information
- Be brave to disagree with the coder, provide constructive feedback.
- Reach good practice on the internet.
