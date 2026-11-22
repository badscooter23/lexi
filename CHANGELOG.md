# Release notes

Every release has a section here, newest first. The desktop app shows the
sections a user hasn't seen yet on its startup screen, so write for people who
use Lexi rather than for its developers. Changes merged since the last release
go under **Unreleased**; at release time that heading becomes the version
(`## 0.3.0 — YYYY-MM-DD`) and a new empty Unreleased section starts.

## Unreleased

### Desktop app
- **Startup screen.** When Lexi opens, it checks which providers have API keys. It shows a welcome screen the first time you run a new version (with these notes), or when no provider has a key yet.
- **Setup wizard.** For each provider, the wizard explains how to get a key and saves it for you. It can also test the key.
- **You choose where each key comes from.** A provider's `api_key` in providers-config.yaml is now a list of places tried in order: an environment variable, the macOS Keychain (Windows Credential Manager on Windows), or a file at any path. Only a missing key moves on to the next place; a locked Keychain stops with an error instead of quietly using a different key. The API Key panel shows each place and which one is in use.
- **Keys in the Keychain.** Keys you save go in the Keychain, encrypted, instead of a plain-text file. On macOS they're all in one item, **Lexi API keys**, so macOS asks for access once — not once per provider — and only when a key is first used. A key file saved by an earlier version keeps working (the config now lists it), and **Move to Keychain** moves it there. The CLI and Python library read the same places.
- **A key per API or model.** An API (say, OpenAI's image generation) or a model can have its own `api_key`, for example to bill images to a different project.
- **Images tab.** Generate images from several providers at once. Each provider has its own model and settings, and images are saved to a folder you choose.
- **Chat and Images list only the right models.** Chat lists only chat models, and Images lists only image models. Providers with no suitable model are hidden.
- **Prompt editing.** Chat and Images share one way of handling prompts:
  - *Load* lights up when you pick a saved prompt.
  - *Save* lights up when you change the text.
  - If you switch prompts with unsaved changes, Lexi asks whether to save them.
- **Messages.** Every action reports what it did in the message area at the top right. Click it to see recent messages.
- **Themes.** The new themes are System (it follows your macOS accent colour and Light/Dark setting), NVIDIA Dark and NVIDIA Light. Your choice is remembered, and every theme's colours can be edited in `~/.lexi-cli/themes.json`.
- **Models tab.** Tables can be sorted and filtered, edits open in dialogs, and every control has hover help.
- **New Lexi logo** for the app icon and the header.

### Fixes
- The Providers tab's API key panel can save, show and remove keys again.
- The Providers tab's **Show** button opens `providers-config.yaml` again.
- A provider whose key comes from an unset environment variable no longer shows as having a key.

## 0.2.0 — 2026-11-16

Lexi 0.2.0 has three headline changes: one Rust CLI, a full Python library, and providers that offer several kinds of model.

### One CLI, in Rust
- `lexi` is now a single native binary with the full command set: prompts, providers, models, aliases, API definitions, config, the interactive shell and history. The Python CLI is gone.
- Every command has `--json`, a stable machine interface.
- Background jobs can be detached and checked later: start one with `respond --background --detach`, then use `jobs status` or `jobs wait`.
- First run fills in missing defaults and never overwrites your files.

### One `respond` for text, images, or both
- `respond` works for text, for images (`--output image`) and for both together (an image tool).
- Images are saved as `<prompt-words>-<n>.png` and never overwrite a file.
- `generate` is a shortcut for `respond --output image`.
- The new `embed` command creates embeddings (OpenAI, NVIDIA).

### Providers with several APIs, and model tags
- A provider can offer several APIs. OpenAI ships with chat, images and embeddings.
- **Tags** describe what each model accepts, produces and supports, and decide which API serves it.
- `lexi models hosted` lists and filters what a provider serves. Listed models work without configuring them.
- Requests are checked before they're sent: image input, token limits, and a warning for deprecated models.
- Provider templates ship with Lexi, so updates reach existing installs. `lexi providers upgrade` moves an older config onto them.

### Python library
- `import lexi` now covers sending, streaming, background jobs, images, embeddings, uploads and config management.
- It shares `~/.lexi-cli` with the CLI.

### Desktop
- The Models tab shows each model's kind, API and tags.

## 0.1.0 — 2026-11-15

- First release: the Python wheel and the desktop app installers.
