# Changelog

## [0.1.2]

- Make the root `VERSION` file the single source of truth for public release
  versioning.
- Add Linux x86_64 release archive generation and Homebrew formula support.

## [0.1.0]

Initial Apple Silicon macOS and x86_64 Linux release for the AES67 tools workspace.

### `aes67-streamer`

- Streams WAV, FLAC, MP3, and AIFF files as AES67-oriented RTP audio.
- Outputs 48 kHz, 24-bit L24 RTP with 1-8 channel support.
- Resamples input files to the AES67 target format.
- Supports multicast and unicast UDP output.
- Generates SDP and announces streams with SAP.
- Runs PTP background handling with local master fallback.
- Supports bounded playback duration and optional source-file looping.
- Provides TOML configuration with CLI override support.
- Applies default DSCP markings for PTP, RTP, and SAP traffic.

### `aes67-player`

- Receives AES67-oriented RTP L24 streams over UDP.
- Supports SDP-based receive setup and basic address/port CLI setup.
- Reorders packets through a jitter buffer with loss, duplicate, late, and
  timestamp-discontinuity accounting.
- Plays received audio through CPAL output on macOS and Linux.
- Lists and selects output devices from the CLI.
- Supports configurable initial playout latency.
- Provides bounded receive duration for validation and soak testing.
- Reports final playback summary metrics for RTP silence, jitter behavior,
  output silence, and dropped samples.

### Release Packaging

- Packages `aes67-streamer` and `aes67-player` together as `aes67-tools`.
- Publishes Apple Silicon macOS and x86_64 Linux archives for Homebrew installation.
- Includes README, license, version, and example SDP/TOML files in the release
  archive.
