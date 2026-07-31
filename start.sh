#!/bin/bash
# Start the Rust Freemodel WorkBuddy proxy or hybrid TUI.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
BINARY="$DIR/target/release/freemodel-workbuddy-proxy"

# WorkBuddy can inject provider settings into every terminal it opens. Project
# config.json is the authoritative runtime configuration for this launcher, so
# do not let inherited provider variables silently switch transports or keys.
if [[ -f "$DIR/config.json" ]]; then
    unset FREEMODEL_API_KEY FREEMODEL_BASE_URL FREEMODEL_TRANSPORT WORKBUDDY_ACP_URL WORKBUDDY_ACP_PASSWORD
fi

SERVER_ARGS=()
case "$#:${1:-}" in
    0:)
        MODE="tui"
        ;;
    1:--server-only)
        MODE="server"
        ;;
    3:--server-only)
        if [[ "${2:-}" != "--project" || -z "${3:-}" ]]; then
            echo "Usage: $0 [--server-only [--project DIRECTORY]]" >&2
            exit 2
        fi
        MODE="server"
        SERVER_ARGS=(--project "$3")
        ;;
    *)
        echo "Usage: $0 [--server-only [--project DIRECTORY]]" >&2
        exit 2
        ;;
esac

# Always ask Cargo to build. Cargo performs a fast no-op when the binary is
# current and recompiles it whenever source files or manifests have changed.
echo "Ensuring optimized Rust proxy is current..."
cargo build --release --manifest-path "$DIR/Cargo.toml"

exec "$BINARY" "$MODE" "${SERVER_ARGS[@]}"
