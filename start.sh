#!/bin/bash
# Start script for Freemodel API Proxy Server & Interactive TUI
set -e

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

PORT="${PROXY_PORT:-40589}"
HOST="${PROXY_HOST:-0.0.0.0}"

if [ "$1" = "--server-only" ]; then
    echo "Starting Freemodel API Proxy on http://$HOST:$PORT..."
    exec python3 -m uvicorn proxy_server:app --host "$HOST" --port "$PORT"
fi

# Ensure proxy server is running in background if not already
if ! fuser "$PORT/tcp" >/dev/null 2>&1; then
    echo "Starting Freemodel API Proxy in background on http://$HOST:$PORT..."
    python3 tui.py --start
fi

# Launch Interactive TUI
python3 tui.py
