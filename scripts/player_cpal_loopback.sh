#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: bash scripts/player_cpal_loopback.sh [--dry-run]

Runs a local aes67-streamer -> aes67-player loopback using real CPAL playback.
This is a manual validation path for audible playback smoothness.

Environment:
  AES67_PLAYER_OUTPUT_DEVICE      Optional CPAL device index or name from -L
  AES67_PLAYER_ALLOW_UNCLOCKED_OUTPUT
                                  Set to 1 only for diagnostics with null/discard devices
  AES67_PLAYER_DURATION_SECONDS   Playback duration in seconds, default 10
  AES67_PLAYER_LATENCY_MS         Player latency in milliseconds, default 50
  AES67_PLAYER_ARTIFACT_DIR       Artifact directory, default target/player-cpal-loopback
  AES67_PLAYER_ADDRESS            Destination address, default 127.0.0.1
  AES67_PLAYER_PORT               Destination port, default 55210
  AES67_PLAYER_INTERFACE          Local interface IPv4, default 127.0.0.1

Before running on Linux, install the CPAL backend dependency. On Fedora:
  sudo dnf install alsa-lib-devel

To list selectable devices:
  cargo run -p aes67-player -- -L
USAGE
}

DRY_RUN=0
for arg in "$@"; do
    case "$arg" in
        --dry-run)
            DRY_RUN=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            usage >&2
            exit 2
            ;;
    esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ARTIFACT_DIR="${AES67_PLAYER_ARTIFACT_DIR:-target/player-cpal-loopback}"
ADDRESS="${AES67_PLAYER_ADDRESS:-127.0.0.1}"
PORT="${AES67_PLAYER_PORT:-55210}"
INTERFACE="${AES67_PLAYER_INTERFACE:-127.0.0.1}"
DURATION_SECONDS="${AES67_PLAYER_DURATION_SECONDS:-10}"
LATENCY_MS="${AES67_PLAYER_LATENCY_MS:-50}"
OUTPUT_DEVICE="${AES67_PLAYER_OUTPUT_DEVICE:-}"
ALLOW_UNCLOCKED_OUTPUT="${AES67_PLAYER_ALLOW_UNCLOCKED_OUTPUT:-0}"
CARGO_CMD="${AES67_PLAYER_CARGO:-cargo}"
PLAYER_BIN="${AES67_PLAYER_PLAYER_BIN:-target/debug/aes67-player}"
STREAMER_BIN="${AES67_PLAYER_STREAMER_BIN:-target/debug/aes67-streamer}"
INPUT_WAV="src/aes67-streamer/tests/resources/audio-formats/tone.wav"
PLAYER_LOG="$ARTIFACT_DIR/player.log"
STREAMER_LOG="$ARTIFACT_DIR/streamer.log"

player_pid=""

cleanup() {
    if [[ -n "$player_pid" ]] && kill -0 "$player_pid" 2>/dev/null; then
        kill "$player_pid" 2>/dev/null || true
        wait "$player_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Missing required command: $1" >&2
        exit 1
    fi
}

summary_value() {
    local label="$1"
    awk -F': ' -v label="$label" '$0 ~ label { value=$NF } END { print value }' "$PLAYER_LOG"
}

