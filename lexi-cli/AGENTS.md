# Repository Guidelines

## Project Structure & Module Organization
- `src/lexi_cli/` holds the CLI implementation; keep the entrypoint in `cli.py` and add helpers under the package as it grows.
- Use `tests/` (mirroring `src/`) for unit/integration coverage; place fixtures under `tests/fixtures/` when needed.
- Keep the shim `./cli` for convenience; the primary entrypoint is `lexi` (console script) or `python -m lexi_cli`.
- Store docs/notes in `README.md` and add `docs/` for ADRs or design notes when the CLI expands.

## Build, Test, and Development Commands
- Install in editable mode: `pip install -e .` (Python 3.9+).
- Run once: `lexi "config list"` or `python -m lexi_cli "config list"`; interactive: `lexi`, `python -m lexi_cli`, or `./cli`.
- Help is available via `help topics | command` (or `?`) with topics: ai, general, shortcuts. Commands with sub-commands (config, prompt) are listed under topics; use `help <command> <sub>` for full syntax. Shortcuts: `!` (shell), `#<N>` (history rerun), and ↑/↓ to navigate history.
- AI commands: `prompt <text> [[-m|--max_tokens] N] [[-t|--temperature] T] [[-r|--role] ROLE]` to emit a JSON payload using defaults from `config/config.yaml`. Manage saved prompts with `prompt list [name] [[-r|--raw] [-d|--detailed]]`, `prompt set` (defaults to active `$$`), and `prompt rm` (persisted as files under `~/.lexi-cli/prompts/`).
- Provider commands: `provider list [--raw|-r]|add|rename|rm` manage entries in `~/.lexi-cli/providers/providers-config.yaml`; `model list [--raw|-r]|add|set|rm` manage `~/.lexi-cli/providers/<name>/models.yaml` files (default model/aliases included). `config list <key> --value|-v` prints just a config value (useful for prompt/prompt_delimiter).
- Default providers included: OpenAI, Anthropic, NVIDIA (seeded from the repo on first run into `~/.lexi-cli/providers/`).
- History supports `-n COUNT` and clearing via `--reset` or `--clear`.
- When tests exist: `pytest` (or `python -m pytest`) from the repo root.
- If lint/format tools are added (e.g., Ruff/Black), expose them via `make lint`/`make format` or `pipx run` and document in `pyproject.toml`.

## Coding Style & Naming Conventions
- Python with type hints; 4-space indentation; prefer small, cohesive modules.
- Functions/variables use `snake_case`; classes `PascalCase`; constants `UPPER_SNAKE_CASE`.
- Order imports: stdlib → third-party → local; avoid side effects in module scope beyond constants.
- Keep commands composable: parse/dispatch separately from command handlers; isolate I/O (e.g., subprocess calls) for easier testing.

## Testing Guidelines
- Mirror `src/` layout (e.g., `tests/test_cli.py`, `tests/commands/test_history.py`).
- Use builders/fixtures for config/history data; avoid touching the real data dir in tests—prefer temp dirs and injection.
- Target meaningful coverage on command routing, config persistence, and history behavior; mock subprocess invocations for `sh`/`!`.

## Commit & Pull Request Guidelines
- Conventional commits (`feat:`, `fix:`, `chore:`, `docs:`); keep subjects ≤72 chars and scoped to a single change.
- PRs should include a summary, test plan (commands run), and, when behavior changes, example invocations or transcripts.
- Update README/docs alongside new commands so users understand flags and expected output.

## Security & Configuration Tips
- Config/history now live under `~/.lexi-cli/` (legacy `~/.exp-cli/` migrates automatically); avoid writing secrets into history. Use environment variables for credentials if future commands require them.
- Handle subprocess output carefully—validate/sanitize inputs before passing to the shell. Avoid `shell=True` for untrusted input if you add new commands.
- Document required Python versions and dependency pins in `pyproject.toml`; avoid committing virtual environments.
