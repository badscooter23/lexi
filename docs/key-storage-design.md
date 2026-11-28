# API Key Storage — Design

**Status:** accepted · 2026-11-25 · steps 1 (source model), 2 (macOS store) and 3 (Windows and Linux stores) implemented · §11 named keys implemented, revised by §12 (proposed)
**Scope:** where Lexi reads API keys from and where it saves them: the CLI, the
Python library and the desktop app (all via `lexi-host`).
**Replaces:** the 0.3.0 rule that a saved key goes into "the first file
`api_key` lists".

## 1. Problem

In 0.3.0 a provider's `api_key` is an ordered list of sources, `{env: VAR}` or
`{file: PATH}`. A key Lexi saves goes into the first file in that list, or into
`providers/<p>/api-key`, which is then appended to the list. This causes four
problems:

1. **Saved keys are plain text.** A 0600 file stops other accounts on the Mac.
   It doesn't stop root, backups, a synced or committed `~/.lexi-cli`, or a
   disk read while you're logged out without FileVault.
2. **Lexi's own file shows up in the config.** Saving a key edits
   providers-config.yaml, and the list mixes files Lexi owns with files you own.
3. **Saving can overwrite your file.** If you list a file you manage yourself
   (`{file: ~/keys/anthropic}`), `api-key set` writes into it.
4. **Environment variables don't reach apps opened from Finder.** On macOS,
   an app started from Finder, the Dock or `open` is launched by `launchd`, not
   your shell, so variables set in `~/.zshrc` aren't there. Python programs
   started the same way (IDEs, Jupyter, launchd jobs) have the same problem.