wait_for_log_content() {
    local file="$1"
    for _ in {1..20}; do
        if [[ -s "$file" ]]; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

print_log_tail() {
    local file="$1"
    local label="$2"

    echo "--- $label ---" >&2
    if [[ -s "$file" ]]; then
        tail -n 120 "$file" >&2
    elif [[ -e "$file" ]]; then
        echo "(log file exists but is empty: $file)" >&2
    else
        echo "(log file was not created: $file)" >&2
    fi
}

fail_with_logs() {
    echo "$1" >&2
    wait_for_log_content "$PLAYER_LOG" || true
    print_log_tail "$PLAYER_LOG" "player log"
    print_log_tail "$STREAMER_LOG" "streamer log"
    echo "--- device list command ---" >&2
    echo "cargo run -p aes67-player -- -L" >&2
    exit 1
}

player_log_has_unclocked_output() {
    grep -Eiq "Created CPAL output on '.*(Discard all samples|generate zero samples|null|dummy)" "$PLAYER_LOG"
}

allow_unclocked_output() {
    [[ "$ALLOW_UNCLOCKED_OUTPUT" == "1" ]]
}

require_command "$CARGO_CMD"
require_command awk

if ! [[ "$DURATION_SECONDS" =~ ^[0-9]+$ ]] || (( DURATION_SECONDS < 1 )); then
    echo "AES67_PLAYER_DURATION_SECONDS must be a positive integer, got: $DURATION_SECONDS" >&2
    exit 2
fi
if ! [[ "$LATENCY_MS" =~ ^[0-9]+$ ]] || (( LATENCY_MS < 1 )); then
    echo "AES67_PLAYER_LATENCY_MS must be a positive integer, got: $LATENCY_MS" >&2
    exit 2
fi

player_duration=$((DURATION_SECONDS + 2))

cat <<CONFIG
CPAL player loopback configuration
  Duration:      $DURATION_SECONDS seconds
  Player wait:   $player_duration seconds
  Latency:       $LATENCY_MS ms
  Address:       $ADDRESS
  Port:          $PORT
  Interface:     $INTERFACE
  Output device: ${OUTPUT_DEVICE:-default}
  Allow null:    $ALLOW_UNCLOCKED_OUTPUT
  Artifacts:     $ARTIFACT_DIR
CONFIG

if (( DRY_RUN )); then
    exit 0
fi

mkdir -p "$ARTIFACT_DIR"
rm -f "$PLAYER_LOG" "$STREAMER_LOG"

echo "Building aes67-streamer and aes67-player with CPAL output..."
"$CARGO_CMD" build -p aes67-streamer
if ! "$CARGO_CMD" build -p aes67-player; then
    fail_with_logs "Failed to build aes67-player with CPAL output. On Fedora, install alsa-lib-devel."
fi

player_args=(
    --address "$ADDRESS"
    --port "$PORT"
    --interface "$INTERFACE"
    --latency-ms "$LATENCY_MS"
    --duration-seconds "$player_duration"
)
if [[ -n "$OUTPUT_DEVICE" ]]; then
    player_args+=(--output-device "$OUTPUT_DEVICE")
fi

echo "Starting aes67-player..."
RUST_LOG=info "$PLAYER_BIN" "${player_args[@]}" >"$PLAYER_LOG" 2>&1 &
player_pid=$!

sleep 1
if ! kill -0 "$player_pid" 2>/dev/null; then
    wait "$player_pid" || true
    fail_with_logs "aes67-player exited before the streamer started"
fi
if player_log_has_unclocked_output; then
    if allow_unclocked_output; then
        echo "Warning: selected CPAL output appears to be an unclocked null/discard device; smoothness results are diagnostic only" >&2
    else
        fail_with_logs "Selected CPAL output appears to be an unclocked null/discard device. Select a real clocked playback device from -L, or set AES67_PLAYER_ALLOW_UNCLOCKED_OUTPUT=1 for diagnostics only."
    fi
fi

echo "Running aes67-streamer for ${DURATION_SECONDS}s..."
RUST_LOG=info "$STREAMER_BIN" \
    --file "$INPUT_WAV" \
    --address "$ADDRESS" \
    --port "$PORT" \
    --interface "$INTERFACE" \
    --loop \
    --duration-seconds "$DURATION_SECONDS" >"$STREAMER_LOG" 2>&1 || fail_with_logs "aes67-streamer failed"

deadline=$((SECONDS + player_duration + 5))
while kill -0 "$player_pid" 2>/dev/null; do
    if (( SECONDS > deadline )); then
        fail_with_logs "Timed out waiting for aes67-player"
    fi
    sleep 0.2
done

wait "$player_pid" || fail_with_logs "aes67-player failed"
player_pid=""

packets_decoded="$(summary_value "Packets decoded")"
rtp_silence_frames="$(summary_value "RTP silence frames")"
jitter_lost_packets="$(summary_value "Jitter lost packets")"
jitter_late_packets="$(summary_value "Jitter late packets")"
jitter_dropped_full_packets="$(summary_value "Jitter dropped-full packets")"
jitter_timestamp_discontinuities="$(summary_value "Jitter timestamp discontinuities")"
output_silence_frames="$(summary_value "Output silence frames")"
output_dropped_samples="$(summary_value "Output dropped samples")"

[[ -n "$packets_decoded" && "$packets_decoded" != "0" ]] || fail_with_logs "Player decoded no RTP audio"
[[ "${rtp_silence_frames:-}" == "0" ]] || fail_with_logs "RTP playout inserted ${rtp_silence_frames:-unknown} silence frames"
[[ "${jitter_lost_packets:-}" == "0" ]] || fail_with_logs "Jitter buffer reported ${jitter_lost_packets:-unknown} lost packets"
[[ "${jitter_late_packets:-}" == "0" ]] || fail_with_logs "Jitter buffer reported ${jitter_late_packets:-unknown} late packets"
[[ "${jitter_dropped_full_packets:-}" == "0" ]] || fail_with_logs "Jitter buffer dropped ${jitter_dropped_full_packets:-unknown} packets while full"
[[ "${jitter_timestamp_discontinuities:-}" == "0" ]] || fail_with_logs "Jitter buffer reported ${jitter_timestamp_discontinuities:-unknown} timestamp discontinuities"
[[ "${output_silence_frames:-}" == "0" ]] || fail_with_logs "Audio output inserted ${output_silence_frames:-unknown} silence frames"
[[ "${output_dropped_samples:-}" == "0" ]] || fail_with_logs "Audio output dropped ${output_dropped_samples:-unknown} samples"

if grep -q " WARN " "$PLAYER_LOG"; then
    fail_with_logs "Player emitted warning logs"
fi

echo "CPAL player loopback passed"
echo "Artifacts: $ARTIFACT_DIR"
echo "Packets decoded: $packets_decoded"
