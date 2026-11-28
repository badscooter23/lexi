//! Where Lexi keeps the keys it saves: each scope's *saved key* (a
//! provider's, or one API's own), in exactly one place
//! (`docs/key-storage-design.md` §4.3):
//!
//! - **macOS:** the login keychain, through Apple's `/usr/bin/security` tool
//!   rather than the Security framework — the item then belongs to a signed
//!   Apple tool whose identity never changes, so there's no dialog however
//!   often Lexi is rebuilt, from the app, the CLI or Python. The key goes in
//!   on stdin (`security -i`, as hex), never on a command line.
//! - **Windows:** Credential Manager (a generic credential, target
//!   `Lexi:<account>`), encrypted with your login.
//! - **Linux with a desktop session:** the Secret Service (GNOME Keyring,
//!   KWallet) over D-Bus, through a pure-Rust client (no libdbus).
//! - **Anywhere else** — no store reachable (CI, containers, a server), or
//!   `key_store: file`: an owner-only (0600) file, `providers/<p>/api-key`
//!   (`api-key-<api>` for an API's own key).
//!
//! `key_store` in config.json picks the backend — `auto` (the default: the OS
//! store when there is one, else the file), `system` (the OS store or an
//! error) or `file` — and `LEXI_KEY_STORE` overrides it (tests, CI).
//!
//! With an OS store chosen, a key file left from before (an older version,
//! or a move that couldn't happen because the store was locked) is still
//! read when the store has no key, and [`move_into_store`] moves it in.

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use crate::keystore::{self, Finished, RunError};
use crate::HostError;

/// The config.json setting that picks the backend.
pub const KEY_STORE_SETTING: &str = "key_store";
/// The environment variable that overrides it.
const KEY_STORE_ENV: &str = "LEXI_KEY_STORE";
/// For tests (macOS): the keychain file to use instead of the login keychain.
#[cfg(target_os = "macos")]
const KEYCHAIN_ENV: &str = "LEXI_KEYCHAIN";
/// The values `key_store` takes.
pub const KEY_STORE_MODES: [&str; 3] = ["auto", "system", "file"];

const SECURITY: &str = "/usr/bin/security";
/// The stored items' service.
const SERVICE: &str = "Lexi";
/// How long the OS store may take (it can be waiting on an unlock dialog).
const STORE_TIMEOUT: Duration = Duration::from_secs(60);
/// `security`'s exit status for "the specified item could not be found"
/// (errSecItemNotFound, -25300, as a byte).
const NOT_FOUND: i32 = 44;
/// …and for "user interaction is not allowed" (errSecInteractionNotAllowed,
/// -25308): the keychain is locked and can't ask (over SSH).
const INTERACTION_NOT_ALLOWED: i32 = 36;

const KEYCHAIN_LOCKED: &str = "it's locked (as it is over SSH until you unlock it): unlock it with `security unlock-keychain`, put the key in an environment variable, or keep saved keys in a file with `lexi config set key_store file`";

/// One saved key: a provider's, or one API's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) provider: String,
    pub(crate) api: Option<String>,
}

impl Slot {
    pub(crate) fn new(provider: &str, api: Option<&str>) -> Slot {
        Slot {
            provider: provider.to_string(),
            api: api.map(String::from),
        }
    }

    /// Its key file, under `$LEXI_HOME`.
    pub(crate) fn file(&self) -> String {
        match &self.api {
            Some(a) => format!("providers/{}/api-key-{a}", self.provider),
            None => format!("providers/{}/api-key", self.provider),
        }
    }

    /// `anthropic's API key`, `openai's image-generations API key`.
    pub(crate) fn what(&self) -> String {
        match &self.api {
            Some(a) => format!("{}'s {a} API key", self.provider),
            None => format!("{}'s API key", self.provider),
        }
    }

