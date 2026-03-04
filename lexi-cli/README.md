# lexi-cli

Python CLI for Lexi. Uses a src/ layout, persists config/history under `~/.lexi-cli/`, and supports both single-command and interactive shell modes.

## Setup

```bash
# Requires Python 3.9+
pip install -e .

# Run
lexi                    # If installed
python -m lexi_cli      # From repo root
./cli                   # Using shim script
```

## Commands

All main commands use **plural forms** and support subcommands. Use `help <command>` for details.

### Lexi Commands

- **`prompts`** — Manage saved prompts
  - `prompts list [name] [-r|--raw] [-d|--detailed] [-t|--table]`
  - `prompts set [name] --prompt TEXT [--max_tokens N] [--temperature T] [--role ROLE]`
  - `prompts rm <name>`

- **`providers`** — Manage LLM providers
  - `providers list [--raw|-r]`
  - `providers add <name> --url URL --api_key KEY --default_model MODEL`
  - `providers api-key set|show|remove <name>`
  - `providers rename <old> <new>`
  - `providers rm <name>`

- **`models`** — Manage models for a provider
  - `models list <provider> [--raw|-r] [--table|-t] [--details|-d]`
  - `models hosted <provider> [--raw|-r] [--table|-t]`
  - `models default <provider> [model_name]`
  - `models alias list|set|rm <provider> [...]`
  - `models add|set|rm <provider> <model> [options]`

- **`respond`** — Send a prompt to a provider
  - `respond <provider> [prompt_name] [--model M] [--temperature T] [--max_tokens N] [--web-search] [--raw|-r]`

- **`alias`** — Manage command aliases
  - `alias list | alias add <name> <expansion> | alias rm <name> | alias reset`

### General Commands

- `help [topic|command]` / `?` — Show help
- `config list|set|rm` — View or update settings
- `history [-n COUNT] [--reset|--clear]` — Show or clear history
- `sh <cmd>` / `!<cmd>` — Run a shell command
- `version` — Show CLI version
- `exit` / `quit` / `e` / `q` — Exit

### Shortcuts

- `?` — Help
- `!<cmd>` — Shell command
- `#<N>` — Re-run history entry (negatives count from end)
- Arrow keys — Navigate command history

## Configuration

Stored in `~/.lexi-cli/` (seeded from repo's `providers/` on first run):

```
~/.lexi-cli/
├── config.json                        # CLI settings
├── history                            # Command history
├── aliases.json                       # Command aliases
├── prompts/                           # Saved prompts (active = $$.json)
└── providers/
    ├── providers-config.yaml          # Provider URLs, auth, request/response config
    └── <provider>/
        ├── configured-models.yaml     # Model metadata
        ├── default-model.yaml         # Default model
        ├── model-aliases.yaml         # Model shorthand aliases
        └── api-key                    # API key (chmod 600)
```

Default config values: `cli_name: lexi`, `prompt: cmd`, `prompt_delimiter: >`, `edit_mode: vi`.

Legacy files (`.config.json`, `.cli_history`, `~/.exp-cli/`) are migrated on first run.

## Development

```bash
pip install -e .
ruff check src/         # Lint
ruff format src/        # Format
pytest                  # Test
pytest --cov=lexi_cli   # Test with coverage
```

Code lives in `src/lexi_cli/`. The console script entrypoint `lexi` is configured in `pyproject.toml`.
