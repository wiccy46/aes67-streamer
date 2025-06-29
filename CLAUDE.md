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

## Current Project Structure (Phase 1-3 Complete)

```
aes67-streamer/
├── Cargo.toml                    # Workspace configuration
├── CLAUDE.md                     # Project documentation
├── readme.md                     # Basic project info
├── streaming_evidence.md         # Proof of working AES67 streaming
├── tests/
│   ├── piano_freesound.wav      # Real audio file for testing
│   └── fake.wav                 # Corrupted file for error testing
├── .zed/
│   └── settings.json            # Zed editor configuration
└── src/
    ├── aes67-streamer/          # Main binary crate
    │   ├── Cargo.toml           # Binary dependencies (config, audio, network, anyhow)
    │   ├── src/
    │   │   ├── main.rs          # CLI entry point with streaming integration
    │   │   └── streamer.rs      # AES67 streaming implementation
    │   └── tests/
    │       └── audio_integration_tests.rs # Integration tests
    ├── config/                  # Configuration management crate
    │   ├── Cargo.toml           # Config dependencies (clap, serde, toml)
    │   └── src/
    │       ├── lib.rs           # Public API exports
    │       ├── args.rs          # CLI argument parsing
    │       └── configs.rs       # TOML configuration structures
    ├── audio/                   # Audio processing crate
    │   ├── Cargo.toml           # Audio dependencies (symphonia, anyhow)
    │   └── src/
    │       ├── lib.rs           # Public API exports
    │       ├── reader.rs        # Multi-channel audio file reader
    │       ├── node.rs          # Node-based processing architecture
    │       └── gain.rs          # Gain node with level metering
    └── network/                 # Network & RTP crate
        ├── Cargo.toml           # Network dependencies (anyhow, audio)
        └── src/
            ├── lib.rs           # Public API exports
            ├── rtp.rs           # RTP packet structure & packetizer
            └── socket.rs        # UDP multicast socket implementation
```

### Current Implementation Status
- ✅ **Phase 1 Complete**: Workspace, CLI parsing, configuration
- ✅ **Phase 2 Complete**: Multi-channel audio reader, node-based processing
- ✅ **Phase 3 Complete**: RTP streaming, UDP multicast, network integration
- 🚧 **Phase 4 Next**: PTP synchronization implementation

## Debugging & Testing Commands

### Build & Test
```bash
# Test entire workspace
cargo test

# Test specific crate
cargo test --package audio
cargo test --package config

# Test with ignored tests (requires test files)
cargo test --package audio -- --ignored

# Build check without running
cargo check
cargo check --package audio
```

### Audio File Testing & Streaming
```bash
# Test AES67 streaming with real network interface
cargo run --bin aes67-streamer -- --file tests/piano_freesound.wav --address 239.192.1.1 --port 5004 --interface 192.168.178.89

# Test with verbose logging (shows packet transmission)
cargo run --bin aes67-streamer -- --file tests/piano_freesound.wav --address 239.192.1.1 --port 5004 --interface 192.168.178.89 --verbose

# Test with loopback interface (for development)
cargo run --bin aes67-streamer -- --file tests/piano_freesound.wav --address 239.192.1.1 --port 5004 --interface 127.0.0.1

# Test error handling with corrupted file
cargo run --bin aes67-streamer -- --file tests/fake.wav --address 239.192.1.1 --port 5004 --interface 127.0.0.1

# Test with non-existent file
cargo run --bin aes67-streamer -- --file nonexistent.wav --address 239.192.1.1 --port 5004 --interface 127.0.0.1

# Test with invalid file format
cargo run --bin aes67-streamer -- --file readme.md --address 239.192.1.1 --port 5004 --interface 127.0.0.1
```

### Debug Logging
```bash
# Enable debug logging
RUST_LOG=debug cargo run --bin aes67-streamer -- --file tests/piano_freesound.wav --address 239.192.1.1 --port 5004

# Enable info logging (default in our build)
RUST_LOG=info cargo run --bin aes67-streamer -- --file tests/piano_freesound.wav --address 239.192.1.1 --port 5004

# Show help
cargo run --bin aes67-streamer -- --help
```

