#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: bash scripts/player_soak_loopback.sh [--dry-run]

Runs a CI-safe aes67-streamer -> aes67-player soak using the player's internal
null output. This validates longer RTP receive, jitter, decode, and summary
counters without requiring an audio device.

Environment:
  AES67_PLAYER_SOAK_DURATION_SECONDS  Playback duration in seconds, default 60
  AES67_PLAYER_SOAK_LATENCY_MS        Player latency in milliseconds, default 50
  AES67_PLAYER_SOAK_ARTIFACT_DIR      Artifact directory, default target/player-soak-loopback
  AES67_PLAYER_SOAK_ADDRESS           Destination address, default 127.0.0.1
  AES67_PLAYER_SOAK_PORT              Destination port, default 55220
  AES67_PLAYER_SOAK_INTERFACE         Local interface IPv4, default 127.0.0.1
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

ARTIFACT_DIR="${AES67_PLAYER_SOAK_ARTIFACT_DIR:-target/player-soak-loopback}"
ADDRESS="${AES67_PLAYER_SOAK_ADDRESS:-127.0.0.1}"
PORT="${AES67_PLAYER_SOAK_PORT:-55220}"
INTERFACE="${AES67_PLAYER_SOAK_INTERFACE:-127.0.0.1}"
DURATION_SECONDS="${AES67_PLAYER_SOAK_DURATION_SECONDS:-60}"
LATENCY_MS="${AES67_PLAYER_SOAK_LATENCY_MS:-50}"
CARGO_CMD="${AES67_PLAYER_SOAK_CARGO:-cargo}"
PLAYER_BIN="${AES67_PLAYER_SOAK_PLAYER_BIN:-target/debug/aes67-player}"
STREAMER_BIN="${AES67_PLAYER_SOAK_STREAMER_BIN:-target/debug/aes67-streamer}"
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
    exit 1
}

require_command "$CARGO_CMD"
require_command awk

if ! [[ "$DURATION_SECONDS" =~ ^[0-9]+$ ]] || (( DURATION_SECONDS < 1 )); then
    echo "AES67_PLAYER_SOAK_DURATION_SECONDS must be a positive integer, got: $DURATION_SECONDS" >&2
    exit 2
fi
if ! [[ "$LATENCY_MS" =~ ^[0-9]+$ ]] || (( LATENCY_MS < 1 )); then
    echo "AES67_PLAYER_SOAK_LATENCY_MS must be a positive integer, got: $LATENCY_MS" >&2
    exit 2
fi

player_duration=$((DURATION_SECONDS + 2))

cat <<CONFIG
Player soak loopback configuration
  Duration:    $DURATION_SECONDS seconds
  Player wait: $player_duration seconds
  Latency:     $LATENCY_MS ms
  Address:     $ADDRESS
  Port:        $PORT
  Interface:   $INTERFACE
  Artifacts:   $ARTIFACT_DIR
CONFIG

if (( DRY_RUN )); then
    exit 0
fi

mkdir -p "$ARTIFACT_DIR"
rm -f "$PLAYER_LOG" "$STREAMER_LOG"

echo "Building aes67-streamer and aes67-player..."
"$CARGO_CMD" build -p aes67-streamer
"$CARGO_CMD" build -p aes67-player

echo "Starting aes67-player with null output..."
RUST_LOG=info "$PLAYER_BIN" \
    --address "$ADDRESS" \
    --port "$PORT" \
    --interface "$INTERFACE" \
    --test-null-output \
    --latency-ms "$LATENCY_MS" \
    --duration-seconds "$player_duration" >"$PLAYER_LOG" 2>&1 &
player_pid=$!

sleep 1
if ! kill -0 "$player_pid" 2>/dev/null; then
    wait "$player_pid" || true
    fail_with_logs "aes67-player exited before the streamer started"
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
jitter_duplicate_packets="$(summary_value "Jitter duplicate packets")"
jitter_dropped_full_packets="$(summary_value "Jitter dropped-full packets")"
jitter_timestamp_discontinuities="$(summary_value "Jitter timestamp discontinuities")"
output_silence_frames="$(summary_value "Output silence frames")"
output_dropped_samples="$(summary_value "Output dropped samples")"

[[ -n "$packets_decoded" && "$packets_decoded" != "0" ]] || fail_with_logs "Player decoded no RTP audio"
[[ "${rtp_silence_frames:-}" == "0" ]] || fail_with_logs "RTP playout inserted ${rtp_silence_frames:-unknown} silence frames"
[[ "${jitter_lost_packets:-}" == "0" ]] || fail_with_logs "Jitter buffer reported ${jitter_lost_packets:-unknown} lost packets"
[[ "${jitter_late_packets:-}" == "0" ]] || fail_with_logs "Jitter buffer reported ${jitter_late_packets:-unknown} late packets"
[[ "${jitter_duplicate_packets:-}" == "0" ]] || fail_with_logs "Jitter buffer reported ${jitter_duplicate_packets:-unknown} duplicate packets"
[[ "${jitter_dropped_full_packets:-}" == "0" ]] || fail_with_logs "Jitter buffer dropped ${jitter_dropped_full_packets:-unknown} packets while full"
[[ "${jitter_timestamp_discontinuities:-}" == "0" ]] || fail_with_logs "Jitter buffer reported ${jitter_timestamp_discontinuities:-unknown} timestamp discontinuities"
[[ "${output_silence_frames:-}" == "0" ]] || fail_with_logs "Audio output inserted ${output_silence_frames:-unknown} silence frames"
[[ "${output_dropped_samples:-}" == "0" ]] || fail_with_logs "Audio output dropped ${output_dropped_samples:-unknown} samples"

unexpected_warnings="$(grep " WARN " "$PLAYER_LOG" | grep -v "Using internal null audio output for test validation" || true)"
if [[ -n "$unexpected_warnings" ]]; then
    fail_with_logs "Player emitted unexpected warning logs"
fi

echo "Player soak loopback passed"
echo "Artifacts: $ARTIFACT_DIR"
echo "Packets decoded: $packets_decoded"
