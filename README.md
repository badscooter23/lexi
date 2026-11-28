# Lexi

[![CI](https://github.com/badscooter23/lexi/actions/workflows/ci.yml/badge.svg)](https://github.com/badscooter23/lexi/actions/workflows/ci.yml)

Lexi gives you one way to work with many LLM providers — OpenAI, Anthropic and
NVIDIA out of the box, and any other that speaks one of the APIs it knows —
from the command line, a native desktop app, or Python. All three are built on
the same Rust library and share one configuration, so a provider, key, model
default or saved prompt set up in one works in the others.

- **One request for every output:** `lexi respond` (Python: `lexi.respond`)
  asks for text, images or both, and the model decides which of the
  provider's APIs is called; embeddings have their own `embed`.
- **Side by side:** the desktop app sends one prompt to several providers at
  once and shows the answers (or images) together.
- **Described, not coded:** how each provider's API is called — its request,
  response and settings — is defined in YAML, so adding a provider or an API
  style is a configuration change.

Configuration lives in `~/.lexi-cli/` (or `$LEXI_HOME`). Each time it runs,
Lexi fills in any defaults that are missing and never overwrites your edits.

## Install

Lexi comes in three parts. Install whichever you'll use; they share the same
configuration, keys and prompts in `~/.lexi-cli/`.

| Part | What you get |
|---|---|
| **Desktop app** | The Lexi app: chat and images with several providers side by side, a setup wizard for API keys, and a CLI tab. It doesn't put a `lexi` command on your PATH. |
| **CLI** | The `lexi` command for a terminal: one native binary, nothing else to install. |
| **Python library** | `import lexi` in your own programs (CPython 3.9+). |

Everything is on the [latest GitHub Release](https://github.com/badscooter23/lexi/releases/latest).
The commands below use version 0.3.0; replace it with the latest release's number.

### Desktop app

Download and run the installer for your system:

| System | File |
|---|---|
| macOS (Apple silicon and Intel) | `Lexi_0.3.0_universal.dmg` — open it and drag Lexi to Applications |
| Windows | `Lexi_0.3.0_x64-setup.exe` (or the `.msi`) |
| Linux | `Lexi_0.3.0_amd64.deb`, `Lexi-0.3.0-1.x86_64.rpm`, or the portable `Lexi_0.3.0_amd64.AppImage` |

The builds aren't signed yet, so the first time:
- **macOS** refuses to open the app. Open **System Settings → Privacy & Security**
  and click **Open Anyway** next to the message about Lexi. (On macOS 14 and
  earlier, you can instead right-click the app and choose **Open**.)
- **Windows** SmartScreen warns about it: choose **More info → Run anyway**.

When it opens, the app checks your providers and, if none has a key yet, takes
you to the **Setup Wizard**, which explains how to get a key and saves it for you.

### CLI

Download the archive for your system, then put `lexi` somewhere on your PATH.

macOS (one universal binary) or Linux (`linux-x86_64` or `linux-aarch64`):

```bash
V=0.3.0; P=macos-universal            # or linux-x86_64 / linux-aarch64
curl -L "https://github.com/badscooter23/lexi/releases/download/v$V/lexi-$V-$P.tar.gz" | tar xz
mkdir -p ~/.local/bin && mv "lexi-$V-$P/lexi" ~/.local/bin/   # needs ~/.local/bin on your PATH
lexi version
```

Downloaded with `curl` like this, macOS runs it as is. If you downloaded the
archive in a browser instead, macOS blocks the unsigned binary; clear that with
`xattr -d com.apple.quarantine ~/.local/bin/lexi`.

Windows: download `lexi-0.3.0-windows-x86_64.zip`, unzip it, and move
`lexi.exe` into a folder on your PATH.

Then give Lexi a key and try it:

```bash
lexi providers api-key set anthropic # asks for the key (not shown as you type)
lexi providers status anthropic      # checks the key (lists models; nothing is billed)
lexi respond anthropic "Explain quicksort in one paragraph"
lexi                                 # the interactive shell
```

### Python library

Install the wheel for your system from the release (it isn't on PyPI: the
`lexi` name there is taken). For example, on an Apple-silicon Mac:

```bash
pip install "https://github.com/badscooter23/lexi/releases/download/v0.3.0/lexi-0.3.0-cp39-abi3-macosx_11_0_arm64.whl"
```

The other wheels end in `macosx_10_12_x86_64` (Intel Mac), `win_amd64`,
`manylinux2014_x86_64…` and `manylinux2014_aarch64…`. See
[the Python library](#python-library-import-lexi) below.

### Building from source

For working on Lexi itself (or a platform without a release build), clone the
repository and install a [Rust toolchain](https://rustup.rs). Each part builds
separately:

```bash
./install.sh                              # the CLI only: cargo install --path lexi-cli-rs (into ~/.cargo/bin)

cargo install tauri-cli --version '^2'    # the desktop app (once)
(cd lexi-desktop && cargo tauri dev)      # run it with devtools
(cd lexi-desktop && cargo tauri build)    # or build its installers (target/release/bundle/)

pip install ./lexi-python                 # the Python library (builds the wheel with maturin)
```

See [Development](#development) for tests and linting.

## API keys

Each provider looks for its key in order: an environment variable
(`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `NVIDIA_API_KEY`), then the key Lexi
saved. `lexi providers api-key set <provider>` (or the desktop app's Setup
Wizard) saves the key in your OS key store — the macOS Keychain, Windows Credential
Manager or the Linux desktop keyring (elsewhere, an owner-only file under
`~/.lexi-cli/providers/<provider>/`) — never in `providers-config.yaml`; `providers api-key show <provider>` lists
where Lexi looked and which key is used. Files of your own and password
managers (`command: [op, read, …]`) can be added with
`providers api-key source add` — see
[Getting Started](docs/getting-started.md#where-a-key-comes-from). The CLI, the desktop app and the
Python library all read the same places.

## Core Commands

All main commands use **plural forms** and support subcommands. Use `help <command>` for details,
and add `--json` to any command for machine-readable output ([spec](docs/spec/cli-machine-v1.md)).

| Command | Description |
|---|---|
| `respond <provider> [TEXT] [--prompt NAME] [--model M] [--output image] [--stream] …` | Send a prompt; get back text, images (saved to disk), or both |
| `embed <provider> TEXT…` · `generate <provider> <prompt>` · `upload <provider> <file>` | Texts → vectors · shortcut for `respond --output image` · upload to a Files API |
| `models tag\|tags` · `tags` | Tag models (what they take, produce, support) · the tag vocabulary |
| `jobs status\|wait` | Background jobs (`respond --background --detach`) |
| `prompts list\|set\|rm\|activate` | Manage saved prompts |
| `providers list\|add\|rename\|rm\|status\|api-key\|api\|upgrade` | Manage LLM providers, their keys and APIs; move older configs to the shipped templates |
| `models list\|hosted\|add\|set\|rm\|default\|alias` | Manage models for a provider |
| `api list\|show\|create\|edit\|save\|rm\|validate\|preview` | Manage API definitions |
| `alias list\|add\|rm\|reset` | Manage command aliases |
| `config list\|set\|rm` | View or update settings (e.g. `request_timeout`) |
| `help`, `history`, `sh`, `version`, `exit` | General commands |

### Examples

```bash
# Save a prompt and send it to Anthropic
prompts set myq --prompt "Explain quicksort in one paragraph"
respond anthropic --prompt myq

# Send text, streaming, with a specific model and a schema flag
respond openai "Hello" --model gpt-4o --stream --temperature 0.2

# An image: the model's tags pick OpenAI's Images API; saved to ./images
respond openai "a lighthouse at dusk" --output image --size 1536x864 --out ./images

# Text and an image in one reply (the Responses API's image tool)
respond openai "Draw a fox and describe it" --tool image_generation

# Use the active prompt ($$), with web search and citations
respond openai --tool web_search --cite

# List hosted models from a provider
models hosted nvidia --table

# Set a default model (for its kind: chat, image-generation, embedding, …)
models default anthropic claude-sonnet-4-5-20250929
models default openai gpt-image-2

# Give one slow request 15 minutes (the default limit is request_timeout, 5 minutes)
respond openai "Write a detailed design doc for …" --timeout 900

# Check a provider's status and API key validity
providers status openai

# List available API definitions
api list

# Create a custom API definition from an existing one
api create my-api --from chat-completions
```

### Shortcuts

| Shortcut | Action |
|---|---|
| `?` | Help |
| `!<cmd>` | Run shell command (terminal only, not the desktop app's CLI tab) |
| `#<N>` | Re-run history entry |
| Arrow keys | Navigate command history |

## Configuration

All persistent configuration lives in `~/.lexi-cli/` (or `$LEXI_HOME`). Each
time it runs, Lexi fills in whatever is missing there — provider configs, model
metadata and default command aliases — and leaves files that are already there,
including your edits, alone.

```
~/.lexi-cli/
├── apis/                              # Custom API definitions (shipped ones are built in)
├── providers/
│   ├── providers-config.yaml          # Providers: template, where each key comes from, overrides
│   ├── anthropic/
│   │   ├── api-key                    # The key Lexi saved (owner-only), where there's no key store
│   │   ├── configured-models.yaml     # Model metadata
│   │   ├── default-model.yaml         # Your default model for each kind (chat, image, …)
│   │   ├── model-aliases.yaml         # Shorthand aliases (e.g., "sonnet")
│   │   ├── model-tags.yaml            # Your model tags
│   │   └── catalog.json               # Cached list of the provider's models
│   ├── openai/
│   │   └── ...
│   └── nvidia/
│       └── ...
├── prompts/                           # Saved prompts as JSON files
├── aliases.json                       # Command aliases
├── history                            # Shell history (shared with the app's CLI tab)
├── themes.json                        # Desktop app themes
└── config.json                        # Settings (request_timeout, images_dir, theme, …)
```

### API Definitions

Each LLM API style (Chat Completions, Messages, Responses, Image Generations, Embeddings, NVIDIA's image API) is defined in a standalone YAML file. The shipped definitions are built into the binary (`lexi api list`, `lexi api show <name>`); custom ones live in `~/.lexi-cli/apis/` (`lexi api create mine --from messages`). An API definition specifies:

- **`auth`** — Header name, value prefix, and any extra headers
- **`request`** — HTTP method, path, body template with `{{model}}`/`{{role}}`/`{{prompt}}` variables, typed parameters with ranges and defaults, and optional tools config
- **`response`** — `text_path` for extracting response text, plus `metadata` paths for token counts, model, and stop reason

The three shipped providers build on templates that come with Lexi (their URL,
key variable, the APIs they offer and which models each API serves), so
`providers-config.yaml` starts out short; anything you add to an entry wins:

```yaml
providers:
  openai:
    template: openai       # Responses for chat, Images, Embeddings
  anthropic:
    template: anthropic    # Messages
  nvidia:
    template: nvidia       # Chat Completions, Embeddings, NVIDIA's image API
```

A provider of your own names its URL, the API it speaks and where its key comes
from:

```yaml
  mine:
    url: https://api.example.com/v1
    api: chat-completions
    api_key: [{env: MINE_API_KEY}]     # then the key Lexi saved
    default_model: some-model
```

Adding a new provider that uses an existing API style requires only a new entry in `providers-config.yaml` (or `lexi providers add`). Adding a new API style requires only a new YAML file in `apis/` — no code changes needed. See [docs/provider-model-design.md](docs/provider-model-design.md).

Providers can also include inline `auth`, `request`, or `response` overrides that are deep-merged on top of the referenced API definition.

## Desktop app

`lexi-desktop` is a native [Tauri](https://tauri.app) app — a thin webview shell
over the Rust `lexi-host` runtime and `lexi-core` engine. No Node, no Python, no
localhost server. It reuses the same `~/.lexi-cli/` config as the CLI, and
everything is served in-process by `lexi-host`:

- **Chat** — one prompt to several providers at once, answers side by side
  (formatted Markdown), streamed or not; Stop cancels.
- **Images** — generate with several providers at once, each with its own model
  and settings; images are saved to a folder you choose.
- **Providers, Models, Prompts, APIs, Configure** — the full admin surface:
  keys (with a step-by-step Setup Wizard), models and a default per kind,
  saved prompts, API definitions, settings and themes.
- **CLI** — run `lexi` commands inside the app, with your history.
- **The command for any action** — hover a button and click **>_** to see the
  `lexi` command and the Python that do the same thing (Copy, or **▶ CLI** to
  run it in the CLI tab).

Installers are on each GitHub Release ([Install](#desktop-app)); building it
yourself is under [Building from source](#building-from-source). See
`lexi-desktop/README.md`.

## Python library (`import lexi`)

Lexi is also an `abi3` Python wheel — a PyO3 binding over the same Rust core
the CLI uses, reading the same `~/.lexi-cli` config, keys and prompts:

```python
import lexi
print(lexi.respond("openai", "Say hello")["text"])
for ev in lexi.stream("anthropic", "Count to five"):
    if ev["event"] == "delta":
        print(ev["text"], end="")
lexi.generate("openai", "a lighthouse at dusk", out_dir="images", timeout=600)
```

It also manages that config — `lexi.providers`, `lexi.models`, `lexi.prompts`,
`lexi.config`, `lexi.apis` mirror the CLI's admin commands. Results are the
CLI's `--json` shapes. To install it, see [Install](#python-library); the
details are in `lexi-python/README.md`.

## Development

```bash
# Rust workspace (engine / runtime / CLI / desktop)
cargo test             # all crates
cargo clippy --all-targets
cargo fmt --all
cargo run -p lexi-cli-rs -- providers list   # run the CLI from source

# Python wheel (bindings)
pip install ./lexi-python pytest pyyaml && pytest lexi-python/tests
```

## Documentation

- [Getting Started](docs/getting-started.md) — Install, configure, and send your first prompt
- [CLI Reference](docs/cli-reference.md) — Complete command reference (generated from the CLI)
- [CLI machine interface](docs/spec/cli-machine-v1.md) — The `--json` contract for scripts and agents
- [API Definitions](docs/api-definitions.md) — How the config-driven API abstraction works
- [Architecture](docs/architecture.md) — System design and developer internals

## License

MIT