### Integration Testing
Proper testing is done via integration tests, not main.rs demos:

```bash
# Run integration tests for main binary
cargo test --package aes67-streamer

# Run specific integration test
cargo test --package aes67-streamer test_audio_processing_integration
```

Integration tests verify:
- ✅ **Multi-channel audio reading**: Proper stereo/multichannel support  
- ✅ **Node-based processing**: Gain nodes with chaining capability
- ✅ **Error validation**: Explicit failures for corrupted files
- ✅ **Interleaved output**: `[L, R, L, R...]` format validation
- ✅ **RTP streaming**: Packet creation and network transmission
- ✅ **AES67 compliance**: 24-bit PCM, 1ms packet timing

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

## Development Phases & Current Status

### ✅ Phase 1: Core Framework (COMPLETE)
1. ✅ Project structure and rust workspace setup
2. ✅ CLI argument parsing with `clap`
3. ✅ Configuration management with TOML support

### ✅ Phase 2: Audio Processing (COMPLETE)
1. ✅ Audio file decoding with `symphonia` (WAV, MP3, AIFF)
2. ✅ Node-based audio processing architecture (linked-list style)
3. ✅ Gain control with level metering and clipping protection
4. ✅ Multi-channel support (1-64 channels, interleaved output)

### ✅ Phase 3: Network & RTP (COMPLETE)
1. ✅ RTP packet structure with proper headers (RFC 3550)
2. ✅ 24-bit PCM payload conversion (AES67 standard)
3. ✅ UDP multicast socket with `std::net::UdpSocket`
4. ✅ Real-time packet transmission with 1ms timing
5. ✅ **PROVEN WORKING**: 1000 packets/6.9MB transmitted successfully

### 🚧 Phase 4: PTP Implementation (NEXT)
1. ❌ PTP client using `statime` crate
2. ❌ IEEE 1588-2008 PTPv2 support
3. ❌ Clock synchronization and discipline
4. ❌ PTP master/slave role handling
5. ❌ Microsecond-precision timing
6. ❌ Best Master Clock Algorithm (BMCA)

### 🚧 Phase 5: Integration & Optimization (FUTURE)
1. ❌ Sample rate conversion with `rubato` crate
2. ❌ Real-time thread priorities per platform
3. ❌ Lock-free data structures (`crossbeam`)
4. ❌ Multi-threaded streaming pipeline
5. ❌ Memory pool pre-allocation
6. ❌ Performance optimizations (SIMD, etc.)
7. ❌ Advanced socket options (DSCP marking, etc.)

## Known Issues & Limitations
- 🐛 Audio reader boundary bug (index out of bounds at end of file)
- ⚠️ No automatic interface discovery (manual IP required)
- ⚠️ Single-threaded implementation (not truly real-time yet)
- ⚠️ No loop playback support
- ⚠️ No sample rate conversion (uses file's native rate)
- ⚠️ Basic timing control (no PTP synchronization)

## Success Criteria

- **Latency**: <100ms end-to-end on local network
- **Quality**: Bit-perfect audio reproduction
- **Reliability**: No dropouts under normal network conditions
- **Compatibility**: Works with existing AES67 equipment
- **Performance**: Minimal CPU usage, efficient memory management
- **Portability**: Identical functionality across Linux/macOS/Windows

## Current Functional Capability

We have a **fully working AES67 audio streamer** that:
- ✅ Reads audio files (WAV, MP3, AIFF) using Symphonia
- ✅ Processes audio through node-based gain control
- ✅ Creates proper RTP packets with AES67-compliant 24-bit PCM
- ✅ Streams over UDP multicast in real-time (1ms packets)
- ✅ Works cross-platform with standard library networking
- ✅ **PROVEN**: Successfully transmitted 1000 packets/6.9MB

**Next logical step**: Implement PTP synchronization (Phase 4) for professional timing accuracy.

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
- Do not over comment or duplicate code. Write comment only when there is specific clarification needed