    /// The stored item's account: `openai`, `openai/image-generations`,
    /// with `@<LEXI_HOME>` when that isn't the default `~/.lexi-cli`, so
    /// separate configs (tests, profiles) never share keys.
    fn account(&self, home: &Path) -> String {
        let mut account = match &self.api {
            Some(a) => format!("{}/{a}", self.provider),
            None => self.provider.clone(),
        };
        let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let default = dirs::home_dir().map(|h| canonical(&h.join(".lexi-cli")));
        let home = canonical(home);
        if default.as_deref() != Some(home.as_path()) {
            account.push('@');
            account.push_str(&home.to_string_lossy());
        }
        account
    }

    /// The stored item's name, as Keychain Access (the label) or Credential
    /// Manager (the comment) shows it.
    fn label(&self) -> String {
        match &self.api {
            Some(a) => format!("Lexi: {} {a} API key", self.provider),
            None => format!("Lexi: {} API key", self.provider),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Backend {
    File,
    /// The macOS keychain: the login keychain, or (tests) this keychain file.
    /// (Only made on macOS; elsewhere its code is never reached.)
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Keychain(Option<PathBuf>),
    /// Windows Credential Manager, or the Secret Service (Linux).
    #[cfg(any(windows, target_os = "linux"))]
    Os,
}

/// Where this machine's saved keys are kept ([`Store::open`]).
#[derive(Clone, Debug)]
pub(crate) struct Store {
    home: PathBuf,
    backend: Backend,
}

/// A `key_store` value: `auto`, `system` or `file` (any case, trimmed).
pub(crate) fn parse_mode(value: &str) -> Option<&'static str> {
    let v = value.trim().to_ascii_lowercase();
    KEY_STORE_MODES.into_iter().find(|m| *m == v)
}

/// Whether the keychain item `security` failed on is locked.
fn locked(out: &Finished) -> bool {
    out.code == Some(INTERACTION_NOT_ALLOWED)
        || out.stderr.contains("User interaction is not allowed")
}

/// Whether a saved key's read error is a locked store.
pub(crate) fn is_locked_error(err: &str) -> bool {
    #[cfg(any(windows, target_os = "linux"))]
    if err == os::LOCKED {
        return true;
    }
    err == KEYCHAIN_LOCKED
}

/// An argument for a `security -i` line: double-quoted, `\` and `"` escaped
/// (the tool's own line parser). A line break can't be written.
fn quote(arg: &str) -> Result<String, String> {
    if arg.contains(['\n', '\r']) {
        return Err(format!("{arg:?} can't be passed to the macOS Keychain"));
    }
    Ok(format!(
        "\"{}\"",
        arg.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The OS store this machine has, if any (whether one is reachable is
/// checked once per process).
#[cfg(any(windows, target_os = "linux"))]
fn system_backend() -> Option<Backend> {
    static REACHABLE: OnceLock<bool> = OnceLock::new();
    REACHABLE.get_or_init(os::reachable).then_some(Backend::Os)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn system_backend() -> Option<Backend> {
    None
}

#[cfg(target_os = "macos")]
fn system_backend() -> Option<Backend> {
    if let Some(path) = std::env::var_os(KEYCHAIN_ENV).filter(|p| !p.is_empty()) {
        return Some(Backend::Keychain(Some(PathBuf::from(path))));
    }
    static REACHABLE: OnceLock<bool> = OnceLock::new();
    let reachable = *REACHABLE.get_or_init(|| {
        let mut cmd = Command::new(SECURITY);
        cmd.arg("default-keychain");
        Path::new(SECURITY).exists()
            && keystore::run_process(cmd, None, Duration::from_secs(10))
                .is_ok_and(|out| out.ok() && !out.stdout.trim().is_empty())
    });
    reachable.then_some(Backend::Keychain(None))
}

impl Store {
    /// The store `home`'s saved keys are kept in: `LEXI_KEY_STORE`, else
    /// `key_store` in its config.json, else `auto`. (Unit tests always get
    /// the file store, so they never touch a real keychain.)
    pub(crate) fn open(home: &Path) -> Result<Store, HostError> {
        if cfg!(test) {
            return Ok(Store::file(home));
        }
        let env = std::env::var(KEY_STORE_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty());
        let (value, from) = match env {
            Some(v) => (v, KEY_STORE_ENV),
            None => (
                crate::config::load_config_in(home)
                    .ok()
                    .and_then(|c| {
                        c.get(KEY_STORE_SETTING)
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_else(|| "auto".into()),
                "key_store in config.json",
            ),
        };
        let backend = match parse_mode(&value) {
            Some("file") => Backend::File,
            Some("system") => system_backend().ok_or_else(|| {
                HostError::Config(format!(
                    "{from} is system, but no key store is reachable here (on Linux, a Secret Service such as GNOME Keyring or KWallet in a desktop session): set it to auto or file"
                ))
            })?,
            Some(_) => system_backend().unwrap_or(Backend::File),
            None => {
                return Err(HostError::Config(format!(
                    "{from} is {value:?}: use {}",
                    KEY_STORE_MODES.join(", ")
                )))
            }
        };
        Ok(Store {
            home: home.to_path_buf(),
            backend,
        })
    }

    /// Saved keys in their key files.
    pub(crate) fn file(home: &Path) -> Store {
        Store {
            home: home.to_path_buf(),
            backend: Backend::File,
        }
    }

    /// Saved keys in the keychain file `keychain` (tests).
    #[cfg(all(test, target_os = "macos"))]
    fn keychain_at(home: &Path, keychain: &Path) -> Store {
        Store {
            home: home.to_path_buf(),
            backend: Backend::Keychain(Some(keychain.to_path_buf())),
        }
    }

    /// `keychain` (macOS), `credential-manager` (Windows), `secret-service`
    /// (Linux) or `file`.
    pub(crate) fn kind(&self) -> &'static str {
        match self.backend {
            Backend::File => "file",
            Backend::Keychain(_) => "keychain",
            #[cfg(any(windows, target_os = "linux"))]
            Backend::Os => os::KIND,
        }
    }

    /// The OS store, for messages: `the macOS Keychain`, `Windows
    /// Credential Manager`, `your desktop keyring` (none for the file).
    fn name(&self) -> Option<&'static str> {
        match self.backend {
            Backend::File => None,
            Backend::Keychain(_) => Some("the macOS Keychain"),
            #[cfg(any(windows, target_os = "linux"))]
            Backend::Os => Some(os::NAME),
        }
    }

    /// Where `slot`'s key is kept, for messages: the OS store's name, or the
    /// key file.
    pub(crate) fn location(&self, slot: &Slot) -> String {
        self.name().map(String::from).unwrap_or_else(|| slot.file())
    }

    /// `slot`'s key, `None` when none is saved; an error is a read error
    /// (an unreadable file, a locked keychain).
    pub(crate) fn read(&self, slot: &Slot) -> Result<Option<String>, String> {
        match &self.backend {
            Backend::File => self.read_file(slot),
            _ => match self.find(slot) {
                Ok(Some(key)) => Ok(Some(key)),
                // A key file left from before is read until it's moved in.
                Ok(None) => self.read_file(slot),
                Err(e) => match self.read_file(slot) {
                    Ok(Some(key)) => Ok(Some(key)),
                    _ => Err(e),
                },
            },
        }
    }

    /// Save `key` as `slot`'s key (replacing any), keeping one copy: a key
    /// file left from before is deleted once the store has it. A locked
    /// store is an error — the key never goes to a file instead.
    pub(crate) fn write(&self, slot: &Slot, key: &str) -> Result<(), HostError> {
        match &self.backend {
            Backend::File => write_key_file(&keystore::file_path(&self.home, &slot.file()), key),
            _ => {
                self.put(slot, key)
                    .map_err(|e| self.store_error("save", slot, &e))?;
                self.remove_file(slot)?;
                Ok(())
            }
        }
    }

    /// Delete `slot`'s key (from the OS store and any key file). Whether
    /// there was one.
    pub(crate) fn delete(&self, slot: &Slot) -> Result<bool, HostError> {
        let in_store = match &self.backend {
            Backend::File => false,
            _ => self
                .delete_item(slot)
                .map_err(|e| self.store_error("delete", slot, &e))?,
        };
        Ok(self.remove_file(slot)? || in_store)
    }

    /// Give `from`'s saved key to `to` (a renamed provider); `from`'s goes.
    /// Key files move with their directory, so only stored items change.
    pub(crate) fn rename(&self, from: &Slot, to: &Slot) -> Result<(), HostError> {
        if self.backend != Backend::File {
            let fail = |e: String| self.store_error("move", from, &e);
            if let Some(key) = self.find(from).map_err(fail)? {
                self.put(to, &key).map_err(fail)?;
                self.delete_item(from).map_err(fail)?;
            }
        }
        Ok(())
    }

    /// Move `slot`'s key file into the OS store: put in, read back, then the
    /// file deleted. Whether a key moved (an error leaves the file as it is).
    pub(crate) fn move_file_in(&self, slot: &Slot) -> Result<bool, String> {
        if self.backend == Backend::File {
            return Ok(false);
        }
        let Some(key) = self.read_file(slot)? else {
            return Ok(false);
        };
        self.put(slot, &key)?;
        if self.find(slot)?.as_deref() != Some(key.as_str()) {
            return Err(format!(
                "{} didn't keep {}",
                self.location(slot),
                slot.what()
            ));
        }
        self.remove_file(slot).map_err(|e| e.to_string())?;
        Ok(true)
    }

    fn read_file(&self, slot: &Slot) -> Result<Option<String>, String> {
        let path = keystore::file_path(&self.home, &slot.file());
        match std::fs::read_to_string(&path) {
            Ok(v) if !v.trim().is_empty() => Ok(Some(v.trim().to_string())),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn remove_file(&self, slot: &Slot) -> Result<bool, HostError> {
        match std::fs::remove_file(keystore::file_path(&self.home, &slot.file())) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// The keychain file to name on each `security` call (tests), if any.
    fn keychain(&self) -> Option<&Path> {
        match &self.backend {
            Backend::Keychain(k) => k.as_deref(),
            _ => None,
        }
    }

    /// "couldn't save openai's API key in the macOS Keychain: it's locked …".
    fn store_error(&self, verb: &str, slot: &Slot, err: &str) -> HostError {
        HostError::Config(format!(
            "couldn't {verb} {} in {}: {err}",
            slot.what(),
            self.location(slot)
        ))
    }

    /// The OS store's key for `slot`.
    fn find(&self, slot: &Slot) -> Result<Option<String>, String> {
        match &self.backend {
            Backend::File => Ok(None),
            Backend::Keychain(_) => self.keychain_find(slot),
            #[cfg(any(windows, target_os = "linux"))]
            Backend::Os => os::find(slot.account(&self.home)),
        }
    }

    /// Put `key` in the OS store as `slot`'s (replacing any).
    fn put(&self, slot: &Slot, key: &str) -> Result<(), String> {
        match &self.backend {
            Backend::File => Ok(()),
            Backend::Keychain(_) => self.keychain_put(slot, key),
            #[cfg(any(windows, target_os = "linux"))]
            Backend::Os => os::put(slot.account(&self.home), slot.label(), key.to_string()),
        }
    }

    /// Delete `slot`'s item from the OS store; whether there was one.
    fn delete_item(&self, slot: &Slot) -> Result<bool, String> {
        match &self.backend {
            Backend::File => Ok(false),
            Backend::Keychain(_) => self.keychain_delete(slot),
            #[cfg(any(windows, target_os = "linux"))]
            Backend::Os => os::delete(slot.account(&self.home)),
        }
    }

    fn security(&self, args: &[&str], input: Option<String>) -> Result<Finished, String> {
        if !cfg!(target_os = "macos") {
            return Err("the macOS Keychain is only on macOS".into());
        }
        let mut cmd = std::process::Command::new(SECURITY);
        cmd.args(args);
        keystore::run_process(cmd, input.map(String::into_bytes), STORE_TIMEOUT)
            .map_err(|e: RunError| e.about("security"))
    }

    /// `security`'s failure as a message (a locked keychain says how to fix it).
    fn failed(out: &Finished) -> String {
        if locked(out) {
            KEYCHAIN_LOCKED.to_string()
        } else {
            format!("`security` {}", out.failure())
        }
    }

    /// The keychain's key for `slot`.
    fn keychain_find(&self, slot: &Slot) -> Result<Option<String>, String> {
        let account = slot.account(&self.home);
        let mut args = vec!["find-generic-password", "-s", SERVICE, "-a", &account, "-w"];
        let keychain = self.keychain().map(|k| k.to_string_lossy().into_owned());
        args.extend(keychain.as_deref());
        let out = self.security(&args, None)?;
        match out.code {
            Some(0) => {
                let key = out.stdout.trim();
                Ok((!key.is_empty()).then(|| key.to_string()))
            }
            Some(NOT_FOUND) => Ok(None),
            _ => Err(Store::failed(&out)),
        }
    }

    /// Put `key` in the keychain as `slot`'s (an item already there is
    /// updated). The key goes on stdin, as hex, never on a command line.
    fn keychain_put(&self, slot: &Slot, key: &str) -> Result<(), String> {
        let mut line = format!(
            "add-generic-password -U -s {} -a {} -l {} -X {}",
            quote(SERVICE)?,
            quote(&slot.account(&self.home))?,
            quote(&slot.label())?,
            hex(key.as_bytes())
        );
        if let Some(k) = self.keychain() {
            line.push(' ');
            line.push_str(&quote(&k.to_string_lossy())?);
        }
        line.push('\n');
        let out = self.security(&["-i"], Some(line))?;
        if out.ok() {
            Ok(())
        } else {
            Err(Store::failed(&out))
        }
    }

    /// Delete `slot`'s keychain item; whether there was one.
    fn keychain_delete(&self, slot: &Slot) -> Result<bool, String> {
        let account = slot.account(&self.home);
        let mut args = vec!["delete-generic-password", "-s", SERVICE, "-a", &account];
        let keychain = self.keychain().map(|k| k.to_string_lossy().into_owned());
        args.extend(keychain.as_deref());
        let out = self.security(&args, None)?;
        match out.code {
            Some(0) => Ok(true),
            Some(NOT_FOUND) => Ok(false),
            _ => Err(Store::failed(&out)),
        }
    }
}

/// Save `key` in the file at `path`, readable only by you.
pub(crate) fn write_key_file(path: &Path, key: &str) -> Result<(), HostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{key}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The saved keys `home`'s key files hold for `providers`, as slots: each
/// provider's `api-key` and `api-key-<api>` files.
pub(crate) fn key_file_slots(home: &Path, providers: &[String]) -> Vec<Slot> {
    let mut out = Vec::new();
    for p in providers {
        let Ok(dir) = std::fs::read_dir(home.join("providers").join(p)) else {
            continue;
        };
        let mut names: Vec<String> = dir
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.file_name().to_str().map(String::from))
            .collect();
        names.sort();
        for name in names {
            if name == "api-key" {
                out.push(Slot::new(p, None));
            } else if let Some(api) = name.strip_prefix("api-key-").filter(|a| !a.is_empty()) {
                out.push(Slot::new(p, Some(api)));
            }
        }
    }
    out
}

/// Move the key files `home` has for `providers` into the OS store, when
/// that's where saved keys are kept (`docs/key-storage-design.md` §6 step 2):
/// what moved, as messages ("Moved anthropic's API key into the macOS
/// Keychain."). A key that can't move (a locked store) stays in its file
/// and is read from there.
pub(crate) fn move_into_store(home: &Path, providers: &[String]) -> Vec<String> {
    let Ok(store) = Store::open(home) else {
        return Vec::new();
    };
    if store.backend == Backend::File {
        return Vec::new();
    }
    key_file_slots(home, providers)
        .into_iter()
        .filter(|slot| store.move_file_in(slot) == Ok(true))
        .map(|slot| format!("Moved {} into {}.", slot.what(), store.location(&slot)))
        .collect()
}

/// Windows Credential Manager and the Secret Service, through the `keyring`
/// crate. Each call runs on its own thread with a time limit, so a store
/// that never answers (an unlock dialog nobody sees) can't hang Lexi.
#[cfg(any(windows, target_os = "linux"))]
mod os {
    use std::time::Duration;

    use keyring::{Entry, Error};

    use super::{SERVICE, STORE_TIMEOUT};

    #[cfg(windows)]
    pub(super) const KIND: &str = "credential-manager";
    #[cfg(windows)]
    pub(super) const NAME: &str = "Windows Credential Manager";
    #[cfg(windows)]
    pub(super) const LOCKED: &str = "it can't be used in this session (as over SSH): put the key in an environment variable, or keep saved keys in a file with `lexi config set key_store file`";

    #[cfg(target_os = "linux")]
    pub(super) const KIND: &str = "secret-service";
    #[cfg(target_os = "linux")]
    pub(super) const NAME: &str = "your desktop keyring";
    #[cfg(target_os = "linux")]
    pub(super) const LOCKED: &str = "it's locked: unlock your keyring (log in to the desktop, or run `gnome-keyring-daemon --unlock`), put the key in an environment variable, or keep saved keys in a file with `lexi config set key_store file`";

    fn entry(account: &str, label: Option<&str>) -> Result<Entry, Error> {
        #[cfg(windows)]
        {
            let mut c = keyring::windows::WinCredential::new_with_target(
                Some(&format!("{SERVICE}:{account}")),
                SERVICE,
                account,
            )?;
            if let Some(l) = label {
                c.comment = l.to_string();
            }
            Ok(Entry::new_with_credential(Box::new(c)))
        }
        #[cfg(target_os = "linux")]
        {
            let mut c =
                keyring::secret_service::SsCredential::new_with_target(None, SERVICE, account)?;
            if let Some(l) = label {
                c.label = l.to_string();
            }
            Ok(Entry::new_with_credential(Box::new(c)))
        }
    }

    fn describe(e: Error) -> String {
        match e {
            Error::NoStorageAccess(_) => LOCKED.to_string(),
            other => other.to_string(),
        }
    }

    /// Run `f` on its own thread, waiting at most `limit` for it.
    fn limited<T: Send + 'static>(
        limit: Duration,
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        crate::keystore::wait_blocking(|| rx.recv_timeout(limit)).unwrap_or_else(|_| {
            Err(format!(
                "{NAME} didn't answer within {} seconds",
                limit.as_secs()
            ))
        })
    }

    /// Whether the store answers: looking up an item that isn't there finds
    /// nothing (rather than failing).
    pub(super) fn reachable() -> bool {
        limited(Duration::from_secs(10), || {
            match entry("lexi-probe", None).and_then(|e| e.get_password()) {
                Ok(_) | Err(Error::NoEntry) => Ok(true),
                Err(_) => Ok(false),
            }
        })
        .unwrap_or(false)
    }

    pub(super) fn find(account: String) -> Result<Option<String>, String> {
        limited(STORE_TIMEOUT, move || {
            match entry(&account, None).and_then(|e| e.get_password()) {
                Ok(k) if !k.trim().is_empty() => Ok(Some(k.trim().to_string())),
                Ok(_) | Err(Error::NoEntry) => Ok(None),
                Err(e) => Err(describe(e)),
            }
        })
    }

    pub(super) fn put(account: String, label: String, key: String) -> Result<(), String> {
        limited(STORE_TIMEOUT, move || {
            entry(&account, Some(&label))
                .and_then(|e| e.set_password(&key))
                .map_err(describe)
        })
    }

    pub(super) fn delete(account: String) -> Result<bool, String> {
        limited(STORE_TIMEOUT, move || {
            match entry(&account, None).and_then(|e| e.delete_credential()) {
                Ok(()) => Ok(true),
                Err(Error::NoEntry) => Ok(false),
                Err(e) => Err(describe(e)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_auto_system_or_file() {
        assert_eq!(parse_mode(" System "), Some("system"));
        assert_eq!(parse_mode("file"), Some("file"));
        assert_eq!(parse_mode("auto"), Some("auto"));
        assert_eq!(parse_mode("keychain"), None);
    }

    #[test]
    fn security_lines_quote_their_arguments() {
        assert_eq!(quote("a b").unwrap(), "\"a b\"");
        assert_eq!(quote(r#"it's "x" \ y"#).unwrap(), r#""it's \"x\" \\ y""#);
        assert!(quote("a\nb").is_err());
        assert_eq!(hex(b"sk-1"), "736b2d31");
    }

    #[test]
    fn a_locked_keychain_is_told_apart() {
        let finished = |code, stderr: &str| Finished {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.into(),
        };
        let ssh = finished(
            INTERACTION_NOT_ALLOWED,
            "security: SecKeychainSearchCopyNext: User interaction is not allowed.",
        );
        assert!(locked(&ssh));
        assert!(is_locked_error(&Store::failed(&ssh)));
        let other = finished(51, "security: SecKeychainItemCopyContent: bad password");
        assert!(!locked(&other));
        assert_eq!(
            Store::failed(&other),
            "`security` exited with status 51: security: SecKeychainItemCopyContent: bad password"
        );
    }

    #[test]
    fn slots_name_their_file_and_keychain_item() {
        let home = tempfile::tempdir().unwrap();
        let p = Slot::new("openai", None);
        let a = Slot::new("openai", Some("image-generations"));
        assert_eq!(p.file(), "providers/openai/api-key");
        assert_eq!(a.file(), "providers/openai/api-key-image-generations");
        assert_eq!(a.what(), "openai's image-generations API key");
        assert_eq!(p.label(), "Lexi: openai API key");
        // A home that isn't ~/.lexi-cli is part of the account.
        let canon = home.path().canonicalize().unwrap();
        assert_eq!(
            a.account(home.path()),
            format!("openai/image-generations@{}", canon.display())
        );
    }

    #[test]
    fn the_file_store_keeps_one_owner_only_file() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::file(home.path());
        let slot = Slot::new("p", None);
        assert_eq!(store.read(&slot), Ok(None));
        store.write(&slot, "k1").unwrap();
        assert_eq!(store.read(&slot), Ok(Some("k1".into())));
        assert_eq!(store.location(&slot), "providers/p/api-key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.path().join("providers/p/api-key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(store.move_file_in(&slot), Ok(false));
        assert!(store.delete(&slot).unwrap());
        assert!(!store.delete(&slot).unwrap());
    }

    #[test]
    fn key_files_are_found_per_provider() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("providers/openai");
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["api-key", "api-key-image-generations", "model-aliases.yaml"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let slots = key_file_slots(home.path(), &["openai".into(), "none".into()]);
        assert!(
            slots
                == vec![
                    Slot::new("openai", None),
                    Slot::new("openai", Some("image-generations"))
                ]
        );
    }

    /// The keychain backend against a throw-away keychain. It runs only when
    /// `LEXI_TEST_KEY_STORE` is set (CI): making a keychain adds it to
    /// the keychain search list, which a developer's Mac shouldn't have done
    /// to it by a test run. The test puts the list back afterwards.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_keychain_store_saves_reads_moves_and_deletes() {
        if !testing_os_store() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let keychain = dir.path().join("lexi test.keychain-db");
        let security = |args: &[&str]| {
            let out = Command::new(SECURITY).args(args).output().unwrap();
            assert!(
                out.status.success(),
                "security {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        let search_list = security(&["list-keychains", "-d", "user"]);
        let k = keychain.to_str().unwrap();
        security(&["create-keychain", "-p", "pw", k]);
        let listed: Vec<String> = search_list
            .lines()
            .map(|l| l.trim().trim_matches('"').to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let mut restore = vec!["list-keychains", "-d", "user", "-s"];
        restore.extend(listed.iter().map(String::as_str));
        security(&restore);
        security(&["unlock-keychain", "-p", "pw", k]);

        let store = Store::keychain_at(&home, &keychain);
        assert_eq!(store.location(&Slot::new("p", None)), "the macOS Keychain");
        // (A locked keychain isn't tested: with a desktop session, as a CI
        // runner has, `security` waits on an unlock dialog instead of failing.)
        exercise(&store, &home);
        security(&["delete-keychain", k]);
    }

    /// Whether to test against the real OS store: only with
    /// `LEXI_TEST_KEY_STORE` set (CI), never on a developer's machine by
    /// default.
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    fn testing_os_store() -> bool {
        let on = std::env::var_os("LEXI_TEST_KEY_STORE").is_some_and(|v| !v.is_empty());
        if !on {
            eprintln!("skipped: set LEXI_TEST_KEY_STORE=1 to test against the OS key store");
        }
        on
    }

    /// Save, read, move a key file in, replace, rename and delete, in `store`
    /// (whose home is `home`).
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    fn exercise(store: &Store, home: &Path) {
        // Quotes and spaces in the name (Windows file names can't hold `"`).
        let name = if cfg!(windows) {
            "my 'odd' provider"
        } else {
            "my \"odd\" provider"
        };
        let slot = Slot::new(name, Some("image-generations"));
        assert_eq!(store.read(&slot), Ok(None));

        // A key file from before is read, then moved in.
        let file = home.join(slot.file());
        write_key_file(&file, "sk-from-file").unwrap();
        assert_eq!(store.read(&slot), Ok(Some("sk-from-file".into())));
        assert_eq!(store.move_file_in(&slot), Ok(true));
        assert!(!file.exists());
        assert_eq!(store.read(&slot), Ok(Some("sk-from-file".into())));

        // Saving replaces it, and leaves no file.
        store.write(&slot, "sk-new 'quoted' \"key\"").unwrap();
        assert_eq!(
            store.read(&slot),
            Ok(Some("sk-new 'quoted' \"key\"".into()))
        );
        assert!(!file.exists());

        // A renamed provider takes its key along.
        let renamed = Slot::new("renamed", Some("image-generations"));
        store.rename(&slot, &renamed).unwrap();
        assert_eq!(store.read(&slot), Ok(None));
        assert_eq!(
            store.read(&renamed),
            Ok(Some("sk-new 'quoted' \"key\"".into()))
        );

        assert!(store.delete(&renamed).unwrap());
        assert!(!store.delete(&renamed).unwrap());
    }

    /// Windows Credential Manager / the Secret Service, under accounts made
    /// unique by the temp home (and deleted at the end). CI runs it with
    /// `LEXI_TEST_KEY_STORE` set (Linux: in a private D-Bus session with
    /// gnome-keyring unlocked).
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn the_os_store_saves_reads_moves_and_deletes() {
        if !testing_os_store() {
            return;
        }
        assert!(os::reachable(), "{} isn't reachable", os::NAME);
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let store = Store {
            home: home.clone(),
            backend: Backend::Os,
        };
        assert_eq!(store.location(&Slot::new("p", None)), os::NAME);
        exercise(&store, &home);
    }
}
