# Getting Started with Lexi

Lexi is a CLI-first tool for working with multiple LLM providers side by side. This guide walks you through installation, configuration, and sending your first prompt.

## Installation

**Prebuilt binary:** download the `lexi` archive for your platform from [GitHub Releases](https://github.com/badscooter23/lexi/releases) and put the `lexi` binary on your `PATH`:

| Platform | Archive |
|---|---|
| macOS (Apple Silicon + Intel) | `lexi-<version>-macos-universal.tar.gz` |
| Linux x86_64 | `lexi-<version>-linux-x86_64.tar.gz` |
| Linux aarch64 | `lexi-<version>-linux-aarch64.tar.gz` |
| Windows x86_64 | `lexi-<version>-windows-x86_64.zip` |

**From source** (requires stable Rust):

```bash
# Clone the repo
git clone https://github.com/badscooter23/lexi.git
cd lexi

# Build and install the `lexi` binary
cargo install --path lexi-cli-rs

# Verify it works
lexi version
```

Run `lexi` with no command for the interactive shell, or `lexi <command>` for a one-off command:

```bash
lexi                      # Interactive shell
lexi providers list       # One-off command
```

The examples below are written as you would type them inside the shell; outside it, prefix them with `lexi`.

> Looking for the Python library? The `lexi` wheel (`import lexi`, built from `lexi-python/`) is attached to GitHub Releases. It uses the same config as the CLI — `lexi.respond("openai", "hi")` — see `lexi-python/README.md`.

## First Run

Lexi keeps its state in `~/.lexi-cli/` (set `$LEXI_HOME` to use a different directory). On first run it seeds any missing defaults:

- **Provider configs** — `providers/providers-config.yaml`, with preconfigured entries for OpenAI, Anthropic, and NVIDIA
- **Model metadata** — Each provider's `configured-models.yaml`, `default-model.yaml`, and `model-aliases.yaml`
- **Default aliases** — `aliases.json`, with shorthand commands (e.g., `prov` for `providers`, `q` for `exit`)

Existing files are never overwritten. The shipped API definitions (`chat-completions`, `messages`, `responses`, `image-generations`) are built into the binary and are not copied to disk; see [API Definitions](api-definitions.md).

Running `lexi` with no command drops you into an interactive shell:

```
cmd>
```

Type `help` (or `?`) to see available commands, `help <command>` for details on a specific command, or `help general|ai|shortcuts` for a topic.

## Setting Up API Keys

Before you can send prompts, you need API keys from your providers. Lexi looks for a provider's key in the places its `api_key` (in `~/.lexi-cli/providers/providers-config.yaml`) lists, in order, then in the key Lexi saved for it.

### Where a key comes from

```yaml
providers:
  openai:
    template: openai                   # api_key: [{env: OPENAI_API_KEY}] from the template
  nvidia:
    template: nvidia
    api_key:
      - file: ~/.config/secrets/nvidia.key   # a file holding the key
  anthropic:
    template: anthropic
    api_key:
      - command: [op, read, "op://Private/Anthropic/credential"]   # a password manager
```

| Source | Meaning |
|---|---|
| `env: VAR` | The environment variable `VAR` (`"${VAR}"` is the same, the older form). |
| `file: PATH` | A file holding the key — yours (a Docker or Kubernetes secret, a key on an encrypted disk): Lexi never writes or deletes it. A relative path is under `~/.lexi-cli`, and `~/` is your home folder. |
| `command: [PROGRAM, ARG, …]` | A program that prints the key: a password manager (`op read …`, `bw get password …`, `pass show …`) or an encrypted file (`sops -d …`, `age -d …`). It runs without a shell, from `~/.lexi-cli`, for up to 60 seconds (time for a Touch ID or unlock prompt); its output, trimmed, is the key. It runs once per command or Python process, and only when a request needs the key — listing providers or opening the app doesn't run it. Lexi won't run commands from a providers-config.yaml other users can change. |
| `saved` | Where the key Lexi saved is looked in. Without it, the saved key is looked in last. |

The first source that has a key wins. Only a *missing* key moves on to the next one: an unset variable, no file, an empty file, a command that prints nothing. A read error — an unreadable file, a command that fails — stops the lookup and is reported, so Lexi never quietly switches to a different key (and bills a different account). When no source has a key, the error lists every place Lexi looked.

A single source can be written without the list (`api_key: {env: OPENAI_API_KEY}`). A literal key (`api_key: sk-…`) still works but isn't recommended.

The shipped templates read the usual environment variable (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `NVIDIA_API_KEY`), so setting it is enough to get started:

```bash
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
export NVIDIA_API_KEY="nvapi-..."
```

Add these to your shell profile (`~/.zshrc`, `~/.bashrc`, etc.) to persist them. Apps opened from the macOS Dock or Finder don't see variables set in your shell profile. For the desktop app to use one, run `launchctl setenv OPENAI_API_KEY "…"` and reopen it, or save the key instead.

To change the list without editing the file:

```bash
providers api-key source list openai                                     # where it's looked for, in order
providers api-key source add openai --file ~/keys/openai --first         # look here first
providers api-key source add openai --command 'op read op://Private/OpenAI/credential'
providers api-key source add openai --saved --first                      # the saved key before the variable
providers api-key source rm openai --env OPENAI_API_KEY                  # stop reading the variable
```

### Saving a key

Lexi can save a key for you (the desktop app's **Setup Wizard**, on the Providers tab, does this too):

```bash
# Inside the Lexi shell
providers api-key set openai --value sk-...
providers api-key set anthropic --value sk-ant-...
```

The key goes in your operating system's key store, encrypted with your login: the **macOS Keychain** (Lexi saves it with Apple's `security` tool, so there's no password prompt), **Windows Credential Manager**, or on Linux your **desktop keyring** (GNOME Keyring or KWallet, through the Secret Service). Where there's no key store (a server, a container, CI), it goes in a file only you can read (mode 0600), `providers/<provider>/api-key` in `~/.lexi-cli`. Keys saved by an older Lexi in that file are moved into the key store once, the first time a newer one runs. Over SSH the macOS Keychain is locked until you run `security unlock-keychain`. `lexi config set key_store file` keeps saved keys in the file instead (`system` insists on the key store; the default is `auto`), and the `LEXI_KEY_STORE` environment variable overrides the setting. Saving never changes providers-config.yaml or a file you listed. Because the saved key is looked in after the listed sources, a set environment variable wins over it — `set` says so when that happens (list `saved` first to change that). Without `--value`, the key is prompted for (or read from stdin).

A development build briefly kept keys in the system keychain; Lexi rewrites a `{keychain: …}` entry to `saved`, so save the key again. Lexi 0.3.0 listed its own key file in `api_key` (`{file: providers/<provider>/api-key}`); a newer Lexi takes it out of the list (or puts `saved` in its place), and the key keeps working.

### A different key for one API or model

A provider's APIs and configured models can each have their own `api_key`, which replaces the provider's:

```yaml
  openai:
    template: openai
    apis:
      - api: image-generations
        api_key: [saved]       # its own saved key: providers/openai/api-key-image-generations
```

`providers api-key set openai --api image-generations` saves a key for that API (in `providers/openai/api-key-image-generations`) and gives it its own `api_key` for you; `show`, `remove` and `source` take `--api` too. For a model, add `api_key:` to its entry in `providers/<provider>/configured-models.yaml` (a model has no saved key of its own: list where its key is).

### Checking API Key Status

View whether a key is configured and where it comes from:

```bash
providers api-key show openai          # Each place in order, and the key in use (masked; commands aren't run)
providers api-key show openai --reveal # Shows the actual key value (runs a command source)
providers api-key remove openai        # Deletes the saved key (the config is unchanged)
```

To check that the key actually works (makes a live call to the provider's models endpoint):

```bash
providers status openai
```

`providers status` reports:
- **Source** — Where the key was found: the variable or file (or none)
- **Status** — `ready` (key works), `no api-key` (no key found), or `invalid api-key` (key was rejected by the provider)

## Working with Prompts

Lexi uses a prompt system where you create named prompts that can be sent to any provider.

### Create a Prompt

```bash
prompts set myq --prompt "Explain quicksort in one paragraph"
```

This creates a prompt named `myq` and sets it as the **active prompt** (`$$`).

### View Prompts

```bash
prompts list                    # List all saved prompts
prompts list myq                # Show details for a specific prompt
prompts list --detailed         # Include max_tokens / temperature / role
prompts activate myq            # Make a saved prompt the active one
```

### Prompt Options

Prompts can include parameters that override provider defaults:

```bash
prompts set creative --prompt "Write a haiku about recursion" --temperature 1.2 --max_tokens 100 --role user
```

## Sending Prompts to Providers

The `respond` command sends a prompt to a provider — the words you give, else a saved prompt (`--prompt NAME`), else the active one — and prints the reply. Images in the reply are saved to disk.

```bash
# Send text (`-` reads stdin)
respond anthropic "What is a monad?"

# Send the active prompt to Anthropic
respond anthropic

# Send a saved prompt to OpenAI
respond openai --prompt myq

# Override the model
respond anthropic --model claude-sonnet-4-5-20250929

# Override temperature and max tokens
respond openai --temperature 0.2 --max-tokens 2000

# Stream the response as it arrives
respond anthropic --stream

# Get raw JSON response
respond anthropic --raw
```

Flags like `--temperature` and `--max-tokens` come from the API definition serving the request — `api show <api>` lists them (the `cli=` entries). `respond` also takes `--set path=value` / `--unset path` for arbitrary body fields, `--params FILE`, `--tool <preset>`, `--image URL|PATH`, `--cite`, and `--verbose`. Long-running requests can run with `--background`; add `--detach` to return a job id immediately and check it later with `jobs status <provider> <id>` or `jobs wait <provider> <id>`. See the [CLI Reference](cli-reference.md#respond) for the full list.

Beyond text:

```bash
# Images: --output image picks the provider's image API and default image model
respond openai "a lighthouse at dusk" --output image --out ./images
respond openai "a lighthouse at dusk" --model gpt-image-2.5-flare --size 1536x864
generate openai "a lighthouse at dusk"                     # shortcut for --output image

# Text and an image in one reply
respond openai "Draw a fox and describe it" --tool image_generation

# Embeddings: one vector per text (--json for the vectors)
embed openai "first text" "second text" --dimensions 256
embed nvidia --file docs.txt --input-type passage          # NVIDIA: passage when indexing, query (default) when searching

# NVIDIA image models run on their own host; name one, or ask for --output image
respond nvidia "a lighthouse at dusk" --output image --steps 4
respond nvidia "a lighthouse at dusk" --model stabilityai/sdxl-turbo

upload openai ./report.pdf                                 # Files API upload; prints the file id
```

`models hosted <provider>` lists what the provider serves, with each model's kind and API — filter with `--tag out:image` — and caches the list so those models can be used without configuring them. Where a provider describes its models (Anthropic: image/PDF input, thinking, token limits), that becomes tags too, and `respond` checks requests against it before sending (`--no-validate` skips that).

Each provider can offer several APIs, and a model's **tags** decide which one serves it (`models tags openai gpt-image-2.5-flare` shows why). The shipped OpenAI template serves chat models through the Responses API and `gpt-image-*` models through the Images API, so the image-only flags (`--size`, `--quality`, `--n`, …) appear when the request goes there. Images are saved as `<prompt-words>-<n>.png` and never overwrite an existing file. Tag models a provider can't describe with `models tag <provider> <model> +kind:image-generation` (see `tags` for the vocabulary). `upload` works with providers whose API definition has a `files` block (`responses`, `messages`).

> Upgrading from an older config: entries without `template:` keep working as one API per provider. `providers upgrade` shows how each would move to its shipped template (keeping only the values you changed); `providers upgrade --yes` applies it, backing up the file first.

A provider's APIs are listed by `providers api list <provider>`; add one with `providers api add <provider> <api> --for <tag> --default-model <model>` (e.g. embeddings on a local OpenAI-compatible server), and `providers api rm` removes one (a template's is turned off).

## Managing Providers

### List Providers

```bash
providers list               # Formatted output
providers list --table       # Table format
providers list --raw         # Raw YAML
```

### Add a New Provider

```bash
providers add my-provider --url https://api.example.com/v1 --default_model my-model --api chat-completions
```

`--api` names the API definition the provider speaks (default: `chat-completions`); see the [API Definitions guide](api-definitions.md). If you pass `--api_key`, the key is written to the provider's key file, not to the config.

## Managing Models

### View Available Models

```bash
models list openai                  # Configured models
models hosted openai --table        # All models from the provider's API
```

### Set a Default Model

Each provider has a default model for each kind of model it serves (chat,
image-generation, embedding, …). A request that doesn't name a model uses the
default for the kind of output it asks for: `respond` the chat default,
`generate` the image default, `embed` the embedding default.

```bash
models default openai                              # the default for each kind
models default anthropic claude-sonnet-4-5-20250929   # the chat default
models default openai gpt-image-2                  # the image default (a model is the default for its own kind)
models default openai --clear --kind image-generation   # back to the provider's default
```

Defaults you set are kept in `providers/<provider>/default-model.yaml`; the
rest come from the provider's template.

### Use Model Aliases

Aliases let you use short names instead of full model IDs:

```bash
models alias set openai gpt4 gpt-4o
respond openai --model gpt4
```

## Command Aliases

Create shortcuts for commands you use often:

```bash
alias add ra respond anthropic
alias add ro respond openai

# Now you can just type:
ra myq
ro --model gpt-4o
```

Aliases expand both in the shell and in one-off commands (`lexi ra myq`), and never shadow a real command. View and manage aliases:

```bash
alias list          # Show all aliases
alias rm ra         # Remove an alias
alias reset         # Reset to the shipped defaults
```

## Configuration

View and modify CLI settings:

```bash
config list                     # Show all settings
config set edit_mode emacs      # Switch to emacs keybindings
config set prompt_delimiter $   # Change the prompt delimiter
config set request_timeout 120  # Give up on a provider after 2 minutes (0 = no limit)
```

Default settings:

| Key | Default | Description |
|---|---|---|
| `cli_name` | `lexi` | CLI name |
| `prompt` | `cmd` | Prompt label |
| `prompt_delimiter` | `>` | Character after the prompt label |
| `edit_mode` | `vi` | Input editing mode (`vi` or `emacs`) |
| `request_timeout` | `300` | How long to wait for a provider, in seconds (0 = no limit). A request that isn't streamed must finish within it; a streamed one may run as long as it keeps sending, and stops after this long with nothing arriving. Used by the CLI, the Python library and the desktop app (Configure → Requests). For one request, `--timeout SECS` on `respond` / `generate` / `embed` (Python: `timeout=`) sets its own limit instead. |

## Shell Integration

Run shell commands without leaving Lexi:

```bash
sh ls -la                    # Run a shell command
!echo "hello"                # Shortcut syntax
```

Re-run previous commands from history (stored in `~/.lexi-cli/history`):

```bash
history                      # Show command history
history -n 5                 # Show last 5 commands
#3                           # Re-run command #3
#-1                          # Re-run the last command
history --clear              # Clear the history
```

## Scripting

Every command accepts the global `--json` flag for machine-readable output, and exits `0` on success, `1` on failure, and `2` on a usage error. See the [machine interface spec](spec/cli-machine-v1.md).

```bash
lexi --json providers list
```

## Next Steps

- [CLI Reference](cli-reference.md) — Full command reference with all options
- [API Definitions](api-definitions.md) — Learn how to add new API styles or customize existing ones
- [Architecture](architecture.md) — Understand how Lexi works under the hood
