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

MODE="tui"
PROJECT=""
FORCE_REBUILD="${PROXY_FORCE_REBUILD:-0}"
while (($#)); do
    case "$1" in
        --server-only)
            MODE="server"
            shift
            ;;
        --project)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "--project requires a directory" >&2
                exit 2
            fi
            PROJECT="$2"
            shift 2
            ;;
        --force-rebuild)
            FORCE_REBUILD=1
            shift
            ;;
        *)
            echo "Usage: $0 [--force-rebuild] [--server-only [--project DIRECTORY]]" >&2
            exit 2
            ;;
    esac
done
if [[ -n "$PROJECT" && "$MODE" != "server" ]]; then
    echo "--project requires --server-only" >&2
    exit 2
fi

needs_build=false
if [[ "$FORCE_REBUILD" == "1" || ! -x "$BINARY" ]]; then
    needs_build=true
else
    for input in "$DIR/Cargo.toml" "$DIR/Cargo.lock" "$DIR/build.rs"; do
        if [[ -e "$input" && "$input" -nt "$BINARY" ]]; then
            needs_build=true
            break
        fi
    done
    if [[ "$needs_build" == false && -n "$(find "$DIR/src" -type f -newer "$BINARY" -print -quit 2>/dev/null)" ]]; then
        needs_build=true
    fi
fi

if [[ "$needs_build" == true ]]; then
    echo "Building optimized Rust proxy (source changed or rebuild requested)..."
    cargo build --release --manifest-path "$DIR/Cargo.toml"
else
    echo "Using current optimized Rust proxy (pass --force-rebuild to rebuild)."
fi

SERVER_ARGS=()
if [[ -n "$PROJECT" ]]; then
    SERVER_ARGS=(--project "$PROJECT")
fi
exec "$BINARY" "$MODE" "${SERVER_ARGS[@]}"
