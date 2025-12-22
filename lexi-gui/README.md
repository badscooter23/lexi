# lexi-gui

GUI for Lexi prompts. Serves a static UI (`public/`) and fronts the `lexi prompt` CLI to create, save, list, and activate prompts.

## Structure
- `src/server/index.js` — HTTP server serving `public/` and proxying prompt management to the `lexi` CLI.
- `public/` — Static assets (HTML/JS/CSS) for the prompt manager screen.

## Setup & Running
- Requirements: Node 18+ and the `lexi` CLI available on PATH (or set `LEXI_BIN` env to the CLI path).
- Local dev: `npm install`, then `npm run start` (or `npm run dev` for watch). Open `http://localhost:3000`.
- Global install: `npm install -g .` then `lexi-gui` to start the server.

## How it works
- The server calls `lexi prompt list`/`set`/`rm` under the hood. Prompts are stored by the CLI in `~/.lexi-cli/prompts/` (active prompt is `$$.json`).
- The UI supports:
  - Creating/updating the active prompt or a named prompt (overriding max_tokens/temperature/role or using defaults).
  - Listing prompts (active shown first), viewing details, setting a named prompt active, and deleting prompts.

## Development Notes
- Adjust server endpoints in `src/server/index.js` when CLI contracts evolve. The helper `runLexi` calls the `lexi` binary (override via `LEXI_BIN`).
- Keep the single-page UI in `public/index.html`; add tests under `tests/` mirroring `src/` if you introduce build tooling.
