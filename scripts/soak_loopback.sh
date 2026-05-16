#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: bash scripts/soak_loopback.sh [--dry-run]

Runs the CI-safe media loopback path for a longer duration. This is intended
for local release-candidate soak validation, not for every CI run.

Environment:
  AES67_SOAK_DURATION_SECONDS  Duration in seconds, default 300
  AES67_SOAK_ARTIFACT_DIR      Artifact directory, default target/soak-loopback
  AES67_SOAK_ADDRESS           Destination address, default 127.0.0.1
  AES67_SOAK_PORT              Destination port, default 55014
  AES67_SOAK_INTERFACE         Local interface IPv4, default 127.0.0.1
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

DURATION_SECONDS="${AES67_SOAK_DURATION_SECONDS:-300}"
ARTIFACT_DIR="${AES67_SOAK_ARTIFACT_DIR:-target/soak-loopback}"
ADDRESS="${AES67_SOAK_ADDRESS:-127.0.0.1}"
PORT="${AES67_SOAK_PORT:-55014}"
INTERFACE="${AES67_SOAK_INTERFACE:-127.0.0.1}"

if ! [[ "$DURATION_SECONDS" =~ ^[0-9]+$ ]] || (( DURATION_SECONDS < 1 )); then
    echo "AES67_SOAK_DURATION_SECONDS must be a positive integer, got: $DURATION_SECONDS" >&2
    exit 2
fi

cat <<CONFIG
Soak test configuration
  Duration: $DURATION_SECONDS seconds
  Address:  $ADDRESS
  Port:     $PORT
  Interface:$INTERFACE
  Artifacts:$ARTIFACT_DIR
  Command:  scripts/e2e_loopback.sh
CONFIG

if (( DRY_RUN )); then
    exit 0
fi

AES67_E2E_DURATION_SECONDS="$DURATION_SECONDS" \
AES67_E2E_ARTIFACT_DIR="$ARTIFACT_DIR" \
AES67_E2E_ADDRESS="$ADDRESS" \
AES67_E2E_PORT="$PORT" \
AES67_E2E_INTERFACE="$INTERFACE" \
bash scripts/e2e_loopback.sh
