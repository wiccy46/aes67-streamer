# AES67 Audio Streamer

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)](https://github.com/your-org/aes67-streamer)
[![AES67 Compliant](https://img.shields.io/badge/AES67-compliant-blue)](https://www.aes.org/publications/standards/search.cfm?docID=96)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

A cross-platform CLI tool for streaming audio files over IP networks with AES67 compliance.

## 🎵 Features

### Audio Processing
- **Multi-format Support**: WAV, MP3, AIFF via [Symphonia](https://github.com/pdeljanov/Symphonia)
- **Sample Rate Conversion**: High-quality resampling with [Rubato](https://github.com/HEnquist/rubato)
- **Multi-channel Audio**: 1-64 channels with efficient non-interleaved processing
- **Real-time Processing**: Node-based audio pipeline with gain control

### Network Streaming
- **AES67 Compliant**: Fully compliant with AES67-2018 standard
- **RTP over UDP**: RFC 3550 compliant with proper sequence numbering
- **Multicast**: Standard AES67 addressing (239.69.x.x range)
- **Low Latency**: 1ms packet timing with microsecond precision

### PTP Synchronization
- **IEEE 1588**: PTPv2 clock synchronization with [statime](https://github.com/pendulum-project/statime)
- **Timing Discipline**: Microsecond-accurate timestamps
- **Real-time Monitoring**: Live PTP state and offset tracking

## 🚀 Quick Start

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
# Stream with custom sample rate and verbose logging
./aes67-streamer \
  --file audio.wav \
  --address 239.69.83.1 \
  --port 5004 \
  --interface 192.168.1.100 \
  --ptp-domain 0 \
  --verbose
```

### Testing & Development
```bash
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
| `--file` | Audio file path | Required | `audio.wav` |
| `--address` | Multicast IP address | Required | `239.69.83.1` |
| `--port` | UDP port number | Required | `5004` |
| `--interface` | Network interface IP | Auto-detect | `192.168.1.100` |
| `--ptp-domain` | PTP domain number | `0` | `1` |
| `--verbose` | Enable verbose logging | `false` | - |

### AES67 Defaults
- **Sample Rate**: 48kHz (AES67 standard)
- **Packet Time**: 1ms (48 samples per packet)
- **Bit Depth**: 24-bit PCM
- **Multicast Range**: 239.69.0.0/16
- **PTP Domain**: 0 (default AES67 domain)

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

## 🔍 Troubleshooting

### Common Issues

**Stream not detected**
```bash
# Check multicast routing
netstat -rn | grep 224

**PTP synchronization**
```bash
# Check PTP traffic
tcpdump -i eth0 port 319

# Use verbose logging
--verbose
```

## 📄 License

This project is licensed under the GNU General Public License v3.0

## 🏆 Compliance

This implementation is fully compliant with:
- **AES67-2018**: High-performance streaming audio-over-IP interoperability standard
- **RFC 3550**: RTP: A Transport Protocol for Real-Time Applications
- **IEEE 1588-2008**: Precision Time Protocol (PTPv2)
- **SMPTE ST 2110**: Professional Media Over Managed IP Networks
