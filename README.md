# AES67 Tools

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)](https://github.com/wiccy46/aes67-tools)
[![AES67 Compliant](https://img.shields.io/badge/AES67-oriented-blue)](https://www.aes.org/publications/standards/search.cfm?docID=96)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

Command-line tools for sending and receiving AES67-oriented RTP audio streams.

- `aes67-streamer` streams audio files as 48 kHz, 24-bit L24 RTP.
- `aes67-player` receives AES67 RTP streams and plays them through the local
  audio output device.
- `aes67-sap` browses AES67 streams announced with SAP and can write discovered
  SDP payloads to files.

The current release target is one stream with 1-8 channels on macOS and Linux.

## Install

### Homebrew (MacOS and Linux)

```bash
brew install wiccy46/aes67/aes67-tools
```

This installs all public binaries:

```bash
aes67-streamer --version
aes67-player --version
aes67-sap --version
```

### GitHub Release Archive

Download the archive for your platform from the GitHub release page, then unpack
it and put the `bin/` directory on your `PATH`.

Current release targets:

- `aarch64-apple-darwin` for Apple Silicon macOS.
- `x86_64-unknown-linux-gnu` for x86_64 Linux.

### Build From Source

```bash
git clone https://github.com/wiccy46/aes67-tools.git
cd aes67-tools
cargo build --release
```
On Linux, building the player requires ALSA development headers. On
Ubuntu/Debian:

```bash
sudo apt-get install libasound2-dev pkg-config
```

On Fedora:

```bash
sudo dnf install alsa-lib-devel pkgconf-pkg-config
```

## Stream Audio

Stream an audio file to a multicast address:

```bash
aes67-streamer \
  --file audio.wav \
  --address 239.69.83.1 \
  --port 5004 \
  --interface 192.168.1.100 \
  --sdp-output stream.sdp
```

The streamer decodes WAV, FLAC, MP3, and AIFF files, resamples to 48 kHz, and
sends L24 RTP packets. It can also announce the generated SDP over SAP.

To repeat the source file continuously:

```bash
aes67-streamer --file audio.wav --address 239.69.83.1 --loop
```

To stop after a bounded duration:

```bash
aes67-streamer --file audio.wav --address 239.69.83.1 --duration-seconds 30
```

## Receive Audio and Playback on Output Device

List output devices:

```bash
aes67-player --list-devices
aes67-player -L
```

Receive with basic address and port arguments:

```bash
aes67-player \
  --address 239.69.83.1 \
  --port 5004 \
  --interface 192.168.1.100
```

Receive from an SDP file:

```bash
aes67-player \
  --sdp stream.sdp \
  --interface 192.168.1.100
```

Select an output device by index or name from `--list-devices`:

```bash
aes67-player --sdp stream.sdp --output-device 0
aes67-player --sdp stream.sdp -o "Built-in Audio"
```

Set initial playout latency:

```bash
aes67-player --sdp stream.sdp --latency-ms 75
```

The player logs a final summary when it exits. Clean playback should report zero
for RTP silence frames, jitter lost/late/dropped-full packets, jitter timestamp
discontinuities, output silence frames, and output dropped samples.

## Browse SAP Announcements

Browse AES67 streams announced on the local SAP multicast group:

```bash
aes67-sap --interface 192.168.1.100
```

The browser prints one line when a stream is discovered, changed, removed, or
expired. Event markers follow the common browse convention: `+` for added, `=`
for updated, and `-` for removed or expired.

To exit after the first discovered AES67 SAP stream:

```bash
aes67-sap --interface 192.168.1.100 --once
```

To save discovered SDP payloads for `aes67-player`:

```bash
aes67-sap --interface 192.168.1.100 --sdp-output-dir discovered-sdp
aes67-player --sdp discovered-sdp/sap-192.168.1.50-1234.sdp --interface 192.168.1.100
```

## Streamer Configuration File

The streamer can load runtime settings from TOML:

```bash
aes67-streamer --config streamer.toml
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
aes67-streamer --config streamer.toml --port 6000
```

## CLI Reference

### `aes67-streamer`

| Option | Description | Default |
|--------|-------------|---------|
| `--config` | TOML configuration file path | None |
| `--file` | Audio file path; required unless set as `audio.file` in config | Required |
| `--address` | RTP destination address; required unless set as `stream.address` in config | Required |
| `--port` | RTP UDP port | `5004` |
| `--interface` | Local interface IP | Auto-detect |
| `--sdp-output` | Write generated SDP to a file before streaming | None |
| `--ptp-domain` | PTP domain number | `0` |
| `--duration-seconds` | Stop after a bounded duration | Unlimited |
| `--loop` | Repeat the audio file instead of stopping at end-of-file | `false` |
| `--verbose` | Enable verbose logging | `false` |

### `aes67-player`

| Option | Description | Default |
|--------|-------------|---------|
| `--sdp` | SDP file describing the AES67 stream | None |
| `--address` / `-a` | RTP destination address in basic receive mode | Required unless `--sdp` is set |
| `--port` / `-p` | RTP UDP port in basic receive mode | Required unless `--sdp` is set |
| `--interface` / `-i` | Local interface name or IPv4 address | `127.0.0.1` |
| `--sender` | Optional sender IPv4 address filter | None |
| `--channels` | Channel count for basic receive mode | `2` |
| `--payload-type` | RTP payload type for basic receive mode | `97` |
| `--latency-ms` | Initial playout latency | `50` |
| `--output-device` / `-o` | Output device index or name from `--list-devices` | Default output device |
| `--list-devices` / `-L` | List output devices and exit | - |
| `--duration-seconds` | Stop receiving after a bounded duration | Unlimited |
| `--verbose` / `-v` | Enable verbose logging | `false` |

### `aes67-sap`

| Option | Description | Default |
|--------|-------------|---------|
| `--interface` / `-i` | Local interface name or IPv4 address used for SAP multicast | Required |
| `--once` | Exit after the first discovered AES67 SAP stream | Continuous browse |
| `--sdp-output-dir` | Write discovered SDP payloads to this directory | None |
| `--verbose` / `-v` | Enable verbose logging | `false` |

## AES67 Defaults

- Sample rate: 48 kHz.
- Payload format: 24-bit PCM L24.
- Packet time: 1 ms, or 48 samples per packet at 48 kHz.
- Default payload type: 97.
- Recommended multicast range: `239.69.0.0/16`.
- Default PTP domain: 0.
- RTP DSCP: 34 / AF41.
- PTP DSCP: 46 / EF.
- SAP DSCP: 24 / CS3.

The streamer generates SDP from the loaded file and stream settings, logs it at
startup, can write it to a file with `--sdp-output` or `stream.sdp_output`, and
announces it over SAP by default. The SAP browser can discover those
announcements and write SDP files that the player can join with
`aes67-player --sdp stream.sdp`.

## Troubleshooting

### Stream Not Detected

Check multicast routing:

```bash
netstat -rn | grep 224
```

Make sure `--interface` is the local interface that should send or receive the
stream. For local testing, use `127.0.0.1` with a unicast address first.

### Monitor With ffplay Or VLC

Use the generated SDP file with VLC, or with ffplay:

```bash
ffplay -protocol_whitelist file,udp,rtp stream.sdp
```

### Capture RTP Or PTP Traffic

```bash
tcpdump -i eth0 -w capture.pcap host YOUR_INTERFACE_IP
tcpdump -i eth0 port 319
```

### PTP Permission Error On Linux

The streamer PTP client uses the standard PTP UDP ports `319` and `320`.
Binding ports below `1024` normally requires elevated privileges on Linux. RTP
streaming may still run, but PTP can log:

```text
PTP loop error: Permission denied (os error 13)
```

For a release build, grant the binary the required network capabilities:

```bash
cargo build --release -p aes67-streamer
sudo setcap cap_net_bind_service,cap_net_admin+ep target/release/aes67-streamer
```

`cap_net_bind_service` allows binding the PTP ports. `cap_net_admin` may be
needed for DSCP/TOS socket options. Reapply capabilities after rebuilding.

## Current Scope

This project targets the core pieces required for a first single-stream AES67
sender/player release:

- AES67-style RTP media: 48 kHz, 24-bit L24 payloads over RTP.
- AES67-style RTP receive: single-stream L24 receive, jitter buffering,
  SDP/basic-CLI configuration, and CPAL output.
- RFC 3550 RTP sequence numbers, timestamps, payload type, and SSRC.
- IEEE 1588-2008 PTPv2 message handling and local master fallback.
- SAP/SDP announcement and discovery through the dedicated `aes67-sap` browser.

The first release does not claim hard real-time scheduling, hardware clock
discipline, kernel-bypass networking, multiple simultaneous streams, or full ST
2110 system compliance. Player playout uses the local audio device clock in this
release; PTP-locked playout is future work.

## More Information

- `CHANGELOG.md` lists release changes.
- `DEV_README.md` covers development, testing, architecture, and release
  automation.
- `tests/receiver-compatibility.md` can be used to record receiver checks.
- `tests/timing-scheduling.md` documents the current timing and scheduling
  release stance.

## License

This project is licensed under the GNU General Public License v3.0.
