# API Key Storage — Design

**Status:** accepted · 2026-11-25 · steps 1 (source model), 2 (macOS store) and 3 (Windows and Linux stores) implemented
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
   hint.

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
