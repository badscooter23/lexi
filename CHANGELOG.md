# Release notes

Every release has a section here, newest first. The desktop app shows the
sections a user hasn't seen yet on its startup screen, so write for people who
use Lexi rather than for its developers. Changes merged since the last release
go under **Unreleased**; at release time that heading becomes the version
(`## 0.3.0 — YYYY-MM-DD`) and a new empty Unreleased section starts.

## Unreleased

### API keys
- **On macOS, saved keys go in your Keychain.** A key you save (with the Setup Wizard or `lexi providers api-key set`) is kept in the macOS Keychain, encrypted with your login, instead of a plain file. There's no password prompt: Lexi saves it through Apple's own `security` tool. Keys you saved before are moved into the Keychain the first time the new version runs (it tells you once). If the Keychain is locked, as it is over SSH, Lexi says how to unlock it. To keep saved keys in files instead, run `lexi config set key_store file`.
- **On Windows and Linux, saved keys go in the system's key store too:** Windows Credential Manager, or your desktop keyring on Linux (GNOME Keyring or KWallet), encrypted with your login. Keys saved before are moved in the same way. A Linux machine without a desktop keyring (a server, a container) keeps using the file.
- **Going back to 0.3.0** after this release won't find keys that were moved into the key store. Run `lexi providers api-key show <provider> --reveal` first, or save the key again in the older version.
- **Keys from a password manager.** A provider's `api_key` can list a command that prints the key, such as 1Password's `op read op://Private/OpenAI/credential`, `sops -d` or `age -d`. It runs only when a request needs the key (listing providers or opening the app doesn't set off a Touch ID prompt), and once per command or Python session.
- **Change where a key is looked for from the command line.** `lexi providers api-key source list`, `add` and `rm` (Python: `lexi.providers.key_sources`, `add_key_source`, `remove_key_source`) list the places in order, add an environment variable, a file, a command or the saved key's place, and take them out.
- **Saving a key no longer edits providers-config.yaml.** The key Lexi saves is always looked in after the places you list (put `saved` in the list to look in it earlier), and saving never writes to a file you listed. Your config is tidied once: Lexi's own key file comes out of the list, and the same key keeps being used.
- **Show Key / Hide Key.** In the desktop app's key panel, the button now says what a click does, and a label under the key says whether it's redacted or shown in clear text.

- **Several keys per provider, each with a name.** One per project or per account, each with its own usage and billing. Save one with `lexi providers api-key set openai#work` (a new name adds the key), and use it with `lexi respond openai#work "…"` or `--key work` (Python: `key="work"`).
  - `lexi providers keys list|add|rename|rm|default` manage them. The default key (★) is the one a request that names no key uses: the only key, or the one you choose. Any key can be renamed or removed, the default and the only one included.
  - Background jobs and uploaded files belong to the account that made them, so check or attach them with the same key.
- **The provider's environment variable wins when it's set.** `$OPENAI_API_KEY` (and `$ANTHROPIC_API_KEY`, `$NVIDIA_API_KEY`) isn't one of the provider's keys any more: it's a key for this shell or script, with nothing stored. When it's set, a request that names no key uses it ahead of the default key. Before, a saved key came first. A request that names a key still uses that key.
- **Commands that change a key must name it.** `lexi providers api-key set`, `remove` and `source add|rm` need the key, e.g. `openai#work`, and say which keys there are when you leave it out. `lexi providers api-key set openai` from earlier versions now asks you to name the key. `api-key show`, `source list` and `providers status` cover every key when you name none, and `providers status` says what a request that names no key would use.
- **Keys from before names keep working.** A key you saved with 0.3.0 or earlier has no name: it stays where it is, is the default while it's the only key, and you write it with nothing after the `#` (`openai#`). Rename it whenever you like: `lexi providers keys rename openai# personal`.
- **No more separate keys for one API or model.** A key covers every API of the provider (chat, images, embeddings). An API's or a model's own key, if you had one, becomes one of the provider's named keys (`#image-generations`), used when a request names it. Lexi says so once. `--api` is gone from the key commands, and `providers add --api-key` now takes `--key-name`.
- Lexi 0.3.0 ignores named keys and reads only a key saved before names existed.

### Desktop app
- **The message area keeps up.** Switching tabs clears it (the recent-messages list, on click, keeps everything), reloads Lexi does on its own no longer cover what you just did with "Loaded 12 saved prompts", and each new message flashes as it arrives, so a repeated one is seen to be new. Clearing the Images results now says so too.
- **Hide and reorder the tabs.** Right-click a tab to hide it or move it, or drag it to a new place; **+** at the end of the tabs shows a hidden one again (its hover help lists them). Lexi remembers your layout. The APIs tab (API definitions, for adding a provider Lexi doesn't ship) is hidden until you show it. Each tab's hover help says what it's for.
- **One tab per key.** On the Providers tab, API Key Management has a tab for each of a provider's keys, labelled the way you write them after the provider: `#work`, and `#` for a key saved before keys had names. ★ marks the default. **+ Add Key** adds a key with a name (and saves it, if you paste it); every key can be renamed, deleted, or made the default. Above the tabs, the panel says whether the provider's variable (`$OPENAI_API_KEY`) is set, which wins for a request that names no key.
- **Edit where a key is looked for.** Each key lists the places Lexi looks for it, in order. **Add Source…** adds an environment variable, a file or a command (a password manager, say), first or last, and **Remove** takes one out of the list. Nothing is deleted from it.
- **Two keys side by side.** In Chat and Images, a provider gets a column for each key that has one (`openai#work`, `openai#`), plus `openai ($OPENAI_API_KEY)` when the variable is set; with just one, the column is `openai`. A provider with no key isn't offered. The **>_** help writes the same `openai#work`.
- **The Setup Wizard names the key it saves,** filled in with the provider's default key (or empty for a new provider). Its "use the environment variable" choice explains the variable and saves nothing.
- **Opened from Finder?** Apps opened from Finder or the Dock don't see variables set in `~/.zshrc`. When the provider's variable isn't set and Lexi was opened from Finder, the key panel and the Setup Wizard say so and suggest saving a key instead.

### Models
- **See how a provider's model list is fetched.** `lexi models hosted <provider> --schema` (Python: `lexi.models.hosted_schema`) shows the address Lexi asks and the catalog settings it uses: paging, the fields shown, and the rules that turn the listing into tags and limits. It also says whether they come from the provider's template or your providers-config.yaml. Nothing is fetched.

## 0.3.0 — 2026-11-25

Lexi 0.3.0 brings an Images tab and a CLI tab to the desktop app, shows the command behind every button, keeps your API keys in files only you can read, and gives each provider a default model for each kind.

### Desktop app
- **Startup screen.** When Lexi opens, it checks which providers have API keys. It shows a welcome screen the first time you run a new version (with these notes), or when no provider has a key yet.
- **Setup wizard.** For each provider, the wizard explains how to get a key and saves it for you. It can also test the key.
- **Images tab.** Generate images from several providers at once. Each provider has its own model and settings, and images are saved to a folder you choose.
- **CLI tab.** Run `lexi` commands right in the app, just as in a terminal: output appears as it's written, ↑ / ↓ go through your command history (shared with the terminal), and Ctrl-C or Stop cancels a long command. Paste commands from a tooltip's **>_** panel (several at once run in order), or click its **▶ CLI** to put one in the prompt, ready to run. A command that needs your API key asks for it in a hidden field; the key never appears on screen or in the history.
- **See the command for any action.** Hover over a button and click **>_** in its help (or hold ⌥ while hovering) to see the `lexi` command and the Python that do the same thing, filled in with what's on screen, with a Copy button for each. Chat and Images show one command per ticked provider. This replaces the old "Show CLI Commands" panel.
- **Stop really stops.** In Chat, Stop cancels the requests still waiting (a provider that never answers no longer leaves Chat stuck), and each provider's answer shows as soon as it arrives instead of waiting for the slowest.
- **Chat's model lists stay current.** Models you add or remove on the Models tab show up in Chat's pickers when you go back to it.
- **Chat and Images list only the right models.** Chat lists only chat models, and Images lists only image models. Providers with no suitable model are hidden.
- **Prompt editing.** Chat and Images share one way of handling prompts:
  - *Load* lights up when you pick a saved prompt.
  - *Save* lights up when you change the text.
  - If you switch prompts with unsaved changes, Lexi asks whether to save them.
- **Messages.** Every action reports what it did in the message area at the top right. Click it to see recent messages.
- **Themes.** The new themes are System (it follows your macOS accent colour and Light/Dark setting), NVIDIA Dark and NVIDIA Light. Your choice is remembered, and every theme's colours can be edited in `~/.lexi-cli/themes.json`.
- **Models tab.** Tables can be sorted and filtered, edits open in dialogs, and every control has hover help.
- **New Lexi logo** for the app icon and the header.


### API keys
- **You choose where each key comes from.** A provider's `api_key` in providers-config.yaml is now a list of places tried in order: an environment variable or a file at any path. Only a missing key moves on to the next place; a file that can't be read stops with an error instead of quietly using a different key. The API Key panel shows each place and which one is in use.
- **Saved keys stay in a file only you can read.** A key you save goes in the provider's key file in `~/.lexi-cli`, never in providers-config.yaml, and the config lists where it is. A key file saved by an earlier version keeps working. The CLI and Python library read the same places.
- **A key per API or model.** An API (say, OpenAI's image generation) or a model can have its own `api_key`, for example to bill images to a different project.

### Models and requests (CLI, Python and the app)
- **A default model for each kind.** Each provider can have its own default for chat, for images and for embeddings. On the Models tab, ☆ makes a model the default for its kind (an image model becomes the default image model; chat keeps its own), and ★ on a default you chose clears it, so the provider's default applies again. Chat's and Images' model pickers show which default they'll use. From the command line: `lexi models default openai gpt-image-2` sets the image default; `lexi models default openai` lists them all; `--clear --kind image-generation` clears one. In Python: `lexi.models.default`, `set_default` and `clear_default`.
- **A time limit for slow providers.** Lexi gives up on a provider that hasn't answered within 5 minutes and says so, instead of waiting forever. When streaming, the limit is on silence, so a long answer that keeps coming is never cut off. Change it on the Configure tab (Requests), or with `lexi config set request_timeout <seconds>` (0 = no limit).
- **A time limit for one request.** `lexi respond`, `generate` and `embed` take `--timeout SECS` (Python: `timeout=`), which sets that request's own time limit instead of the saved `request_timeout`, for example more time for a slow reasoning model or a quick check that a provider answers at all. 0 means no limit.

### Fixes
- **Chat shows formatted answers.** Headings, lists, tables, code blocks and links now display formatted instead of as plain Markdown text (the formatting libraries had been left out of the app). Links in answers open in your browser.
- The **Show CLI Commands** panel at the bottom of the app is gone. It had shown nothing since the app stopped running the CLI behind the scenes.
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
