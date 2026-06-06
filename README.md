# AES67 Streamer and Player

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)](https://github.com/your-org/aes67-streamer)
[![AES67 Compliant](https://img.shields.io/badge/AES67-compliant-blue)](https://www.aes.org/publications/standards/search.cfm?docID=96)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

CLI tools for sending and receiving AES67-oriented RTP audio streams over IP
networks. `aes67-streamer` sends 48 kHz L24 RTP audio, and `aes67-player`
receives AES67 RTP into CPAL audio output.

## Features

### Audio Handling for File Streaming
- **Multi-format Support**: WAV, FLAC, MP3, and AIFF via [Symphonia](https://github.com/pdeljanov/Symphonia)
- **Sample Rate Conversion**: Resampling with [Rubato](https://github.com/HEnquist/rubato)
- **Release Target**: Single stream with 1-8 channels (Multi-stream support to come in future releases)
- **Real-time Processing**: Node-based audio pipeline with gain control

### Network Streaming
- **AES67-Oriented RTP**: 48 kHz, 24-bit L24 RTP streaming with generated SDP and optional SDP file export
- **RTP over UDP**: RFC 3550 compliant with proper sequence numbering
- **Multicast**: Standard administratively scoped multicast addresses
- **SAP Announcements**: Announces generated SDP over SAP by default, configurable with `stream.sap`, stream discoverable by other AES67 devices
- **Packet Timing Metrics**: Reports packet rate, late packets, max lateness, and average late-packet lateness

### AES67 Player
- **CPAL Playback**: Native audio output through CPAL on Linux and macOS first (Windows in the future)
- **Basic Receive Mode**: Receive with address, port, interface, payload type, and channel count
- **SDP Receive Mode**: Parse stream address, port, payload type, L24 format, packet time, and clock metadata from SDP
- **Device Selection**: List devices with `--list-devices` / `-L` and select one with `--output-device` / `-o`
- **Dropout Visibility**: Final summary and warning logs for RTP silence, jitter loss, late packets, discontinuities, output silence, and output drops

### PTP Synchronization
- **IEEE 1588-2008 PTP Messages**: Announce, Sync, FollowUp, DelayReq, and DelayResp handling
- **Best Master Clock Selection**: Tracks candidate masters and selects a reference identity
- **Local Master Fallback**: Emits local PTP messages when no external grandmaster is present

## Quick Start

### Installation

To build from source:

```bash
# Clone the repository
git clone https://github.com/your-org/aes67-streamer.git
cd aes67-streamer

# Build the project
cargo build --release

# Binaries will be available at target/release/aes67-streamer
# and target/release/aes67-player.
```

### Versioning

The public binary version follows SemVer and is recorded in the root `VERSION`
file. Release changes must keep `VERSION` and
`src/aes67-streamer/Cargo.toml` in sync; tests validate both the SemVer format
and the version match.

### Basic Usage

```bash
# Stream an audio file to multicast address
./aes67-streamer \
  --file audio.wav \
  --address 239.69.83.1 \
  --port 5004 \
  --interface 192.168.1.100 \
  --sdp-output stream.sdp
```

### Player Usage

List audio output devices:

```bash
./aes67-player --list-devices
./aes67-player -L
```

Receive a stream with basic CLI arguments:

```bash
./aes67-player \
  --address 239.69.83.1 \
  --port 5004 \
  --interface 192.168.1.100
```

Receive from an SDP file:

```bash
./aes67-player \
  --sdp stream.sdp \
  --interface 192.168.1.100
```

Select a CPAL output device by index or name from `-L`:

```bash
./aes67-player --sdp stream.sdp --output-device 0
./aes67-player --sdp stream.sdp -o "Built-in Audio"
```

Set initial playout latency:

```bash
./aes67-player --sdp stream.sdp --latency-ms 75
```

The player logs a final summary when it exits. Clean playback should report zero
for RTP silence frames, jitter lost/late/dropped-full packets, jitter timestamp
discontinuities, output silence frames, and output dropped samples. Warning logs
from the player should be treated as playback smoothness issues to investigate.

On Linux, CPAL's ALSA backend requires the ALSA development package at build
time. On Fedora:

```bash
sudo dnf install alsa-lib-devel
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
sdp_output = "stream.sdp"
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

The streamer generates SDP from the loaded file and stream settings, logs it at
startup, and can write it to a file with `--sdp-output` or `stream.sdp_output`.
The player can then join the stream with `aes67-player --sdp stream.sdp`.

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

# Run player null-output E2E tests
cargo test -p aes67-player --test e2e

# Run a longer player receive soak without an audio device
bash scripts/player_soak_loopback.sh

# Run real CPAL playback validation on a clocked output device
bash scripts/player_cpal_loopback.sh

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
| `--sdp-output` | Write generated SDP to a file before streaming | None | `stream.sdp` |
| `--ptp-domain` | PTP domain number | `0` | `1` |
| `--duration-seconds` | Stop after a bounded duration | Unlimited | `2` |
| `--loop` | Repeat the audio file instead of stopping at end-of-file | `false` | - |
| `--verbose` | Enable verbose logging | `false` | - |

### Player Command Line Options

| Option | Description | Default | Example |
|--------|-------------|---------|---------|
| `--sdp` | SDP file describing the AES67 stream | None | `stream.sdp` |
| `--address` / `-a` | RTP destination address to receive in basic CLI mode | Required unless `--sdp` is set | `239.69.83.1` |
| `--port` / `-p` | RTP UDP port to receive in basic CLI mode | Required unless `--sdp` is set | `5004` |
| `--interface` / `-i` | Network interface name or IPv4 address | `127.0.0.1` | `192.168.1.100` |
| `--sender` | Optional sender IPv4 address filter | None | `192.168.1.20` |
| `--channels` | Channel count for basic CLI mode | `2` | `8` |
| `--payload-type` | RTP payload type for basic CLI mode | `97` | `101` |
| `--latency-ms` | Initial playout latency | `50` | `75` |
| `--output-device` / `-o` | CPAL output device index or name from `--list-devices` | Default output device | `0` |
| `--list-devices` / `-L` | List audio output devices and exit | - | - |
| `--duration-seconds` | Stop receiving after a bounded duration | Unlimited | `10` |
| `--verbose` / `-v` | Enable verbose logging | `false` | - |

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
cargo test -p aes67-player
cargo check --release -p aes67-player
cargo build --release -p aes67-player
bash scripts/e2e_loopback.sh
bash scripts/player_soak_loopback.sh
bash scripts/soak_loopback.sh
```

Build distributable release archives:

```bash
bash scripts/package_release.sh
```

The package script builds both public binaries in release mode, creates
`target/release-packages/aes67-tools-<version>-<target>.tar.gz`, and writes a
matching `.sha256` checksum file. Use `--dry-run` to verify the package name,
target triple, output path, and archive layout without building.

The tarball contains:

- `bin/aes67-streamer`
- `bin/aes67-player`
- `README.md`, `LICENSE`, and `VERSION`
- example streamer TOML and SDP files under `examples/`

These archives are the input for package-manager metadata. Homebrew can point a
formula at the tarball URL and checksum. Debian/apt packaging can install the
same binaries and docs into the standard filesystem layout.

The Homebrew formula template lives at `packaging/homebrew/aes67-tools.rb`.
After uploading release archives, replace its `REPLACE_WITH_*_SHA256`
placeholders with the matching values from `target/release-packages/*.sha256`.

Check the player release CLI surface:

```bash
cargo run --release -p aes67-player -- --help
cargo run --release -p aes67-player -- --version
cargo run --release -p aes67-player -- -L
target/release/aes67-player --address 127.0.0.1 --port 5004 --test-null-output
```

The final command should fail in release builds because the internal null output
is test-only.

For audible playback validation, choose a real clocked output device from `-L`
and run:

```bash
AES67_PLAYER_OUTPUT_DEVICE=<index-or-name> bash scripts/player_cpal_loopback.sh
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
├── aes67-player/       # AES67 RTP receiver and CPAL player
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

**PTP loop reports `Permission denied` on Linux**

The streamer PTP client uses the standard PTP UDP ports `319` and `320`.
Binding ports below `1024` normally requires elevated privileges on Linux. The
RTP audio stream may still run, but PTP will log an error similar to:

```text
PTP loop error: Permission denied (os error 13)
```

Build the binary, grant the required network capabilities, and run the binary
directly:

```bash
cargo build -p aes67-streamer
sudo setcap cap_net_bind_service,cap_net_admin+ep target/debug/aes67-streamer

target/debug/aes67-streamer \
  --file src/aes67-streamer/tests/resources/audio-formats/tone.wav \
  --address 127.0.0.1 \
  --interface 127.0.0.1 \
  --port 55210 \
  --duration-seconds 10
```

For release builds:

```bash
cargo build --release -p aes67-streamer
sudo setcap cap_net_bind_service,cap_net_admin+ep target/release/aes67-streamer
```

`cap_net_bind_service` allows binding the PTP ports. `cap_net_admin` may be
needed for DSCP/TOS socket options. Reapply the capabilities after rebuilding
the binary.

## License

This project is licensed under the GNU General Public License v3.0

## Compliance Target

This project targets the core pieces required for a first single-stream AES67
sender/player release:

- **AES67-style RTP media**: 48 kHz, 24-bit L24 payloads over RTP.
- **AES67-style RTP receive**: Single-stream L24 receive, jitter buffering,
  SDP/basic-CLI configuration, and CPAL output.
- **RFC 3550**: RTP sequence numbers, timestamps, payload type, and SSRC.
- **IEEE 1588-2008 PTPv2**: Basic message handling, BMCA selection, delay
  request/response, and local master fallback.
- **SAP/SDP discovery**: Generated SDP and SAP announcements.

The first release does not claim hard real-time scheduling, hardware clock
discipline, kernel-bypass networking, multiple simultaneous streams, or full
ST 2110 system compliance. Player playout uses the local audio device clock in
this release; PTP-locked playout is future work.
