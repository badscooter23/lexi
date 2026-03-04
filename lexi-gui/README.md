# lexi-gui

GUI wrapper for Lexi. Serves a web UI and proxies to the `lexi` CLI backend for prompt management and provider interaction.

## Setup

```bash
# Requires Node 18+ and `lexi` CLI on PATH
npm install
npm run start          # http://localhost:3000
npm run dev            # Development mode with --watch
```

Or use the convenience script:
```bash
./launch-gui.sh
```

## How It Works

- `src/server/index.js` — HTTP server that serves `public/` and proxies commands to the `lexi` CLI
- `public/index.html` — Single-page web UI
- The server calls `lexi` subcommands under the hood (override binary path via `LEXI_BIN` env var)
- Falls back to `lexi-cli/cli` in the repo if `lexi` is not on PATH

## Features

- Create, update, and delete prompts (active prompt shown as `$$`)
- List and inspect saved prompts with detail views
- Set named prompts as active
- Configurable max_tokens, temperature, and role per prompt

## Development

```bash
npm install
npm run dev            # Watch mode
npm test               # Run tests (Jest)
npm run lint           # Lint (ESLint)
npm run format         # Format (Prettier)
```

Adjust server endpoints in `src/server/index.js` when CLI contracts evolve. Keep the single-page UI in `public/index.html`.
