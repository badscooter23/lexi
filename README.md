# Lexi

Lexi is a CLI-first tool for working with multiple LLM providers side by side, with a simple GUI that wraps the same CLI. It seeds provider configs and models into `~/.lexi-cli/` on first run and ships sensible default aliases.

## Quick start
- Requirements: Python 3.9+, Node 18+ (for the GUI).
- Install CLI (dev): `cd lexi-cli && pip install -e .`
- Run: `lexi` (or `python -m lexi_cli`/`./cli`). First launch seeds `~/.lexi-cli/providers/` and `~/.lexi-cli/aliases.json`.
- Default aliases (grouped per command): `exit -> e,q,ex`, `help -> h`, `history -> hist`, `prompts -> prompts,prom`, `providers -> providers,prov`, `models -> models,mod`.

## Core commands (plurals)
- `prompts list [name] [-r|--raw] [-d|--detailed] [-t|--table]`
- `prompts set [name] --prompt TEXT [--max_tokens N] [--temperature T] [--role ROLE]`
- `prompts rm <name>`
- `providers list|add|rename|rm` (config stored in `providers-config.yaml`)
- `models list|hosted|add|set|rm <provider> [...]` (`hosted` calls the provider’s /models endpoint; `-t/--table` and `-d/--details` are supported)
- `alias list|add|rm` (persisted to `~/.lexi-cli/aliases.json`; stored as `command: [aliases]`)
- General: `help`, `sh`, `history`, `config`, `exit`, `version`

## Files and configuration
- `~/.lexi-cli/providers/providers-config.yaml` — provider URLs/api_keys/default_model plus hosted endpoint/fields.
- `~/.lexi-cli/providers/<provider>/models.yaml` — local model metadata/aliases.
- `~/.lexi-cli/aliases.json` — command -> [aliases] (seeded from packaged defaults if missing/empty).
- Prompts: `~/.lexi-cli/prompts/*.json`; active prompt is `$$.json`.

## GUI
- `cd lexi-gui && npm install`
- Start: `npm run start` (http://localhost:3000)
- The server proxies to the CLI (expects `lexi` on PATH or `lexi-cli/cli` in the repo).

## Notes
- Commands are plural (`prompts`, `providers`, `models`); old singular forms are not available.
- Hosted model listings use provider configs’ `hosted_models` path and `hosted_model_fields` to shape table output.
