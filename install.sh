#!/usr/bin/env bash
set -euo pipefail

echo "Installing Lexi CLI..."

# Check Python version
if ! command -v python3 &>/dev/null; then
    echo "Error: python3 is required but not found." >&2
    exit 1
fi

PYTHON_VERSION=$(python3 -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')
REQUIRED="3.9"
if [ "$(printf '%s\n' "$REQUIRED" "$PYTHON_VERSION" | sort -V | head -n1)" != "$REQUIRED" ]; then
    echo "Error: Python >= $REQUIRED is required (found $PYTHON_VERSION)." >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Install CLI in editable mode
cd "$SCRIPT_DIR/lexi-cli"
pip install -e . --quiet

echo "Lexi CLI installed. Run 'lexi' to start."

# Optionally install GUI
if command -v node &>/dev/null; then
    NODE_VERSION=$(node -v | sed 's/v//' | cut -d. -f1)
    if [ "$NODE_VERSION" -ge 18 ]; then
        echo "Installing Lexi GUI..."
        cd "$SCRIPT_DIR/lexi-gui"
        npm install --quiet
        echo "Lexi GUI installed. Run 'npm start' in lexi-gui/ to launch."
    else
        echo "Skipping GUI install: Node >= 18 required (found v$NODE_VERSION)."
    fi
else
    echo "Skipping GUI install: Node.js not found."
fi
