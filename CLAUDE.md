# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture Overview

Lexi is a CLI-first tool for working with multiple LLM providers side by side, with a GUI wrapper. The project has two main components:

- **lexi-cli**: Python CLI application (src/lexi_cli/) that provides the core functionality
- **lexi-gui**: Node.js GUI server (src/server/) that wraps and proxies to the CLI

### CLI Architecture
- Single-file CLI implementation in `cli.py` (~95KB) with embedded commands, history, and configuration management
- Uses src/ layout with package data including provider configs and default aliases
- Persistent configuration stored in `~/.lexi-cli/` directory structure
- Provider-specific configurations in `~/.lexi-cli/providers/<provider>/` with YAML files for models, aliases, and configs
- Interactive shell with history support and command aliases
- Config-driven API abstraction: provider auth, request body, and response parsing are defined in YAML, not hardcoded

### Configuration Structure
- Main provider config: `~/.lexi-cli/providers/providers-config.yaml`
- Per-provider model configs: `~/.lexi-cli/providers/<provider>/configured-models.yaml`
- Model aliases: `~/.lexi-cli/providers/<provider>/model-aliases.yaml`
- Default model: `~/.lexi-cli/providers/<provider>/default-model.yaml`
- Command aliases: `~/.lexi-cli/aliases.json`
- Prompts stored as individual JSON files in `~/.lexi-cli/prompts/`

## Development Commands

### CLI Development
```bash
# Install CLI in development mode
cd lexi-cli && pip install -e .

# Run CLI directly (multiple options)
lexi                    # If installed
python -m lexi_cli      # From repo root
./cli                   # Using shim script

# Development tools
ruff check src/         # Lint code
ruff format src/        # Format code
mypy src/              # Type checking
pytest                 # Run tests
pytest --cov=lexi_cli  # Run tests with coverage

# Install with crypto support (for API key encryption)
pip install -e ".[crypto]"
```

### GUI Development
```bash
# Setup and run GUI
cd lexi-gui && npm install
npm run start          # Production server (http://localhost:3000)
npm run dev           # Development server with --watch

# The GUI expects 'lexi' on PATH or will fall back to 'lexi-cli/cli' in repo
```

## Key Implementation Details

### Command Structure
All main commands use plural forms (`prompts`, `providers`, `models`, `alias`) and support subcommands. The CLI includes extensive help system accessible via `help <command>` or `help <command> <subcommand>`.

### Provider System
- Three preconfigured providers: OpenAI, Anthropic, NVIDIA
- Each provider has hosted models endpoint support with configurable field mapping
- API keys can be encrypted using cryptography library if installed
- Model metadata and aliases stored separately from main provider config
- Auth headers, request body templates, and response text extraction are all config-driven via `auth`, `request`, and `response` blocks in providers-config.yaml
- Legacy configs using `respond_kind`/`respond_path` are migrated at runtime

### Configuration Seeding
On first run, the CLI seeds `~/.lexi-cli/` from the packaged `providers/` directory and creates default aliases from `default-aliases.json`.

### Dependencies
- CLI: Python 3.9+, pyyaml, rich (optional: cryptography for API key encryption)
- GUI: Node 18+, yaml package

## File Locations
- CLI source: `lexi-cli/src/lexi_cli/`
- GUI source: `lexi-gui/src/server/`
- Provider configs: `providers/` (copied to `~/.lexi-cli/providers/` on first run)
- Package configuration: `lexi-cli/pyproject.toml`, `lexi-gui/package.json`
