# Tests for AES67 Streamer

## Running locally

```bash
cargo test --workspace
bash scripts/e2e_loopback.sh
```

## Test Tiers

| Tier | Command | Purpose | CI |
| --- | --- | --- | --- |
| Unit and integration | `cargo test --workspace` | Fast Rust coverage for audio, config, RTP, PTP, socket behavior, and script dry-run validation | Yes |
| Media loopback | `bash scripts/e2e_loopback.sh` | Full streamer-to-ffmpeg RTP loopback with decoded WAV validation | Yes |
| Multicast validation | `AES67_E2E_INTERFACE=<local-ip> bash scripts/e2e_multicast.sh` | Opt-in multicast group join and receive validation on a real interface | No |
| Player null-output E2E | `cargo test -p aes67-player --test e2e` | Streamer-to-player RTP validation without requiring an audio device | Yes |
| Player null-output soak | `bash scripts/player_soak_loopback.sh` | Longer streamer-to-player RTP receive soak without requiring an audio device | No |
| Player CPAL loopback | `bash scripts/player_cpal_loopback.sh` | Streamer-to-player validation using real CPAL audio output | No |
| Soak validation | `bash scripts/soak_loopback.sh` | Longer local release-candidate run using the media loopback path | No |
| Receiver compatibility | `tests/receiver-compatibility.md` | Manual/pro-tool matrix for VLC, RAVENNA Stream Monitor, Dante AES67 mode, and Wireshark | No |

## Full Media Loopback E2E

`scripts/e2e_loopback.sh` is the local and CI entry point for end-to-end media testing. It:

- Builds `aes67-streamer`.
- Generates a short 48 kHz stereo WAV fixture.
- Starts `ffmpeg` as an RTP receiver from an SDP file.
- Runs the streamer for a bounded duration on loopback.
- Validates the recorded WAV with `ffprobe`.

Requirements:

- `ffmpeg`
- `ffprobe`
- Local UDP loopback networking

## AES67 Player Validation

The player receives AES67 RTP and plays through CPAL. For deterministic CI-style
validation, the player E2E test uses an internal null output sink and verifies
the RTP decode, jitter-buffer, packet-timing, and output-summary counters:

```bash
cargo test -p aes67-player --test e2e
```

The player supports both basic receive arguments and SDP files:

```bash
cargo run -p aes67-player -- \
  --address 239.192.1.1 \
  --port 5004 \
  --interface 127.0.0.1

cargo run -p aes67-player -- \
  --sdp tests/example.sdp \
  --interface 127.0.0.1
```

To inspect and select audio devices:

```bash
cargo run -p aes67-player -- -L
cargo run -p aes67-player -- --sdp tests/example.sdp -o 0
```

On Linux, CPAL's ALSA backend requires the ALSA development package at build
time. On Fedora:

```bash
sudo dnf install alsa-lib-devel
```

For real playback validation, run the CPAL loopback script:

```bash
bash scripts/player_cpal_loopback.sh
```

Use `AES67_PLAYER_OUTPUT_DEVICE` to select a device by index or name from `-L`:

```bash
AES67_PLAYER_OUTPUT_DEVICE=<index-or-name> bash scripts/player_cpal_loopback.sh
```

Use a real clocked playback device for smoothness validation. ALSA's `null` /
discard device is useful for checking CPAL startup, but it can consume samples
faster than wall-clock time and should not be treated as a dropout-free playback
test. The loopback script rejects known null/discard devices by default; set
`AES67_PLAYER_ALLOW_UNCLOCKED_OUTPUT=1` only when intentionally running a
diagnostic startup check.

The player logs a final summary. Clean playback should report zero for RTP
silence frames, jitter lost/late/dropped-full packets, timestamp
discontinuities, output silence frames, and output dropped samples. Any warning
line from the player should be treated as a dropout or smoothness issue to
investigate before release.

For longer CI-safe receive validation, run the player soak loopback:

```bash
bash scripts/player_soak_loopback.sh
```

The default duration is 60 seconds. To change it:

```bash
AES67_PLAYER_SOAK_DURATION_SECONDS=300 bash scripts/player_soak_loopback.sh
```

## Optional Multicast E2E

`scripts/e2e_multicast.sh` validates that ffmpeg can join the selected multicast
group and record the RTP stream. It requires an explicit local interface IPv4:

```bash
AES67_E2E_INTERFACE=192.168.1.100 bash scripts/e2e_multicast.sh
```

Use `--dry-run` to verify the selected address, port, interface, and artifact
directory without starting the streamer:

```bash
AES67_E2E_INTERFACE=192.168.1.100 bash scripts/e2e_multicast.sh --dry-run
```

## Soak Validation

`scripts/soak_loopback.sh` runs the media loopback path for a longer duration.
The default is 300 seconds:

```bash
bash scripts/soak_loopback.sh
```

To change the duration:

```bash
AES67_SOAK_DURATION_SECONDS=900 bash scripts/soak_loopback.sh
```

## CI

GitHub Actions on `ubuntu-latest` runs `cargo test --workspace` and `bash scripts/e2e_loopback.sh`.
