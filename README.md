# Lexi

[![CI](https://github.com/badscooter23/lexi/actions/workflows/ci.yml/badge.svg)](https://github.com/badscooter23/lexi/actions/workflows/ci.yml)

Lexi is a CLI-first tool for working with multiple LLM providers side by side, with an optional GUI wrapper. It ships with preconfigured support for OpenAI, Anthropic, and NVIDIA, and seeds provider configs into `~/.lexi-cli/` on first run.

## Quick Start

**Requirements:** Python 3.9+, Node 18+ (for the GUI)

```bash
# Install everything
./install.sh

# Or install just the CLI
cd lexi-cli && pip install -e .

# Run
lexi
```

On first launch, Lexi seeds `~/.lexi-cli/providers/` with provider configs, model metadata, and default command aliases.

## Core Commands

All main commands use **plural forms** and support subcommands. Use `help <command>` for details.

| Command | Description |
|---|---|
| `prompts list\|set\|rm` | Manage saved prompts |
| `providers list\|add\|rename\|rm` | Manage LLM providers |
| `models list\|hosted\|add\|set\|rm\|default\|alias` | Manage models for a provider |
| `respond <provider> [prompt] [--model M] [--raw]` | Send a prompt to a provider |
| `alias list\|add\|rm\|reset` | Manage command aliases |
| `config list\|set\|rm` | View or update settings |
| `help`, `history`, `sh`, `version`, `exit` | General commands |

### Examples

```bash
# Set a prompt and send it to Anthropic
prompts set myq --prompt "Explain quicksort in one paragraph"
respond anthropic myq

# Use the active prompt ($$) with a specific model
respond openai --model gpt-4o

# List hosted models from a provider
models hosted nvidia --table

# Set a default model
models default anthropic claude-sonnet-4-5-20250929
```

### Shortcuts

| Shortcut | Action |
|---|---|
| `?` | Help |
| `!<cmd>` | Run shell command |
| `#<N>` | Re-run history entry |
| Arrow keys | Navigate command history |

## Configuration

All persistent configuration lives in `~/.lexi-cli/`:

```
~/.lexi-cli/
├── providers/
│   ├── providers-config.yaml          # Provider URLs, auth, request/response config
│   ├── anthropic/
│   │   ├── configured-models.yaml     # Model metadata
│   │   ├── default-model.yaml         # Default model selection
│   │   └── model-aliases.yaml         # Shorthand aliases (e.g., "sonnet")
│   ├── openai/
│   │   └── ...
│   └── nvidia/
│       └── ...
├── prompts/                           # Saved prompts as JSON files
├── aliases.json                       # Command aliases
└── config.json                        # CLI settings
```

### Provider API Configuration

Provider-specific API details (auth headers, request body shape, response parsing) are driven by YAML config — no code changes needed to add a new provider:

```yaml
providers:
  my_provider:
    url: https://api.example.com/v1
    api_key: ${MY_PROVIDER_API_KEY}
    auth:
      header: Authorization
      value_prefix: "Bearer "
    request:
      path: /chat/completions
      body:
        model: "{{model}}"
        messages:
          - role: "{{role}}"
            content: "{{prompt}}"
      param_mapping:
        temperature: temperature
        max_tokens: max_tokens
    response:
      text_path: "choices[0].message.content"
```

## GUI

```bash
cd lexi-gui && npm install
npm run start          # http://localhost:3000
npm run dev            # Development mode with --watch
```

The GUI server proxies to the CLI backend (expects `lexi` on PATH or falls back to `lexi-cli/cli` in the repo).

## Development

```bash
# CLI
cd lexi-cli
pip install -e .
ruff check src/        # Lint
ruff format src/       # Format
pytest                 # Test
pytest --cov=lexi_cli  # Test with coverage

# GUI
cd lexi-gui
npm install
npm run dev
```

## License

MIT
