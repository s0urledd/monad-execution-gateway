#!/usr/bin/env bash
# Quick test: connect to the gateway and print raw events
#
# Usage:
#   ./examples/websocket-test.sh ws://your-validator:8443/v1/ws
#   ./examples/websocket-test.sh ws://your-validator:8443/v1/ws/blocks
#
# Requires: websocat (cargo install websocat)

set -euo pipefail

URL="${1:-ws://127.0.0.1:8443/v1/ws}"

echo "Connecting to $URL ..."
echo "Press Ctrl+C to stop"
echo "---"

websocat "$URL" | while IFS= read -r line; do
    echo "$line" | python3 -m json.tool 2>/dev/null || echo "$line"
done
