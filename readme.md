# AES67 Audio Streamer

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)](https://github.com/your-org/aes67-streamer)
[![AES67 Compliant](https://img.shields.io/badge/AES67-compliant-blue)](https://www.aes.org/publications/standards/search.cfm?docID=96)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

A cross-platform CLI tool for sending AES67-oriented RTP audio streams over IP networks.

## Features

### Audio Processing
- **Multi-format Support**: WAV, FLAC, MP3, and AIFF via [Symphonia](https://github.com/pdeljanov/Symphonia)
- **Sample Rate Conversion**: High-quality resampling with [Rubato](https://github.com/HEnquist/rubato)
- **Release Target**: Single stream with 1-8 channels
- **Real-time Processing**: Node-based audio pipeline with gain control

### Network Streaming
- **AES67-Oriented RTP**: 48 kHz, 24-bit L24 RTP streaming with generated SDP
- **RTP over UDP**: RFC 3550 compliant with proper sequence numbering
- **Multicast**: Standard administratively scoped multicast addresses
- **Packet Timing Metrics**: Reports packet rate, late packets, max lateness, and average late-packet lateness

### PTP Synchronization
- **IEEE 1588-2008 PTP Messages**: Announce, Sync, FollowUp, DelayReq, and DelayResp handling
- **Best Master Clock Selection**: Tracks candidate masters and selects a reference identity
- **Local Master Fallback**: Emits local PTP messages when no external grandmaster is present

## Quick Start

### Installation

```bash
# Clone the repository
git clone https://github.com/your-org/aes67-streamer.git
cd aes67-streamer

# Build the project
cargo build --release

# The binary will be available at target/release/aes67-streamer
```

### Basic Usage

```bash
# Stream an audio file to multicast address
./aes67-streamer \
  --file audio.wav \
  --address 239.69.83.1 \
  --port 5004 \
  --interface 192.168.1.100
```

### Advanced Usage

```bash
# Stream with a custom PTP domain and verbose logging
./aes67-streamer \
  --file audio.wav \
  --address 239.69.83.1 \
  --port 5004 \
  --interface 192.168.1.100 \
  --ptp-domain 0 \
  --verbose
```

### Config File Usage

The streamer can load runtime settings from a TOML config file:

```bash
./aes67-streamer --config streamer.toml
```

Example `streamer.toml`:

```toml
[audio]
file = "audio.wav"
loop = false
duration_seconds = 30
gain_db = 0.0

[stream]
name = "AES67 Stream"
address = "239.69.83.1"
port = 5004
interface = "192.168.1.100"
packet_time_ms = 1
payload_type = 97
ssrc = 305419896
ttl = 32
sap = true
ptp_domain = 0

[runtime]
verbose = false
```

CLI flags override config file values. For example, this uses the file,
address, and interface from `streamer.toml`, but streams on port `6000`:

```bash
./aes67-streamer --config streamer.toml --port 6000
```

To continuously repeat a source file, enable loop:

```bash
./aes67-streamer --file audio.wav --address 239.69.83.1 --loop
```

Looping is disabled by default. In TOML, use:

```toml
[audio]
loop = true
```

Keep the CLI for common startup choices. Use TOML for stream metadata and
audio-over-IP tuning:

```toml
[audio]
gain_db = -3.0

[stream]
payload_type = 97
ssrc = 305419896
packet_time_ms = 1
ttl = 32
```

The streamer generates SDP from the loaded file and stream settings and logs it
at startup. SDP file export can be added later without making users hand-write
SDP in the config.

The streamer applies fixed DSCP values as professional defaults for networks
that honor QoS marking:

```text
PTP  DSCP 46  EF    highest priority timing
RTP  DSCP 34  AF41  high priority media
SAP  DSCP 24  CS3   moderate control/discovery
```

PTP is marked highest because clock timing affects the whole stream. RTP audio
is elevated as latency-sensitive media. SAP is discovery metadata, so it should
not compete with timing or audio packets.

### Testing & Development
```bash
# Run unit and integration tests
cargo test --workspace

# Run full media loopback E2E test with ffmpeg/ffprobe
bash scripts/e2e_loopback.sh

# Run optional multicast validation on a real local interface
AES67_E2E_INTERFACE=192.168.1.100 bash scripts/e2e_multicast.sh

# Run a longer local release-candidate soak test
bash scripts/soak_loopback.sh

# Local testing with loopback
./aes67-streamer --file test.wav --address 127.0.0.1 --port 5004 --interface 127.0.0.1

# Monitor stream with VLC
Add the .sdp file to VLC, example SDP (testing locally with your interface):

v=0
o=- 123456 123456 IN IP4 127.0.0.1
s=AES67 Streamer
c=IN IP4 239.192.1.1/32
t=0 0
m=audio 5004 RTP/AVP 97
a=rtpmap:97 L24/48000/2
a=ptime:1
a=recvonly


# Use ffplay to receive the stream
ffplay -protocol_whitelist file,udp,rtp YOURSDPFILE.sdp

# Capture packets for analysis
tcpdump -i eth0 -w capture.pcap host YOURINTERFACE_IP
```

## 🛠️ Configuration

### Command Line Options

| Option | Description | Default | Example |
|--------|-------------|---------|---------|
| `--config` | TOML configuration file path | None | `streamer.toml` |
| `--file` | Audio file path; required unless set as `audio.file` in config | Required | `audio.wav` |
| `--address` | Multicast IP address; required unless set as `stream.address` in config | Required | `239.69.83.1` |
| `--port` | UDP port number | `5004` | `5004` |
| `--interface` | Network interface IP | Auto-detect | `192.168.1.100` |
| `--ptp-domain` | PTP domain number | `0` | `1` |
| `--duration-seconds` | Stop after a bounded duration | Unlimited | `2` |
| `--loop` | Repeat the audio file instead of stopping at end-of-file | `false` | - |
| `--verbose` | Enable verbose logging | `false` | - |

### AES67 Defaults
- **Sample Rate**: 48kHz (AES67 standard)
- **Packet Time**: 1ms (48 samples per packet)
- **Bit Depth**: 24-bit PCM
- **Recommended Multicast Range**: 239.69.0.0/16
- **PTP Domain**: 0 (default AES67 domain)
- **RTP QoS**: DSCP 34 / AF41
- **PTP QoS**: DSCP 46 / EF
- **SAP QoS**: DSCP 24 / CS3

### Release Validation

Before tagging a release candidate, run:

```bash
cargo test --workspace
bash scripts/e2e_loopback.sh
bash scripts/soak_loopback.sh
```

On a multicast-capable interface, also run:

```bash
AES67_E2E_INTERFACE=<local-ip> bash scripts/e2e_multicast.sh
```

Use `tests/receiver-compatibility.md` to record receiver checks with ffmpeg,
VLC, RAVENNA Stream Monitor, Dante AES67 mode, and Wireshark. Use
`tests/timing-scheduling.md` for the current timing/scheduling release stance.

### Project Structure
```
src/
├── aes67-streamer/     # Main binary crate
├── audio/              # Audio processing and samples vector
├── network/            # RTP and UDP multicast
├── ptp/                # IEEE 1588 PTP synchronization
└── config/             # CLI and configuration
```

### Contributing
1. Fork the repository
2. Create a feature branch
3. Make changes with tests
4. Submit a pull request

## Troubleshooting

### Common Issues

**Stream not detected**
```bash
# Check multicast routing
netstat -rn | grep 224
```

```bash
# Validate the selected interface before a multicast test
AES67_E2E_INTERFACE=<local-ip> bash scripts/e2e_multicast.sh --dry-run
```

**PTP synchronization**
```bash
# Check PTP traffic
tcpdump -i eth0 port 319

# Use verbose logging
--verbose
```

## License

This project is licensed under the GNU General Public License v3.0

## Compliance Target

This project targets the core pieces required for a first single-stream AES67
sender release:

- **AES67-style RTP media**: 48 kHz, 24-bit L24 payloads over RTP.
- **RFC 3550**: RTP sequence numbers, timestamps, payload type, and SSRC.
- **IEEE 1588-2008 PTPv2**: Basic message handling, BMCA selection, delay
  request/response, and local master fallback.
- **SAP/SDP discovery**: Generated SDP and SAP announcements.

The first release does not claim hard real-time scheduling, hardware clock
discipline, kernel-bypass networking, multiple simultaneous streams, or full
ST 2110 system compliance.