The OS keychain was tried in development and removed (PR #65). The cause was
that Lexi called the Security framework directly. An unsigned or ad-hoc-signed
build gets a new code identity on every rebuild, so macOS asked "Always Allow"
again after each update, and on the developer's Mac it wouldn't accept the
password.

## 2. Goals

- A saved key is **not stored in clear text** wherever the OS offers a store:
  macOS, Windows and Linux desktops.
- **No prompts, dialogs or admin rights,** at install time or at use time.
- **One copy of a saved key** in one place, chosen automatically.
- The same behaviour in the CLI, Python and the app.
- **Machines with no store keep working.** That covers headless Linux, CI,
  containers and SSH sessions.
- Room for **real protection** (Touch ID, a password manager) when the user
  opts in.

**Non-goals:**
- Protecting keys from programs already running as the user, without a
  prompt. No design can do that: anything Lexi can read silently, they can too.
- Encryption Lexi designs itself.

## 3. Threat model

| Threat | 0.3.0 (0600 file) | OS store (this design) | `command:` + a password manager |
|---|---|---|---|
| Another account on the machine | Protected | Protected | Protected |
| An admin or root on the machine | **Exposed** | Protected (needs your login password) | Protected |
| Backups, sync, a dotfiles repo, a copied `~/.lexi-cli` | **Exposed** | Protected | Protected |
| Disk read while logged out, no FileVault | **Exposed** | Protected | Protected |
| A program running as you (malicious package, extension) | Exposed | **Exposed** | Protected (Touch ID or unlock prompt) |
| Seen on screen / in a `grep` | Exposed | Protected | Protected |

The last row is why the OS store must be described as **"encrypted with your
login"**, never "safe from other programs".

## 4. Design

### 4.1 Sources

`api_key` lists the places to look, in order. There are three kinds; Lexi never
writes to any of them:

| Source | Meaning |
|---|---|
| `{env: VAR}` (or `"${VAR}"`) | An environment variable. |
| `{file: PATH}` | A file **you** manage, e.g. a Docker or Kubernetes secret (`/run/secrets/anthropic`), or a key on an encrypted volume. **Read-only:** Lexi never writes or deletes it. Relative paths are under `$LEXI_HOME`; `~/` is expanded. |
| `{command: [PROGRAM, ARG…]}` | Run a program and use what it prints (§4.5). This covers password managers and encrypted files: 1Password `op read`, `sops -d`, `age -d`, `pass`, Vault and so on. |

Two more cases are handled:
- **A key written into providers-config.yaml itself** (a literal string) is
  still read, with a warning. `providers upgrade` moves it into the saved store.
- **`{keychain: NAME}`** (0.3.0 development builds) stays an error saying what
  to use instead.

**The saved key** is not a source you list. It is wherever Lexi keeps keys
(§4.3), and it is **always checked after the listed sources**. To check it
earlier, put the marker `saved` in the list where it should go:

```yaml
anthropic:
  template: anthropic                  # api_key: [{env: ANTHROPIC_API_KEY}] from the template
openai:
  template: openai
  api_key: [saved, {env: OPENAI_API_KEY}]   # saved key wins over the variable
work:
  template: anthropic
  api_key:
    - command: [op, read, "op://Work/Anthropic/credential"]
```

### 4.2 Lookup

The listed sources are tried in order, then the saved store unless `saved`
placed it earlier. The first key found is used. As in 0.3.0:
- **A missing key moves on.** That means an unset variable, an absent file, an
  item not in the store, or empty command output.
- **A read error stops the lookup.** That covers an unreadable file, a failing
  command or a locked store. An error never silently switches you to a
  different key.

### 4.3 The saved store

The store is chosen per machine. Each scope (a provider, an API's own key, a
configured model's own key) has at most one saved key, and it lives in exactly
one of these places:

| Platform | Backend | Notes |
|---|---|---|
| macOS | Login keychain, accessed through `/usr/bin/security` | See below. |
| Windows | Credential Manager (generic credentials) | Encrypted with the user's login; no prompts. |
| Linux with a desktop session | Secret Service over D-Bus (GNOME Keyring, KWallet) | Pure-Rust D-Bus client, so no system library is needed. |
| Anything else (no store reachable, CI, containers, `key_store: file`) | `providers/<p>/api-key[-<api>]`, 0600 | Today's file, now implicit. |

**Why macOS goes through `security`.** Lexi runs Apple's signed `security`
tool instead of calling the Security framework (the same approach as the `gh`
CLI). The keychain item then belongs to an Apple tool whose identity never
changes, so there's no dialog, however often Lexi is rebuilt, and whether the
caller is the app, the CLI or a Python process.
- **Verified 2026-11-25 on macOS 15:** add, read and update all ran silently
  with a dummy value.
- **The key never appears in a process list:** it is passed on stdin in
  `security -i` mode, as hex (`-X`), so quoting can't break it.

**Item naming.**
- Service `Lexi`, account `<provider>` or `<provider>/<api>`, label
  `Lexi: <provider> API key`.
- When `LEXI_HOME` isn't the default `~/.lexi-cli`, the account gets
  `@<canonical LEXI_HOME>` appended, so separate configs (tests, profiles)
  never share keys.
- In the `security -i` line, arguments are double-quoted with `\` and `"`
  escaped (the tool's own parser); `security` exits with the low byte of the
  OSStatus (44 not found, 36 interaction not allowed = locked).
- Windows targets and Secret Service attributes carry the same fields: the
  credential's target is `Lexi:<account>` (its comment the label); the
  Secret Service item has `service: Lexi`, `username: <account>` and the
  label. Both go through the `keyring` 3 crate (Linux: its pure-Rust zbus
  client), each call on its own thread with a 60-second limit, so an unlock
  dialog nobody answers can't hang Lexi.
- **Reachable** means looking up an item that isn't there finds nothing
  rather than failing (checked once per process). WSL has a Secret Service
  but often no default collection: lookups work and saving fails, with the
  error; `key_store: file` avoids it.

**Choosing a backend.** Set `key_store` in `config.json` to `auto` (the
default), `system` or `file`. `LEXI_KEY_STORE` overrides it (tests, CI).
- **`auto`** uses the OS store when it's reachable, else the file.
- **`system`** fails rather than fall back.
- **`file`** always uses the file.

**A locked store** (macOS over SSH is the common case) is a read error, which
only happens if no earlier source had a key, and if no key file is left from
before (one is read until it can be moved in). The message says how to fix it:
`security unlock-keychain`, set the variable, or `key_store: file`. Saving
into a locked store fails the same way; it never falls back to a file, so a
key is never split across two places.

### 4.4 Saving and removing

- **`set`** writes the saved key into the store, then deletes any key file
  Lexi left for that scope, so there's one copy. It **never** edits
  providers-config.yaml and never writes to a `file:` source. If a listed source
  already has a key that will win, it says so (as in 0.3.0).
- **`remove`** deletes the saved key, from the store and any leftover Lexi key
  file. It never touches listed sources.
- **Rename and remove provider** move or delete its saved keys, as they do with
  key files today.

### 4.5 The `command:` source

- **How it runs.** An argv list, run directly with no shell. `~/` is expanded
  in arguments, and the working directory is `$LEXI_HOME`.
- **Output.** stdout is trimmed; empty output is a missing key. A non-zero
  exit or a timeout is a read error that shows the program's stderr (never its
  stdout).
- **Timeout:** 60 seconds, separate from `request_timeout`, to leave time for
  Touch ID or an unlock prompt.
- **Caching.** The key is kept in memory for the life of the process, per
  scope, so a Touch ID prompt comes once per CLI command or Python process, not
  once per request. The desktop app keeps it until the app quits or the
  provider's key settings change; *Check key* runs the command again.
- **Startup.** The desktop startup screen doesn't run commands. A `command:`
  source counts as "has a key (not checked)", so opening the app never fires a
  Touch ID prompt.
- **Security.** Running a command from the config means anyone who can edit
  providers-config.yaml can run programs as you, just as with `~/.zshrc`. Lexi
  refuses `command:` sources when providers-config.yaml is writable by group or
  others.

### 4.6 Scopes

An API binding's or a configured model's own `api_key` replaces the provider's
(unchanged). The saved store follows the same rules per scope: `set --api A`
saves `<provider>/<A>` (and gives the API its own `api_key: [saved]` if it used
the provider's). A configured model's own `api_key` has **no** saved key —
there's no command to save one; it is only what it lists.

## 5. Surfaces

All of this lives in `lexi-host` (`keystore.rs`, `config.rs`). Every new
operation is listed in `docs/surface-matrix.md`.

| Operation | lexi-host | CLI | Python |
|---|---|---|---|
| Save / show / remove the saved key | `config::set_api_key`, `admin::api_key_info`, `config::remove_api_key` (same signatures) | `providers api-key set\|show\|remove` (unchanged) | `providers.set_key`, `show_key`, `remove_key` (unchanged) |
| List / add / remove listed sources | `config::add_key_source`, `remove_key_source` | `providers api-key source list\|add\|rm NAME [--api A] (--env VAR \| --file PATH \| --command '…' \| --saved) [--first]` | `providers.key_sources`, `add_key_source`, `remove_key_source` |
| Choose the backend | `key_store` in config | `config set key_store auto\|system\|file` | `config.set("key_store", …)` |

**Result shape changes** in `api_key_info` and `providers api-key show --json`:
- `save_to` names the backend, e.g. `macOS Keychain` or
  `providers/anthropic/api-key`.
- `adds_source` is removed.
- A new `store` field: `{kind, available, locked}`.
- Each entry in `sources` gains `kind: "saved"` for the store.
- `docs/spec/cli-machine-v1.md` records the change.

**Desktop app**
- The API Key panel shows each source and where the saved key lives
  ("Saved in macOS Keychain, encrypted with your login").
- It lets you add and remove listed sources.
- When a variable is missing and the app was opened from Finder (parent
  process is `launchd`, and no `TERM`), it explains that apps opened from Finder
  can't see variables set in `~/.zshrc` and offers to save the key instead.
- The Setup Wizard saves to the store.

## 6. Migration

`ensure_home` runs on every start and changes existing configs once:

1. **List entries.** An `api_key` entry `{file: …}` that points at Lexi's own
   key file (`providers/<p>/api-key[-<api>]`) is removed from the list, since
   the saved store now covers it. Its position is kept with `saved` if it
   wasn't last.
2. **The file itself.** With `key_store: auto` or `system` and a reachable
   store, the key is copied into the store, read back to check it, and then the
   file is deleted. The app and CLI report it once: "Moved anthropic's API key
   into the macOS Keychain." If the store is unavailable or locked, the file
   stays and is read as before.
3. **Files you listed yourself**, outside Lexi's key-file names, are left
   alone and become read-only sources.

**Downgrading** to 0.3.0 after the move leaves keys in the store, where 0.3.0
can't see them. The release notes say so; `providers api-key show --reveal`
then `set` on the old version restores them.

## 7. Testing

- Every existing test runs with `LEXI_KEY_STORE=file`, so no test touches
  the developer's real keychain or credential store. The harness sets it, and
  a guard test fails if it's missing.
- **macOS backend tests** use a throw-away keychain (`security
  create-keychain` in a temp dir), passed to each `security` call by path
  (`LEXI_KEYCHAIN` for the CLI). Creating a keychain adds it to the search
  list, so the tests put the list back straight away, and they run only when
  `LEXI_TEST_KEY_STORE` is set (CI, on every platform), never on a
  developer's machine by default.
- **Windows backend tests** use accounts made unique by the test's temp
  home, and delete them.
- **Linux backend tests** run against a private `gnome-keyring-daemon` on a
  session bus in CI (`dbus-run-session`, the login keyring unlocked with a
  test password).
- **`command:`** is covered for success, empty output, non-zero exit (stderr
  shown), timeout, caching, and refusal when the config is writable by others.
- **Migration** has a case for each step in §6, including a locked or
  unavailable store.

## 8. Rollout

1. **Source model.**
   - What changes: the implicit saved store (file backend only), read-only
     `file:`, `command:`, the `saved` marker, the `source` commands and Python
     functions, and migration step 1.
   - For users: no change in where keys live.
2. **macOS store.** The `security` backend, `key_store` / `LEXI_KEY_STORE`,
   migration step 2, and the locked-keychain message.
3. **Windows and Linux stores.**
4. **Desktop.** The API Key panel and wizard changes, and the Finder-launch
   hint. Named keys (§11) come first, so this screen is built once with
   them in it.

Each step is its own PR with CHANGELOG notes.

## 9. Open questions

1. **Migration step 2:** move keys into the store automatically (proposed),
   or ask once in the app and CLI?
2. **`saved` marker:** keep it, or always check the saved store last (simpler;
   users who want it first remove the variable)?
3. **Linux without a running Secret Service** but with a desktop: fall back to
   the file silently (proposed), or say once that keys are in a plain file?
4. **Signed builds later:** once the app is signed with a Developer ID, an
   opt-in "require Touch ID" for the saved key becomes possible on macOS (a
   Secure Enclave or access-controlled item). This is out of scope here.

## 10. Rejected alternatives

- **Encrypting the key file with a key built into Lexi.** It only hides the
  key from people. The decryption key would be in a public repo and every
  binary. It would also stop GitHub secret scanning from spotting (and
  providers from revoking) a key committed by mistake, while making the file
  sound safe.
- **A key file encrypted with a passphrase.** Real encryption, but it means a
  prompt per session or a background agent. `command:` with `age` or `sops`
  gives the same for users who want it.
- **Calling the Security framework directly (0.3.0-dev, #64).** It re-prompts
  after every unsigned rebuild, and "Always Allow" rejected the password.
- **Reading the login shell's environment at app start (`$SHELL -ilc env`).**
  It slows every launch, runs the user's shell startup files inside the app,
  and breaks when they print or expect a terminal.
- **Keeping both a file and a store copy.** Two copies drift, and the file
  undoes the protection.

## 11. Addition: named keys (several accounts per provider)

**Status:** accepted · 2026-11-28 (decisions in §11.10) · implemented (host, CLI, Python; desktop with step 4) · **revised by §12**: every key has a name

### 11.1 Problem

A provider has exactly one key in use at a time. People with **separate
accounts** at the same provider — work and personal, one per client, a team
key and their own — want to choose which account a request is billed to.
Today they can only add a second provider entry from the same template
(`openai-work: {template: openai}`). That works, but it duplicates the
provider's models, defaults and aliases, and makes one service look like
two.

### 11.2 Goals

- Several **named keys** per provider, each with its own sources and its own
  saved key, chosen **per request** or by a per-provider **default**.
- Nothing changes for anyone with one key: today's `api_key` is the
  provider's unnamed key.
- The same in the CLI, Python and the desktop app.
- **Non-goal:** switching keys automatically (on a rate limit, or when credit
  runs out). That's a different feature: a named key is a choice of
  account, not a pool.

### 11.3 Config

```yaml
providers:
  openai:
    template: openai
    api_key: [{env: OPENAI_API_KEY}]   # the unnamed key, as today
    keys:
      work:                            # a named key: its own sources...
        api_key: [{command: [op, read, "op://Work/OpenAI/credential"]}]
      personal: {}                     # ...or none: just its saved key
    default_key: work                  # used when a request names no key
```

- **`keys`** maps a name to `{api_key?}`. `api_key` is an ordered list of
  sources, exactly as in §4.1. The key's saved key (§4.3) comes after them,
  unless the marker `saved` places it earlier. With no `api_key`, a named
  key is its saved key alone.
- **`default_key`** names the key used when none is asked for. When it's
  absent, the unnamed key is used.
- **Names:** letters, digits, `-` and `_`, up to 32 characters. `default`
  is reserved: in commands it means the unnamed key.
- A template never defines named keys. They're yours, in
  providers-config.yaml.

### 11.4 Which key a request uses

1. The key the request names: `--key work`, `key="work"`, the shorthand
   `openai#work` (§11.7), or the desktop app's picker. An unknown name is an
   error that lists the provider's keys.
2. Otherwise, as today: a configured model's own `api_key`, then an API
   binding's own `api_key`, then the provider's **default key**
   (`default_key`, else the unnamed one).

A named key covers **every API** of the provider. An account's key works for
chat, images and embeddings alike. An API binding's own `api_key` (§4.6) is
an override of the *unnamed* key only. When a request names a key, that key
is used for every API, and the binding's own key is skipped. Per-API keys
under a named key are left out (§11.10).

In the effective config, the chosen key goes in `_key_scope` as `{key:
NAME}`, next to today's `{api}` / `{model}`. Everything downstream that
reads `_key_scope` then finds the right saved key unchanged.

### 11.5 Saved keys

Each named key has its own slot in the saved store:

| Store | Unnamed key (today) | Named key `work` |
|---|---|---|
| Keychain / Credential Manager / Secret Service account | `openai[@home]` | `openai#work[@home]` |
| Key file (no store) | `providers/openai/api-key` | `providers/openai/keys/work` |

`#` can't appear in a provider name (new names are refused; §11.7), nor in
a key name. The `keys/`
folder can't clash with `api-key-<api>`. Rename and remove provider carry
named keys along, as they do the others. Removing a named key from the
config deletes its saved key.

### 11.6 Things that belong to an account

Some provider objects only exist in the account that made them, so the
key that made them must be used again:
- **Background jobs** (`respond --background --detach`): `jobs status` /
  `jobs wait` take `--key`. The command Lexi prints for checking on a job
  includes it.
- **Uploaded files** (`file:<id>` attachments): use the same `--key` as the
  upload. A mismatch is the provider's "not found" error; Lexi can't tell
  which account owns an id.
- **The catalog** (`models hosted`): accounts can see different models, such
  as fine-tunes. There's one cache per provider, not one per key (§11.10).
  `models hosted --key work` lists, and caches, what that account sees.

### 11.7 Surfaces

| Operation | lexi-host | CLI | Python |
|---|---|---|---|
| Use a named key for a request | `key` on `ChatRequest`, `EmbedRequest`, upload, hosted listing, job status | `--key NAME` on `respond`, `generate`, `embed`, `upload`, `models hosted`, `providers status`, `jobs status\|wait` | `key="NAME"` on `respond`, `stream`, `submit`, `prepare`, `generate`, `embed`, `upload`, `models.hosted`, `providers.status`, `jobs.*` |
| List a provider's keys | `key_names` (each `{name, default, source, from}`, never the key) | `providers api-key list NAME` | `providers.keys` |
| Add / rename / remove a named key | `add_named_key`, `rename_named_key`, `remove_named_key` | `providers api-key add\|rename\|rm NAME KEY` | `providers.add_key`, `rename_key`, `remove_key_name` |
| Choose the default key | `set_default_key` | `providers api-key default NAME [KEY]` | `providers.set_default_key` |
| Save / show / remove a named key's saved key, edit its sources | today's functions, gaining `key` | `providers api-key set\|show\|remove\|source … --name KEY` | today's functions, gaining `key=` |

- `providers api-key set openai --name work` creates the named key if it
  doesn't exist yet. That's the one-step path most people need.
- **As built,** the commands that manage named keys are their own group,
  `providers keys list|add|rename|rm|default`, rather than `providers
  api-key add|rename|rm|default`: `api-key remove` already deletes a saved
  key, and an `api-key rm` beside it would mean something else. The Python
  functions are `providers.keys`, `add_key`, `rename_key`, `delete_key` and
  `set_default_key`. On the key functions, the named key is `key_name=`
  (CLI `--name`), because `set_key`'s `key` is the key itself. Requests take
  `key=` (CLI `--key`). JSON results name it `key_name`.

**The `provider#key` shorthand.** Wherever a command or function takes the
provider a request goes to, `openai#work` means `openai` with `--key work`:
- In the CLI and Python, `lexi respond openai#work "…"` and
  `lexi.respond("openai#work", …)` are the same request as with `--key work`
  / `key="work"`.
- In `providers api-key …`, `openai#work` is the same as `--name work`, so
  `providers api-key set openai#work` creates the key.
- Naming the key both ways is fine when the two agree. When they disagree,
  it's an error.
- `openai#default` is the unnamed key.
- Commands that change the provider itself (`providers rename|rm|upgrade`,
  `providers api …`, `models …`) take the provider alone: a `#key` there is
  an error that says to leave it out.
- `#` is safe on a command line. Lexi's shell treats it specially only at the
  start of a line (`#N`), and shells start a comment only at the start of a
  word.
- `#` becomes a character provider names can't contain: `providers add` and
  `rename` refuse it. A provider already named with `#` (unlikely) keeps
  working but can't be given named keys until it's renamed.
- The desktop app shows a provider column with a named key as `openai#work`.
  The ">_" help writes the shorthand rather than `--key`.
- `api-key show` without `--name` shows the default key, and says which one
  that is.

**Desktop app** (with step 4):
- The API key panel shows one tab per key, plus **Add key**.
- In Chat and Images, a provider with more than one key gets a key picker
  in its column, so the same prompt can go to two accounts side by side.
- The Setup Wizard saves the unnamed key, as today.
- The ">_" command help includes `--key` whenever the picker isn't on the
  default.

### 11.8 Migration and compatibility

There's nothing to migrate: a config without `keys` behaves exactly as
today. A config with `keys` is ignored by an older Lexi, which keeps using
the unnamed key, so a request meant for `work` would be billed to the
unnamed account. The release notes say so. People using the second-provider
workaround can keep it. `providers api-key add` doesn't convert it.

### 11.9 Rollout

Named keys come **before step 4**, so the desktop key screen is built once
with them in it:
- **Step 3a, the host, CLI and Python:**
  - `keys` / `default_key`;
  - the selection rules;
  - the slots;
  - `--key` / `key=`;
  - the list / add / rename / rm / default commands;
  - jobs and uploads.
- **Step 4, desktop:** as planned, plus the key tabs and pickers.

### 11.10 Decisions

Decided 2026-11-28:
1. **The flag is `--key`** (Python `key=`), not `--account`: it matches
   `keys:` and `api-key`, and not every key is an account.
2. **No per-API keys under a named key.** A named key covers every API of
   the provider. An API's own `api_key` applies only to the unnamed key.
3. **One catalog cache per provider,** shared by its keys.
4. **The `provider#key` shorthand is in** (§11.7).

## 12. Revision: every key has a name

**Status:** proposed · 2026-11-28 · revises §11 (not yet released, so
nothing depends on the §11 behaviour). The rules in §12.2 were decided
2026-11-28; §12.9 lists what's still open.

### 12.1 Why

§11 treats the provider's original key as special: it has no name, and
commands call it `default`. That clashes with `default_key`, the ★ key that
requests use when they name none. Trying the desktop app showed it straight
away: the column labelled "default" wasn't the default key.

The use case is also broader than §11.1 says. Several keys per provider isn't
only for separate accounts: one account can have a key per **project**, each
with its own usage and billing reports. So a provider's keys are peers, and
they should all behave the same way.

### 12.2 The rules

1. **Every key has a name.** `default` is not a key name (`openai#default`
   goes), so the word "default" means only the ★ key.
   - **The one exception is a key saved before names existed** (0.3.0 and
     earlier). It stays unnamed, and you refer to it as **`openai#`**: the
     shorthand with an empty name. It works like any other key (it can be
     the default, changed, renamed or deleted) until you rename it, and then
     it's a named key like the rest.
   - Nothing can create a new unnamed key. Once a provider's unnamed key is
     renamed or deleted, `openai#` is an error for that provider.
2. **A provider needs a key to be used,** a stored key or its variable
   (rule 5), and the key has to be valid for a request to succeed. A provider
   with neither, or whose key finds nothing in its sources, is **not ready**
   until a key is provided.
3. **One key is the default (★).** A request that names no key uses it.
   - A provider with exactly one key always has it as the default.
   - With several keys, `default_key` names the default.
4. **Any key can be deleted by name, and renamed,** including the default
   and the only key.
   - Deleting the only key leaves the provider with no key (not ready).
   - Deleting the default when other keys remain leaves no default. A
     request that names no key is refused ("openai has several keys and no
     default: name one, like `openai#projectx`, or choose a default"), and
     the provider's status says so. With one key left, that key is the
     default (rule 3).
   - Renaming the default keeps it the default.
5. **The provider's environment variable is not a key.** `$OPENAI_API_KEY`
   (the variable the provider's template names) is a key handed to one
   process, by a script, a CI job or `OPENAI_API_KEY=… lexi …`, with nothing
   stored.
   - It isn't one of the provider's keys: it has no tab, isn't in `providers
     keys list`, and can't be renamed or deleted. Lexi doesn't own it.
   - **When it's set, it wins** for a request that names no key, over the ★
     default. That's how the OpenAI and Anthropic SDKs treat their variables,
     and it lets a script override your stored default without touching
     your setup.
   - **A request that names a key** (`openai#projectx`) uses that key, set
     variable or not.
   - Nothing needs a stored key for the variable to work. A provider with no
     keys and the variable set is ready.
   - A key's *own* sources may still list variables (`keys.personal.api_key:
     [{env: PERSONAL_OPENAI_KEY}]`). That's where that key's value is
     kept, and it's separate from this rule.

### 12.3 Config

Named keys live under `keys`. The provider's own `api_key` and saved slot
belong to the unnamed key (§12.2 rule 1), and only while it exists. The
template's `api_key` (`${OPENAI_API_KEY}`) names the provider's variable
(rule 5), not a key.

```yaml
providers:
  openai:
    template: openai
    api_key: [saved]                         # the unnamed key (openai#), if you had one
    keys:
      personal:                              # a named key: its own sources...
        api_key: [{command: [op, read, "op://Private/OpenAI/credential"]}]
      projectx: {}                           # ...or its saved key alone
    default_key: projectx
```

- **Each key** is `{api_key?}`: its sources in order (§4.1) and its saved key
  (§4.3, slot `openai#<name>[@home]`, or `providers/openai/keys/<name>` with
  no store), exactly as §11.3 and §11.5 describe named keys.
- **The unnamed key** keeps today's storage: its sources are the provider's
  `api_key` (without the provider's variable, which rule 5 covers), and its
  saved key is in the provider's slot
  (`openai[@home]`, `providers/openai/api-key`). Lexi 0.3.0 reads it, so a
  downgrade keeps working until it's renamed. **Renaming** it moves it into
  `keys` and the named slot (read back first, as in §6), like any key.
- **Deleting the unnamed key** deletes its saved key and the provider's own
  `api_key` list.
- **A fresh install, and a new provider, start with no keys.** Lexi creates
  no key and picks no name. The provider is ready if its variable is set,
  and otherwise not ready until you add a named key. Every key is one you
  named.
- **An API binding's or configured model's own `api_key`** (§4.6) still
  overrides the key, but only for a request that uses the **default** key.
  A request that names a key uses that key for every API. This keeps
  §11.10 #2: there are no per-API keys under a key.

### 12.4 Which key a request uses

1. The key the request names: `--key projectx`, `key=`, `openai#projectx`,
   or the desktop column. `openai#` names the unnamed key (rule 1). An
   unknown name is an error that lists the keys.
2. Otherwise, in order:
   - a configured model's own `api_key`, then an API binding's own (§12.3:
     set up because that model or API needs a different key, which the
     provider's variable wouldn't be either);
   - **the provider's variable, when it's set** (§12.2 rule 5);
   - the **default** key (§12.2 rule 3).
3. Nothing at all, or several keys with no default and no variable, is the
   "not ready" error, never a silent fallback to some other key.
4. Lexi says what it used: `-v` prints `key: $OPENAI_API_KEY` or `key:
   #projectx`. Status and the desktop app say the same (§12.5, §12.6).

### 12.5 Commands

A command that **changes** a key always names it. A command that only
**looks** at keys covers every key when it names none. Only a **request**
uses the default (§12.4). Nothing that changes a key is ever applied to
"whichever key is the default", so a write can't land on the wrong key.

| Command | With a name (`openai#NAME`, or `--name NAME`) | With no name |
|---|---|---|
| `api-key set` | Saves that key's value, and creates the key if it's new | **Refused** |
| `api-key remove` | Deletes that key's saved value (the key stays, not ready) | **Refused** |
| `api-key source add\|rm` | Edits that key's sources | **Refused** |
| `api-key show`, `api-key source list` | That key | **Every** key, ★ on the default, and the provider's variable (set or not) |
| `providers status` | Checks that key | Checks **every** key and the variable if it's set, and says what a request with no key would use: "requests that name no key use $OPENAI_API_KEY (from your environment)", or "… use #projectx ★", or why there's nothing |
| `providers keys add\|rename\|rm\|default` | Take the key name as an argument, as in §11.7 | — |
| Requests (`respond`, `generate`, `embed`, `upload`, `models hosted`, `jobs`) | That key | The **default** key, or the "not ready" error (§12.4) |

- **`openai#` is a name** for every row above: `api-key set openai#`,
  `providers keys rename openai# personal` and `providers keys rm openai#`
  work on the unnamed key. (`#` starts a shell comment only at the start of
  a word.) On a provider without an unnamed key it's an error, and `api-key
  set openai#` never creates one.
- **The refusal says what to do**, e.g. `name the key: lexi providers
  api-key set openai#NAME (openai's keys: # ★, projectx)`. With no key at
  all: `openai has no key: add one with lexi providers api-key set
  openai#NAME`.
- **Python** matches:
  - `set_key`, `remove_key`, `add_key_source` and `remove_key_source` need
    `key_name=` (or `"openai#NAME"`) and raise the same error without it;
  - `show_key`, `key_sources` and `providers.status` without one return
    every key;
  - requests take `key=`;
  - `key="default"` goes, and `providers.set_default_key(name, key)` needs a
    key.
- **JSON:**
  - `key_names` rows stay `{name, default, source, from}`, with `name` `""`
    for the unnamed key;
  - a new `variable: {name, set}` gives the provider's variable;
  - a provider result gains `ready`, `uses` (`{variable: NAME}` or `{key:
    NAME}`, what a request with no key would use) and `reason` (`no key`,
    `no default key`, `no key found`).

### 12.6 Desktop

- **Key tabs are labelled the way you'd write the key after the provider:**
  `#personal`, `#projectx`, and just **`#`** for the unnamed key. The `#`
  tab's hover help says "The unnamed key, saved before keys had names.
  Rename it to give it one." ★ marks the default. Every tab has
  **Rename…** and **Delete Key**, plus **Use by Default** on the others.
  **+ Add Key** asks for a name and, optionally, the key.
- **The provider's variable** shows as a line above the tabs, not as a tab:
  "$OPENAI_API_KEY is set: requests that name no key use it", or "…isn't
  set". The Finder hint (§5) belongs here: an app opened from Finder doesn't
  see `~/.zshrc`.
- **A provider with no key** (no stored key, variable not set) shows "Not
  ready: add a key" in the panel, and isn't offered in Chat or Images.
- **Chat and Images:**
  - one column per stored key, each `openai#NAME` (the unnamed key's is
    `openai#`);
  - plus `openai ($OPENAI_API_KEY)` when the variable is set;
  - with only one of these, just `openai`;
  - no column is called "default".
- **The Setup Wizard** always saves a named key. The name field is filled in
  with the provider's only key (or its default), and empty for a provider
  with no key. Its "use the environment variable" choice explains the
  variable and creates no key. Every panel action works on the open tab's
  key, by name.

### 12.7 Migration and compatibility

- **Nothing is renamed or moved on upgrade.** A key saved by 0.3.0 becomes
  the unnamed key `openai#` (§12.2 rule 1). As the provider's only key it's
  the default, so requests work as before. It gets a name only when you
  rename it.
- **Adding a second key** leaves the unnamed key the default (`default_key`
  absent means the unnamed key, while there is one), so a request that
  worked before keeps working. Make another key the default whenever you
  like.
- **The provider's variable comes out of the unnamed key's list.** 0.3.0
  configs list it there (`api_key: [saved, {env: OPENAI_API_KEY}]`), after
  the saved key. Rule 5 now checks it first, so `ensure_home` drops it from
  the list once (other variables a list names stay).
  - **This changes which key wins:** where both the saved key and
    `$OPENAI_API_KEY` are set, a request that names no key now uses the
    variable. The release notes say so.
  - Anyone who wants the stored key can unset the variable, or name the
    key.
- **§11 development-build configs:** `default_key: default` becomes "the
  unnamed key", and `openai#default` in a command is refused with a hint to
  write `openai#`.

**Downgrading to 0.3.0** keeps working while the original key is unnamed:
0.3.0 reads the provider's own slot. Two things it no longer finds:
- the provider's variable, once it's out of the list (put `{env:
  OPENAI_API_KEY}` back in `api_key` for 0.3.0 to read it);
- the original key once it's renamed, and any key added later. As in §6, the release notes say to run `providers api-key
show --reveal` first.

**Commands from 0.3.0:** `lexi providers api-key set openai`, `remove` and
`source add|rm` without a key name now fail, with the message from §12.5
showing the name to add. Scripts write `openai#` for the unnamed key, or
the key's name. The README,
getting-started and the release notes change to match.

### 12.8 Rollout

1. **Host, CLI and Python:**
   - the unnamed key as `openai#` (and renaming it into `keys`);
   - the provider's variable as its own rule, winning when set, and coming
     out of the unnamed key's list;
   - the name-required commands and the default rules;
   - `ready` / `uses` / `reason`;
   - removing `default` as a name;
   - no seeded keys.
2. **Desktop:** #85 reworked to match §12.6.

### 12.9 Open questions

1. **API and model own keys** (§12.3, §12.4): keep them as overrides for a
   request that names no key, ahead of the variable, as proposed, or drop
   them now that a key can be chosen per request?

Decided 2026-11-28, besides §12.2:
- **No starter keys.** A fresh install and a new provider have no keys (§12.3).
  Lexi never picks a name.
- **The provider's variable wins** over the ★ default for a request that
  names no key (rule 5).
