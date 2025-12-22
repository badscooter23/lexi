# lexi-cli

Python CLI for Lexi. Uses a src/ layout, persists config/history under `~/.lexi-cli/`, and supports both single-command and interactive shells.

## Setup
- Ensure Python 3.9+.
- Install editable for development (optional): `pip install -e .`
- Run directly via `lexi`, `python -m lexi_cli`, or the shim `./cli`.

## Commands
- `help topics | command` / `? topics | command` — show topics or details for one. Topics: `ai`, `general`, `shortcuts`. Commands with sub-commands (`config`, `prompt`) list them under topics; use `help <command> <sub>` for full syntax. Shortcuts include `!` (shell), `#<N>` (history rerun), and ↑/↓ to navigate history.
- `version` — print `<cli_name>:<version>` from config (defaults to `lexi:0.1.0`).
- `sh <cmd>` / `!<cmd>` — run a shell command.
- `history [-n COUNT] [--reset|--clear]` — show recent commands or clear history; `#<N>` reruns an entry (negatives count from the end).
- `config list [--raw|key]` — show config values (raw JSON or selected keys).
- `config set <key> <value>` / `config rm <key>` — update or remove config entries.
- General commands:\n  - `help` / `?` — show help topics and command usage.\n  - `alias list|add|rm` — manage command aliases stored in `~/.lexi-cli/aliases.json` (expansions are shlex-split and appended with any extra args).\n  - `config` — view or update settings. `config list [--raw|key] | config set <key> <value> | config rm <key>`\n  - `history` — show/clear history. `history [-n COUNT] [--reset|--clear]`\n  - `sh <cmd>` — run a shell command; `exit` / `quit` to leave.\n- Lexi commands:\n  - `prompt <text> [[-m|--max_tokens] N] [[-t|--temperature] T] [[-r|--role] ROLE]` — emit a prompt JSON payload using defaults from `config/config.yaml` when flags are omitted.\n    - `prompt list [name] [[-r|--raw] [-d|--detailed]]` — view saved prompts from `~/.lexi-cli/prompts/` (raw JSON or readable). Active prompt is `$$`.\n    - `prompt set [name] [--prompt TEXT] [[-m|--max_tokens] N] [[-t|--temperature] T] [[-r|--role] ROLE]` — save or update a prompt definition (defaults to `$$` active prompt); `prompt rm <name>` removes it.\n  - `provider list|add|rename|rm` — manage providers from `~/.lexi-cli/providers/providers-config.yaml` and per-provider model directories.\n  - `model list|add|set|rm|hosted` — manage models for a provider (files under `~/.lexi-cli/providers/<name>/models.yaml`). Use `model list <provider> [-r|--raw] [-t|--table] [-d|--details]` for different output formats. Use `model hosted <provider> [-r|--raw] [-t|--table]` to query the provider’s hosted models via its API.\n- `exit` / `quit` — leave the CLI.
  - `prompt list [name] [[-r|--raw] [-d|--detailed]]` — view saved prompts from `~/.lexi-cli/prompts/` (raw JSON or readable). Active prompt is `$$`.
  - `prompt set [name] [--prompt TEXT] [[-m|--max_tokens] N] [[-t|--temperature] T] [[-r|--role] ROLE]` — save or update a prompt definition (defaults to `$$` active prompt); `prompt rm <name>` removes it.
- `provider list|add|rename|rm` — manage providers from `providers/providers-config.yaml` and per-provider model directories.
- `models list|add|set|rm` — manage models for a provider (files under `providers/<name>/models.yaml`). Use `model list <provider> [-r|--raw] [-t|--table] [-d|--details]` to view models (table view uses `rich`).
- `exit` / `quit` / `e` / `q` — leave the CLI.

## Configuration & History
- Stored in `~/.lexi-cli/config.json` and `~/.lexi-cli/history`.
- Saved prompts persist as individual files under `~/.lexi-cli/prompts/` (active prompt is `$$.json`); defaults for prompt params come from `config/config.yaml` (or `.example`), falling back to built-ins.
- Providers and models are stored under `~/.lexi-cli/providers/` (seeded from the repo’s `providers/` on first run). `providers-config.yaml` holds provider URLs/API keys/default models; each provider has `providers/<name>/models.yaml` with models/aliases/default. OpenAI, Anthropic, and NVIDIA are preconfigured; use `provider list` and `models list <provider>` to inspect.
- Legacy files `.config.json`, `.cli_history`, and `~/.exp-cli/` are migrated on first run if present.
- Default config values:
  - `cli_name`: `lexi`
  - `version`: `0.1.0`
  - `prompt`: `cmd`
  - `prompt_delimiter`: `>`
  - `edit_mode`: `vi` (set to `emacs` if preferred)

## Development Notes
- Code lives in `src/lexi_cli/`. Run `python -m lexi_cli` from the repo root while iterating.
- Add tests under `tests/` mirroring the src layout; use pytest or unittest as preferred.
- When publishing, the console script entrypoint is `lexi` (configured in `pyproject.toml`).
