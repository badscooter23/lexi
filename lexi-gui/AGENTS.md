# Repository Guidelines

## Project Structure & Module Organization
- `src/server/index.js`: Node HTTP server that serves `public/` and proxies prompt actions to the `lexi` CLI.
- `public/`: Static UI assets; single-page prompt manager.
- Add new code under `src/` with focused modules; mirror structure under `tests/` (use `tests/fixtures/` for sample payloads).

## Build, Test, and Development Commands
- `npm install`: Install Node dependencies.
- `npm run start`: Start the server at `http://localhost:3000`.
- `npm run dev`: Run the server with `node --watch` for local iteration.
- Global bin: `npm install -g .` then `lexi-gui` starts the server.
- When you add tooling, expose it via scripts (`npm test`, `npm run lint`, `npm run format`).

## Coding Style & Naming Conventions
- JavaScript (CommonJS) with 2-space indentation, single quotes, trailing commas where supported.
- Prefer named exports; utilities camelCase; components/pages PascalCase if/when added. Asset filenames use kebab-case.
- Order imports: built-ins → external → internal; keep server handlers small and composable.
- If adding lint/formatting, prefer ESLint + Prettier; configure in `package.json`/config files.

## Testing Guidelines
- Co-locate tests under `tests/` mirroring `src/` (`tests/server/index.test.js`, etc.).
- Mock subprocess calls to the CLI to keep tests deterministic.
- Target meaningful coverage on request handling, error paths, and payload shape; add regression tests for reported issues.

## Commit & Pull Request Guidelines
- Use Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`, `test:`); subjects ≤72 chars.
- PRs include: summary, testing notes (commands run), and screenshots/GIFs for UI changes.
- Keep changes scoped; update docs/examples when altering endpoints or UI flows.

## Security & Configuration Tips
- Do not commit secrets; introduce `.env.local` for local overrides and mirror safe defaults in `.env.example` if env vars are added.
- Validate/limit input sizes at the server boundary (keep the 1MB guard); handle CLI errors clearly in responses.
- Document runtime requirements (Node/Python versions) in `README` or `.nvmrc` as the stack solidifies.
