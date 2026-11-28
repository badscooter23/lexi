# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture Overview

Lexi is a CLI-first tool for working with multiple LLM providers side by side, with a native desktop app. Everything is a Rust workspace; Python gets the engine as a library (the `lexi` wheel).

- **lexi-core**: Rust engine — the provider-agnostic, pure (no-I/O) schema-driven request composition, response extraction, message IR, and definition validation. Behavioral parity pinned by the `conformance/` oracle.
- **lexi-host**: Rust runtime (I/O layer over the engine) — HTTP/SSE transport, config/key loading, auth, streaming, polling, uploads, first-run seeding, and the admin read/write helpers over `~/.lexi-cli/`. Every user-facing operation lives here once.
- **lexi-cli-rs**: the `lexi` CLI — a library (`lexi_cli`: command tree, parser, dispatcher, shell, output) plus a thin binary. Complete command surface; `--json` machine mode on every command.
- **lexi-desktop**: native [Tauri](https://tauri.app) app — a thin webview shell whose `/api/*` calls are served in-process by `lexi-host` (see `lexi-desktop/src/backend.rs`). No Node, no Python, no localhost server.
- **lexi-python**: PyO3 wheel (`import lexi`) — the Python library. `src/lib.rs` wraps the `lexi-core` engine; `src/host.rs` wraps the `lexi-host` runtime (respond / stream / jobs / generate / upload, plus the admin ops via one `_admin("group.verb", args)` dispatcher) on a shared tokio runtime with the GIL released; the public API is `python/lexi/` (maturin mixed layout, native module `lexi._lexi`; admin namespaces `providers`, `models`, `prompts`, `config`, `apis`). Results use the CLI's `--json` shapes, built by the same `lexi-host` code (`lexi_host::admin`, `PreparedChat::result_value`, `OperationStatus::to_value`) — put new result shaping there, not in a surface.

Surface principle (`docs/cli-consolidation-plan.md` §3): library, language bindings, CLI and GUI all sit on the library; every `lexi-host` operation must be reachable from the CLI and from Python. `docs/surface-matrix.md` lists them and `lexi-cli-rs/tests/surface_matrix.rs` enforces it — a new `pub fn` in `lexi-host` fails CI until it is listed there with its CLI command and `lexi.*` Python function (and a new public Python function fails until it is listed).

### CLI Architecture
- Grammar: plural nouns with verb subcommands (`prompts list`, `models alias set`, …), kept from the original Python CLI. Command handlers live in `lexi-cli-rs/src/commands/` (one module per group).
- `lexi` with no command starts the interactive shell (`src/shell.rs`, rustyline): history (`~/.lexi-cli/history`), `#N` replay, `!cmd`, `?`, command aliases (`~/.lexi-cli/aliases.json`, which also expand for one-off commands and never shadow real commands).
- `respond` is the one command for every output (text, images, or both): bare words are the prompt text, `--prompt NAME` a saved prompt, `--output image` asks for the provider's image API; artifacts are saved by `lexi_host::generate::save_artifacts` (`<prompt-slug>-<n>.<ext>`, never overwriting). It pre-scans `--model`/`--output`, resolves the model, then parses schema flags from *that* API's definition (`x-cli`), so they can't be declared to clap. `generate` is a shortcut for `respond --output image`. With `--model` and no `--output`, the model's API decides the output (an image model makes images); vector-only models are refused (use `embed`).
- `embed` (`lexi_host::embed`, `commands/embed.rs`): texts → vectors through the provider's `kind:embedding` binding (`lexi-core/apis/embeddings.yaml`; vectors via `lexi_core::extract_embeddings`). An API whose `request.path` contains `{model}` gets the model id in the URL (`nvidia-genai-image`), and `model` is only put in the body when the schema has it.
- Commands write through `output::Io` (never directly to stdio), so `lexi_cli::run_with` / `execute_line` work in-process with captured output.
- Config-driven API abstraction: provider auth, request body, and response parsing are defined in YAML, not hardcoded.

### Providers, APIs and model tags
- Design: `docs/provider-model-design.md`. A provider offers one or more **bindings** (`apis: [{api, for?, default_model?, url?}]`); the older single `api:` still reads as one binding.
- Shipped **provider templates** (`lexi-core/providers/*.yaml`, embedded) — an entry with `template: openai` is the template merged with the entry (`lexi_core::merge_provider_template`); `config::load_providers` applies it.
- **Tags** (closed vocabulary in `lexi_core::models`: `in:`/`out:`/`kind:`/`feature:`/`status:`, plus free-form `label:`) describe models; layers: API defaults → provider `tag_rules` (globs) → catalog → user tags (`providers/<p>/model-tags.yaml`). `lexi_core::classify_model` picks the binding (first `for:` match, else a catch-all whose API serves the model's kind); `lexi_host::models::resolve_model` is the entry point for respond/generate. Pinned by `conformance/cases/model/`.
- **Catalog** (`catalog:` in a template: path, pagination, `tags`/`limits`/`name` value rules → `lexi_core::catalog_tags`/`catalog_facts`): `models hosted` fetches and caches `providers/<p>/catalog.json`; classification reads only the cache (no hidden network calls), so catalog models are usable unconfigured. `prepare_chat` checks requests against the model (`ResolvedModel::check_request`: `in:image` for images, output-token limits — a default over the limit is lowered, an explicit one refused; deprecated → warning); `--no-validate` skips it.
- `providers upgrade` (`config::upgrade_provider`) moves an entry to `template:` form, dropping values equal to the template or the pre-template shipped defaults (`lexi-core/providers/legacy-providers-config.yaml`) and keeping the rest (a custom `api`/`default_model` becomes a binding); backups are `providers-config.yaml.bak-<secs>[-n]`, never reused. `providers api add|rm` edit bindings (`add_provider_api` / `remove_provider_api`); removing a template's binding writes `{api, enabled: false}` (`lexi_core::provider_bindings` skips disabled ones).
- Put provider-specific knowledge in templates / tag rules / API definitions, never in code.

### Configuration Structure
- Config dir: `~/.lexi-cli/` (override with `$LEXI_HOME`)
- Main provider config: `providers/providers-config.yaml`
- Per-provider: `providers/<provider>/configured-models.yaml`, `model-aliases.yaml`, `default-model.yaml` (your default per kind: `default_model` is chat, `defaults: {kind: model}` the rest), `model-tags.yaml` (your tags), `catalog.json` (cached listing), `api-key` / `api-key-<api>` (0600; the saved key where there's no OS key store, or `key_store: file`)
- Command aliases: `aliases.json`; CLI settings: `config.json` (also `request_timeout`: seconds a provider request may take, default 300, 0 = none — a buffered request's whole wait, a stream's longest silence; `respond::within_limit`; one operation can have its own limit with `with_request_timeout` — a tokio task-local read by `config::request_timeout`, so it covers every request in that future, not ones it spawns — which is the CLI's `--timeout` and Python's `timeout=`); shell history: `history`
- Desktop themes: `themes.json` (every theme's colours; copied from the shipped `lexi-desktop/themes.json` on first use, then the user's copy is what's read); the chosen theme is `theme` in `config.json` (default `dark-cyan`). A colour may be a list (the first value the web engine supports wins) and `scheme: auto` follows the OS Light/Dark setting; the System theme uses WebKit's `-apple-system-*` colours (accent, labels, backgrounds) with CSS system-colour fallbacks
- Prompts: individual JSON files in `prompts/` (the active prompt is `$$`)
- API definitions: shipped ones are embedded in the binary (`lexi-core/apis/`) and always win; `apis/` holds custom definitions only

## Development Commands

```bash
cargo test                     # all crates (CLI integration tests run the real binary)
cargo clippy --all-targets     # lint (CI gates -D warnings)
cargo fmt --all                # format
cargo run -p lexi-cli-rs -- …  # run the CLI from source
cargo install --path lexi-cli-rs   # install `lexi` (or ./install.sh)

# Regenerate docs/cli-reference.md after changing the command tree
LEXI_UPDATE_DOCS=1 cargo test -p lexi-cli-rs --test cli_reference

# Desktop app (Tauri)
cargo install tauri-cli --version '^2'   # once
cd lexi-desktop && cargo tauri dev        # run with devtools
cd lexi-desktop && cargo tauri build      # native installer

# The Python wheel builds via maturin, not `cargo build` (PyO3 extension module):
pip install ./lexi-python pytest pyyaml && pytest lexi-python/tests
```

## Key Implementation Details

### Provider System
- Three preconfigured providers, each from a shipped template: OpenAI (`responses` + `image-generations`), Anthropic (`messages`), NVIDIA (`chat-completions`)
- API key lookup (`lexi-host/src/keystore.rs`, `config::resolve_api_key`; design: `docs/key-storage-design.md`): a provider's `api_key` is an ordered list of sources Lexi never writes — `{env: VAR}` (or `"${VAR}"`), `{file: PATH}` (relative to `$LEXI_HOME`, `~/` expanded) and `{command: [PROGRAM, ARG…]}` (no shell, 60 s, output cached per process by argv; refused when providers-config.yaml is group/world-writable) — then the **saved key** (`lexi-host/src/key_store.rs`: the macOS Keychain via `/usr/bin/security` — never the Security framework; service `Lexi`, account `<p>[/<api>][@<LEXI_HOME> when not ~/.lexi-cli]`, the key passed on stdin to `security -i` as hex — else the 0600 file `providers/<p>/api-key`, `api-key-<api>` for an API's own; `key_store` in config.json `auto|system|file`, overridden by `LEXI_KEY_STORE`; every test harness sets `LEXI_KEY_STORE=file`, unit tests always get the file store, and the real-keychain tests run only with `LEXI_TEST_KEYCHAIN` set, against a throw-away keychain), which is implicit: looked in last unless the marker `saved` places it. The first with a key wins; only a *missing* key moves on, a read error stops and is the error. Listings (`api_key_source`, `api_key_info` without reveal, the desktop startup) don't run commands: one is `not run` and counts as the source. An API binding's or configured model's own `api_key` replaces the provider's (resolve from the effective config; `config::KEY_SCOPE` in it says whose saved key applies — a model has none). Saving (`set_api_key`) writes only the saved key (one copy: a leftover key file is deleted once the keychain has it; a locked keychain is an error, never a file fallback); providers-config.yaml changes only to give an API its own `[saved]` list or drop a literal key. `providers api-key source list|add|rm` (`key_source_list` / `add_key_source` / `remove_key_source`) edit the list. `ensure_home` (`upgrade_key_sources`) rewrites `{keychain: …}` to `saved`, takes Lexi's own key file out of lists (0.3.0 listed it), and gives an old (0.2.0) key file `[saved, …]`; `move_saved_keys` then moves key files into the keychain (read back before the file is deleted) and queues a notice (`take_notices`: the CLI prints it, the app shows it at startup, Python warns). Rename/remove provider carry saved keys along
- Model resolution (`lexi_host::models::resolve_model`): `--model` (aliases resolved; must be configured or recognized by a `kind:` tag from a rule or your tags), else the default for the kind the binding producing the requested output serves (`kind_default`: yours from `default-model.yaml`, then the binding's `default_model`, then — catch-all only — the provider's; `models::defaults` / `set_default` (kind inferred from the model's tags) / `clear_default`); per-model `auth`/`request`/`response` overrides are deep-merged; an explicit `api:` on a configured model pins its binding
- Providers reference an API definition with `api:` (or define `request`/`response` inline); provider entries deep-merge over it

### Configuration Seeding
On every run, `lexi_host::ensure_home` fills in missing defaults (embedded from the root `providers/` directory and `providers/default-aliases.json`). It never overwrites existing files.

### Tests
- CLI: `lexi-cli-rs/tests/` — the binary end to end against a temp `LEXI_HOME` and a wiremock provider, plus the surface-matrix and CLI-reference guards
- Runtime: `lexi-host/tests/` (wiremock) and unit tests
- Engine: `lexi-core/tests/conformance.rs` runs `conformance/`; the wheel runs the same fixtures (`lexi-python/tests/`)
- Python runtime: `lexi-python/tests/test_runtime.py` — the wheel against an in-process mock provider

## File Locations
- Rust engine / runtime / CLI / desktop: `lexi-core/`, `lexi-host/`, `lexi-cli-rs/`, `lexi-desktop/`
- Desktop frontend shared UI (in `index.html`): `DataTable` (sticky header, click-to-sort, right-click column filters), `openDialog` / `confirmDialog` (modal dialogs; don't use `window.confirm`), and `data-tip="…"` hover help (give every icon-only or non-obvious control one; native `title` tooltips don't reliably show in the app) with `data-cmd="NAME"` on every action: a key of `CLI_COMMANDS`, which builds the `lexi` command line and Python for it from what's on screen (the tip's ">_"), or `"none"` — a desktop test fails for a `<button>` with `data-tip` but no `data-cmd`, and `setStatus(text, tone)` — the message area at the top right, with a recent-messages list on click: every action reports what it did there (tones `success`/`error`/`warn`/`muted`, and `busy` for work in progress, which isn't kept in the list); don't add per-tab status lines
- Shipped API definitions: `lexi-core/apis/` (embedded by `lexi-host`)
- Default provider configs: `providers/` (embedded; seeded into `~/.lexi-cli/providers/`)
- Python wheel: `lexi-python/` (PyO3 over lexi-core + lexi-host; Python API in `lexi-python/python/lexi/`)
- Desktop `/api/*` backend: `lexi-desktop/src/backend.rs`; frontend: `lexi-desktop/frontend/index.html`
- Desktop CLI tab: `lexi-desktop/src/console.rs` runs a line in-process with `lexi_cli::execute_line` (Tauri commands `cli_run` — streamed `out`/`err`/`input`/`done` events, cancelled by `cancel_request` — plus `cli_input` and `cli_history`); `sh`/`!` are off. Commands that need typed input (a key, `-` text) ask through `Io::input` (`io.read_secret` / `io.read_text`), never stdin directly, so the console can show a field (masked for secrets)
- Desktop startup screen (`runStartup` in `index.html`, `GET /api/startup` in `backend.rs`): shown while the providers' keys are checked; it closes itself when this version has been opened before (`seen_version` in `config.json`) and some provider has a key, else it shows the unseen release notes and/or sends you to the **Setup Wizard** (`openSetupWizard`; per-provider help is the template's `setup:` block). The header logo reopens it
- Release notes: `CHANGELOG.md` (repo root, embedded in the desktop app) — add user-facing notes under `## Unreleased` with each change; at release time it becomes `## X.Y.Z — date`. A desktop test fails unless the app's version has a section
- Desktop Images tab: `/api/images/*` in `backend.rs`, the `respond --output image` pipeline sent to each ticked provider (each with its own model and settings — the image API's `x-cli` flags); the Chat tab's model pickers list only `kind: chat` models; images are saved to `images_dir` in `config.json` (default `~/Pictures/Lexi`)
- Cross-language conformance oracle: `conformance/`
- Docs: `docs/cli-reference.md` (generated), `docs/spec/` (API definition, message IR, CLI machine interface), `docs/surface-matrix.md`
