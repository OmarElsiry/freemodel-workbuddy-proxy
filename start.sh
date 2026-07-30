#!/bin/bash
# Start the Rust Freemodel WorkBuddy proxy or hybrid TUI.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
BINARY="$DIR/target/release/freemodel-workbuddy-proxy"

if [ ! -x "$BINARY" ]; then
    echo "Building optimized Rust proxy..."
    cargo build --release --manifest-path "$DIR/Cargo.toml"
fi

case "$#:${1:-}" in
    0:)
        exec "$BINARY" tui
        ;;
    1:--server-only)
        exec "$BINARY" server
        ;;
    *)
        echo "Usage: $0 [--server-only]" >&2
        exit 2
        ;;
esac
