#!/usr/bin/env bash
# Launch the Lexi GUI server
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if [ ! -d node_modules ]; then
    echo "Installing dependencies..."
    npm install
fi

echo "Starting Lexi GUI on http://localhost:3000"
node src/server/index.js
