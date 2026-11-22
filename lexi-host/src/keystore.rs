//! Where API keys come from. A provider's `api_key` in providers-config.yaml
//! is an ordered list of sources — `{env: VAR}`, `{keychain: NAME}`,
//! `{file: PATH}` (or the older `${VAR}` / literal strings) — and the first
//! one that has a key is used. Only a missing key (an unset variable, no
//! keychain item, no file) moves on to the next; a read error stops the lookup,
//! so a locked keychain never silently switches you to a different key.
//!
//! Keychain keys live in the OS credential store under service `Lexi`: on
//! macOS all of them in one login-Keychain item (`Lexi API keys`, so a new
//! build is asked for access once, not once per key), on Windows one
//! Credential Manager item per NAME.
//! There's no OS store elsewhere, and `LEXI_KEY_STORE=file` (tests, CI,
//! headless machines) turns it off: keychain sources then count as missing and
//! a key Lexi saves goes in an owner-only file.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The keychain service name every Lexi item is filed under.
const SERVICE: &str = "Lexi";

/// Whether this platform has an OS credential store Lexi can use.
pub(crate) const NATIVE: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The kind of source Lexi adds when it saves a key and the config names
/// nowhere to put it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyStore {
    Keychain,
    File,
}

impl KeyStore {
    /// The store in effect: `LEXI_KEY_STORE=file|keychain`, else the OS store
    /// when there is one.
    pub(crate) fn current() -> KeyStore {
        match std::env::var("LEXI_KEY_STORE").as_deref() {
            Ok("file") => KeyStore::File,
            _ if NATIVE => KeyStore::Keychain,
            _ => KeyStore::File,
        }
    }

    /// `"keychain"` or `"file"` — also the `source` a key read from it reports.
    pub(crate) fn id(self) -> &'static str {
        match self {
            KeyStore::Keychain => "keychain",
            KeyStore::File => "file",
        }
    }

    /// What to call it in messages.
    pub(crate) fn label(self) -> &'static str {
        match self {
            KeyStore::Keychain if cfg!(target_os = "windows") => "Windows Credential Manager",
            KeyStore::Keychain => "macOS Keychain",
            KeyStore::File => "an owner-only key file",
        }
    }
}

/// The keychain account Lexi names for `label` (a provider, or
/// `provider/api`) under `home`: a non-default `$LEXI_HOME` gets its own
/// accounts so separate homes never share keys.
pub(crate) fn account(home: &Path, label: &str) -> String {
    let default_home = dirs::home_dir().map(|h| h.join(".lexi-cli"));
    if default_home.as_deref() == Some(home) {
        label.to_string()
    } else {
        format!("{label} ({})", home.display())
    }
}

/// One place a key can come from: an entry of an `api_key` list. (No `Debug`:
/// a literal holds the key itself.)
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// An environment variable.
    Env(String),
    /// An OS keychain item (service `Lexi`, this account).
    Keychain(String),
    /// A file holding the key; relative paths are under `$LEXI_HOME`.
    File(String),
    /// The key written into providers-config.yaml itself.
    Literal(String),
}

const SOURCE_FORMS: &str =
    "api_key entries are {env: VAR}, {keychain: NAME}, {file: PATH} or \"${VAR}\"";

impl Source {
    /// `env`, `keychain`, `file` or `config` (a literal).
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Source::Env(_) => "env",
            Source::Keychain(_) => "keychain",
            Source::File(_) => "file",
            Source::Literal(_) => "config",
        }
    }

    /// Whether Lexi can save a key here (a keychain item or a file).
    pub(crate) fn writable(&self) -> bool {
        matches!(self, Source::Keychain(_) | Source::File(_))
    }

    /// Where it is, for messages: `$OPENAI_API_KEY`, `macOS Keychain (Lexi
    /// API keys: openai)`, a path.
    pub(crate) fn describe(&self) -> String {
        match self {
            Source::Env(v) => format!("${v}"),
            Source::Keychain(a) => keychain_place(a),
            Source::File(p) => p.clone(),
            Source::Literal(_) => "providers-config.yaml (the key itself)".into(),
        }
    }

    /// As written in providers-config.yaml.
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Source::Env(v) => json!({ "env": v }),
            Source::Keychain(a) => json!({ "keychain": a }),
            Source::File(p) => json!({ "file": p }),
            Source::Literal(k) => Value::from(k.as_str()),
        }
    }

    fn parse(v: &Value) -> Result<Option<Source>, String> {
        match v {
            Value::Null => Ok(None),
            Value::String(s) if s.trim().is_empty() => Ok(None),
            Value::String(s) => Ok(Some(
                match s.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
                    Some(var) => Source::Env(var.to_string()),
                    None => Source::Literal(s.clone()),
                },
            )),
            Value::Object(o) if o.len() == 1 => {
                let (k, v) = o.iter().next().expect("one entry");
                let name = v
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("api_key {k}: needs a name or path ({SOURCE_FORMS})"))?
                    .to_string();
                match k.as_str() {
                    "env" => Ok(Some(Source::Env(name))),
                    "keychain" => Ok(Some(Source::Keychain(name))),
                    "file" => Ok(Some(Source::File(name))),
                    other => Err(format!("unknown api_key source {other:?} ({SOURCE_FORMS})")),
                }
            }
            _ => Err(SOURCE_FORMS.to_string()),
        }
    }
}

/// An `api_key` value (a source, or a list of them) as its sources, in order.
pub(crate) fn parse_sources(v: Option<&Value>) -> Result<Vec<Source>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| Source::parse(i).transpose())
            .collect(),
        Some(one) => Ok(Source::parse(one)?.into_iter().collect()),
    }
}

/// Sources as an `api_key` value (always a list).
pub(crate) fn sources_value(sources: &[Source]) -> Value {
    Value::Array(sources.iter().map(Source::to_value).collect())
}

/// A file source's path: `~/…` is under your home folder, a relative path is
/// under `$LEXI_HOME`.
pub(crate) fn file_path(home: &Path, raw: &str) -> PathBuf {
    if raw == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        home.join(p)
    }
}

/// What looking at one source found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// It had the key that's used.
    Used,
    /// No key there (`not set`, `no item`, `no file`, `empty`) — the next
    /// source is tried.
    Missing(&'static str),
    /// It can't be used here (no keychain on this platform, or
    /// `LEXI_KEY_STORE=file`) — counts as missing.
    Unavailable(&'static str),
    /// Reading it failed; the lookup stops here.
    Failed(String),
    /// An earlier source decided the lookup.
    NotChecked,
}

impl Outcome {
    /// `used`, `missing`, `unavailable`, `error` or `not checked`.
    pub(crate) fn state(&self) -> &'static str {
        match self {
            Outcome::Used => "used",
            Outcome::Missing(_) => "missing",
            Outcome::Unavailable(_) => "unavailable",
            Outcome::Failed(_) => "error",
            Outcome::NotChecked => "not checked",
        }
    }

    pub(crate) fn note(&self) -> Option<String> {
        match self {
            Outcome::Missing(n) | Outcome::Unavailable(n) => Some((*n).to_string()),
            Outcome::Failed(e) => Some(e.clone()),
            _ => None,
        }
    }
}

/// The result of looking for a key through a list of sources.
pub(crate) struct Lookup {
    pub(crate) key: Option<String>,
    pub(crate) checked: Vec<(Source, Outcome)>,
}

impl Lookup {
    /// The source the key came from.
    pub(crate) fn used(&self) -> Option<&Source> {
        self.checked
            .iter()
            .find(|(_, o)| *o == Outcome::Used)
            .map(|(s, _)| s)
    }

    /// The source whose read failed, and why.
    pub(crate) fn failure(&self) -> Option<(&Source, &str)> {
        self.checked.iter().find_map(|(s, o)| match o {
            Outcome::Failed(e) => Some((s, e.as_str())),
            _ => None,
        })
    }

    /// `$OPENAI_API_KEY (not set), macOS Keychain (Lexi API keys: openai) (no item)`.
    pub(crate) fn summary(&self) -> String {
        self.checked
            .iter()
            .map(|(s, o)| match o.note() {
                Some(n) => format!("{} ({n})", s.describe()),
                None => s.describe(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Each source as `{kind, from, state, note?}`.
    pub(crate) fn to_value(&self) -> Value {
        Value::Array(
            self.checked
                .iter()
                .map(|(s, o)| {
                    let mut v =
                        json!({ "kind": s.kind(), "from": s.describe(), "state": o.state() });
                    if let Some(n) = o.note() {
                        v["note"] = Value::from(n);
                    }
                    v
                })
                .collect(),
        )
    }
}

/// Look through `sources` in order for a key.
pub(crate) fn lookup(home: &Path, sources: &[Source], secrets: Option<&dyn Secrets>) -> Lookup {
    lookup_with(home, sources, secrets, false)
}

/// Like [`lookup`], but a keychain item is only checked for — its secret
/// isn't read, so macOS doesn't ask for access. Which source would be used
/// is the same; `key` is `None` when that's a keychain item. For status
/// (does a provider have a key, and where from), not for requests.
pub(crate) fn probe(home: &Path, sources: &[Source], secrets: Option<&dyn Secrets>) -> Lookup {
    lookup_with(home, sources, secrets, true)
}

fn lookup_with(
    home: &Path,
    sources: &[Source],
    secrets: Option<&dyn Secrets>,
    probe: bool,
) -> Lookup {
    let mut key = None;
    let mut checked: Vec<(Source, Outcome)> = Vec::new();
    for source in sources {
        let decided = checked
            .iter()
            .any(|(_, o)| matches!(o, Outcome::Used | Outcome::Failed(_)));
        if decided {
            checked.push((source.clone(), Outcome::NotChecked));
            continue;
        }
        let outcome = match source {
            Source::Env(var) => match std::env::var(var) {
                Ok(v) if !v.trim().is_empty() => {
                    key = Some(v.trim().to_string());
                    Outcome::Used
                }
                _ => Outcome::Missing("not set"),
            },
            Source::Keychain(account) => match secrets {
                None if NATIVE => Outcome::Unavailable("LEXI_KEY_STORE=file"),
                None => Outcome::Unavailable("no keychain on this platform"),
                Some(store) if probe => match store.exists(account) {
                    Ok(true) => Outcome::Used,
                    Ok(false) => Outcome::Missing("no item"),
                    Err(e) => Outcome::Failed(e),
                },
                Some(store) => match store.get(account) {
                    Ok(Some(v)) if !v.trim().is_empty() => {
                        key = Some(v.trim().to_string());
                        Outcome::Used
                    }
                    Ok(_) => Outcome::Missing("no item"),
                    Err(e) => Outcome::Failed(e),
                },
            },
            Source::File(raw) => {
                let path = file_path(home, raw);
                if !path.exists() {
                    Outcome::Missing("no file")
                } else {
                    match std::fs::read_to_string(&path) {
                        Ok(v) if !v.trim().is_empty() => {
                            key = Some(v.trim().to_string());
                            Outcome::Used
                        }
                        Ok(_) => Outcome::Missing("empty"),
                        Err(e) => Outcome::Failed(e.to_string()),
                    }
                }
            }
            Source::Literal(k) => {
                key = Some(k.trim().to_string());
                Outcome::Used
            }
        };
        checked.push((source.clone(), outcome));
    }
    Lookup { key, checked }
}

/// A secret store keyed by account (the OS keychain, or a map in tests).
pub(crate) trait Secrets {
    /// The saved secret, `None` when there's no item.
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    /// Whether there's an item, without reading its secret where the store
    /// can tell (on macOS reading the secret is what asks for access).
    fn exists(&self, account: &str) -> Result<bool, String> {
        self.get(account)
            .map(|s| s.is_some_and(|v| !v.trim().is_empty()))
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    /// Delete the item; whether there was one.
    fn delete(&self, account: &str) -> Result<bool, String>;
}

/// The macOS Keychain item that holds every key Lexi saves there (service
/// `Lexi`, this account), and the name Keychain Access shows for it.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
const BUNDLE_ACCOUNT: &str = "API keys";
const BUNDLE_LABEL: &str = "Lexi API keys";

/// Where a `{keychain: NAME}` key lives, for messages: `macOS Keychain (Lexi
/// API keys: openai)`, or `Windows Credential Manager item Lexi/openai`.
pub(crate) fn keychain_place(account: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{} item {SERVICE}/{account}", KeyStore::Keychain.label())
    } else {
        format!("{} ({BUNDLE_LABEL}: {account})", KeyStore::Keychain.label())
    }
}

/// The OS credential store. On macOS every key is in one Keychain item (see
/// [`bundle`]), so a program is asked for access once, not once per key; on
/// Windows (which doesn't ask) each key is its own Credential Manager item.
/// Every call goes through one lock, so the app's concurrent requests never
/// stack several prompts, and what's read is remembered for the process.
pub(crate) struct OsSecrets;

/// Keys kept together in one secret item. macOS asks before a program reads
/// a Keychain item it didn't create, and remembers "Always Allow" per item —
/// so one item for every key is one prompt for a new build of the app or the
/// CLI, not one per provider. The item's comment lists the names it holds
/// (attributes aren't encrypted), so whether a key exists is answered
/// without reading the secret. An item of its own from before (`Lexi/NAME`)
/// moves in the first time its key is read.
#[cfg(any(target_os = "macos", test))]
pub(crate) mod bundle {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, MutexGuard};

    use super::{Secrets, BUNDLE_ACCOUNT, BUNDLE_LABEL};

    /// The store underneath: the bundle item and the older one-key items.
    pub(crate) trait Raw {
        /// The names the bundle item's comment lists; `None` when there's no
        /// bundle item. Never reads the secret.
        fn bundle_names(&self) -> Result<Option<Vec<String>>, String>;
        /// The bundle item's secret (what macOS may ask about).
        fn read_bundle(&self) -> Result<Option<String>, String>;
        /// Write the bundle item (`exists`: update it rather than add it).
        fn write_bundle(&self, data: &str, comment: &str, exists: bool) -> Result<(), String>;
        /// Whether there's a one-key item for `account`, without reading it.
        fn single_exists(&self, account: &str) -> Result<bool, String>;
        fn read_single(&self, account: &str) -> Result<Option<String>, String>;
        fn delete_single(&self, account: &str) -> Result<bool, String>;
    }

    type Keys = BTreeMap<String, String>;

    pub(crate) struct Bundled<R> {
        raw: R,
        /// The bundle's keys once read.
        keys: Mutex<Option<Keys>>,
    }

    fn parse(data: Option<String>) -> Result<Keys, String> {
        match data {
            None => Ok(Keys::new()),
            Some(s) => serde_json::from_str(&s).map_err(|_| {
                format!("the Keychain item {BUNDLE_LABEL:?} isn't a list of keys Lexi can read")
            }),
        }
    }

    impl<R: Raw> Bundled<R> {
        pub(crate) const fn new(raw: R) -> Self {
            Bundled {
                raw,
                keys: Mutex::new(None),
            }
        }

        fn lock(&self) -> MutexGuard<'_, Option<Keys>> {
            self.keys.lock().unwrap_or_else(|p| p.into_inner())
        }

        fn listed(&self, account: &str) -> Result<bool, String> {
            Ok(self
                .raw
                .bundle_names()?
                .is_some_and(|names| names.iter().any(|n| n == account)))
        }

        /// The bundle's keys, read now if they haven't been (`fresh`: read
        /// again, so another program's changes aren't lost on a write).
        fn load<'a>(
            &self,
            keys: &'a mut Option<Keys>,
            fresh: bool,
        ) -> Result<&'a mut Keys, String> {
            if fresh || keys.is_none() {
                *keys = None;
                let read = match self.raw.bundle_names()? {
                    None => Keys::new(),
                    Some(_) => parse(self.raw.read_bundle()?)?,
                };
                *keys = Some(read);
            }
            Ok(keys.get_or_insert_with(Keys::new))
        }

        fn save(&self, keys: &mut Option<Keys>) -> Result<(), String> {
            let all = keys.clone().unwrap_or_default();
            let data = serde_json::to_string(&all).map_err(|e| e.to_string())?;
            let comment = all.keys().cloned().collect::<Vec<_>>().join("\n");
            let exists = self.raw.bundle_names()?.is_some();
            let written = self.raw.write_bundle(&data, &comment, exists);
            if written.is_err() {
                *keys = None; // not saved: read it again next time
            }
            written
        }

        /// A one-key item from before, unless the name is the bundle's own.
        fn single(&self, account: &str) -> Result<bool, String> {
            Ok(account != BUNDLE_ACCOUNT && self.raw.single_exists(account)?)
        }
    }

    impl<R: Raw> Secrets for Bundled<R> {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            let mut keys = self.lock();
            if let Some(v) = keys.as_ref().and_then(|k| k.get(account)) {
                return Ok(Some(v.clone()));
            }
            if self.listed(account)? {
                // Listed but not read yet (or added since by another program).
                let fresh = keys.is_some();
                if let Some(v) = self.load(&mut keys, fresh)?.get(account) {
                    return Ok(Some(v.clone()));
                }
            }
            if !self.single(account)? {
                return Ok(None);
            }
            let Some(key) = self
                .raw
                .read_single(account)?
                .filter(|v| !v.trim().is_empty())
            else {
                return Ok(None);
            };
            // Move it in with the others; if that fails it's still used.
            if let Ok(all) = self.load(&mut keys, false) {
                all.insert(account.to_string(), key.clone());
                if self.save(&mut keys).is_ok() {
                    let _ = self.raw.delete_single(account);
                }
            }
            Ok(Some(key))
        }

        fn exists(&self, account: &str) -> Result<bool, String> {
            let keys = self.lock();
            if keys.as_ref().is_some_and(|k| k.contains_key(account)) {
                return Ok(true);
            }
            Ok(self.listed(account)? || self.single(account)?)
        }

        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            let mut keys = self.lock();
            self.load(&mut keys, true)?
                .insert(account.to_string(), secret.to_string());
            self.save(&mut keys)?;
            if self.single(account).unwrap_or(false) {
                let _ = self.raw.delete_single(account);
            }
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<bool, String> {
            let mut keys = self.lock();
            let mut had = false;
            if self.listed(account)? {
                had = self.load(&mut keys, true)?.remove(account).is_some();
                self.save(&mut keys)?;
            } else if let Some(k) = keys.as_mut() {
                k.remove(account);
            }
            let single = self.single(account)? && self.raw.delete_single(account)?;
            Ok(had || single)
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use security_framework::item::{update_item, ItemClass, ItemSearchOptions, ItemUpdateOptions};
    use security_framework::passwords::{self, PasswordOptions};

    use super::{BUNDLE_ACCOUNT, BUNDLE_LABEL, SERVICE};

    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    /// The login Keychain, under a service (`Lexi`).
    pub(super) struct MacRaw(pub(super) &'static str);

    pub(super) const LEXI: MacRaw = MacRaw(SERVICE);

    fn search(service: &str, account: &str) -> ItemSearchOptions {
        let mut s = ItemSearchOptions::new();
        s.class(ItemClass::generic_password())
            .service(service)
            .account(account);
        s
    }

    /// An item's attributes (they aren't encrypted, so this never asks).
    fn attributes(
        service: &str,
        account: &str,
    ) -> Result<Option<std::collections::HashMap<String, String>>, String> {
        match search(service, account)
            .load_attributes(true)
            .limit(1)
            .search()
        {
            Ok(found) => Ok(found.first().map(|r| r.simplify_dict().unwrap_or_default())),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    impl super::bundle::Raw for MacRaw {
        fn bundle_names(&self) -> Result<Option<Vec<String>>, String> {
            Ok(attributes(self.0, BUNDLE_ACCOUNT)?.map(|attrs| {
                attrs
                    .get("icmt")
                    .map(|c| {
                        c.lines()
                            .filter(|l| !l.is_empty())
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default()
            }))
        }
        fn read_bundle(&self) -> Result<Option<String>, String> {
            self.read_single(BUNDLE_ACCOUNT)
        }
        fn write_bundle(&self, data: &str, comment: &str, exists: bool) -> Result<(), String> {
            if exists {
                // Updating keeps the item, and with it the programs you've allowed.
                passwords::set_generic_password(self.0, BUNDLE_ACCOUNT, data.as_bytes())
                    .map_err(|e| e.to_string())?;
                let mut update = ItemUpdateOptions::new();
                update.set_comment(comment);
                update_item(&search(self.0, BUNDLE_ACCOUNT), &update).map_err(|e| e.to_string())
            } else {
                let mut options = PasswordOptions::new_generic_password(self.0, BUNDLE_ACCOUNT);
                options.set_label(BUNDLE_LABEL);
                options.set_comment(comment);
                passwords::set_generic_password_options(data.as_bytes(), options)
                    .map_err(|e| e.to_string())
            }
        }
        fn single_exists(&self, account: &str) -> Result<bool, String> {
            Ok(attributes(self.0, account)?.is_some())
        }
        fn read_single(&self, account: &str) -> Result<Option<String>, String> {
            match passwords::generic_password(PasswordOptions::new_generic_password(
                self.0, account,
            )) {
                Ok(bytes) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| format!("the Keychain item {}/{account} isn't text", self.0)),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        }
        fn delete_single(&self, account: &str) -> Result<bool, String> {
            match passwords::delete_generic_password(self.0, account) {
                Ok(()) => Ok(true),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
                Err(e) => Err(e.to_string()),
            }
        }
    }

    /// The real login Keychain, under a throwaway service. Run by hand on a
    /// Mac: `cargo test -p lexi-host real_keychain -- --ignored`.
    #[test]
    #[ignore = "uses the login Keychain"]
    fn real_keychain_keeps_keys_in_one_item() {
        use super::bundle::{Bundled, Raw};
        use super::Secrets;
        const TEST: MacRaw = MacRaw("Lexi test (safe to delete)");
        let cleanup = || {
            for a in [BUNDLE_ACCOUNT, "one", "two", "old"] {
                let _ = TEST.delete_single(a);
            }
        };
        cleanup();
        passwords::set_generic_password(TEST.0, "old", b"old-key").unwrap();

        let store = Bundled::new(TEST);
        assert!(store.exists("old").unwrap());
        store.set("one", "k1").unwrap();
        store.set("two", "k2").unwrap();
        assert_eq!(store.get("old").unwrap().as_deref(), Some("old-key"));
        assert!(
            !TEST.single_exists("old").unwrap(),
            "moved into the one item"
        );

        let mut names = TEST.bundle_names().unwrap().unwrap();
        names.sort();
        assert_eq!(names, ["old", "one", "two"]);
        let fresh = Bundled::new(MacRaw(TEST.0));
        assert_eq!(fresh.get("two").unwrap().as_deref(), Some("k2"));
        assert!(fresh.delete("one").unwrap());
        assert_eq!(Bundled::new(MacRaw(TEST.0)).get("one").unwrap(), None);
        let attrs = attributes(TEST.0, BUNDLE_ACCOUNT).unwrap().unwrap();
        assert_eq!(attrs.get("labl").map(String::as_str), Some(BUNDLE_LABEL));
        cleanup();
    }
}

#[cfg(target_os = "macos")]
static MAC_KEYCHAIN: bundle::Bundled<mac::MacRaw> = bundle::Bundled::new(mac::LEXI);

#[cfg(target_os = "macos")]
impl Secrets for OsSecrets {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        MAC_KEYCHAIN.get(account)
    }
    fn exists(&self, account: &str) -> Result<bool, String> {
        MAC_KEYCHAIN.exists(account)
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        MAC_KEYCHAIN.set(account, secret)
    }
    fn delete(&self, account: &str) -> Result<bool, String> {
        MAC_KEYCHAIN.delete(account)
    }
}

#[cfg(target_os = "windows")]
static READ: std::sync::Mutex<Option<std::collections::HashMap<String, Option<String>>>> =
    std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
fn with_cache<T>(f: impl FnOnce(&mut std::collections::HashMap<String, Option<String>>) -> T) -> T {
    let mut guard = READ.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(Default::default))
}

#[cfg(target_os = "windows")]
impl Secrets for OsSecrets {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        with_cache(|cache| {
            if let Some(known) = cache.get(account) {
                return Ok(known.clone());
            }
            let entry = keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?;
            let read = match entry.get_password() {
                Ok(s) => Some(s),
                Err(keyring::Error::NoEntry) => None,
                Err(e) => return Err(e.to_string()),
            };
            cache.insert(account.to_string(), read.clone());
            Ok(read)
        })
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        with_cache(|cache| {
            keyring::Entry::new(SERVICE, account)
                .and_then(|e| e.set_password(secret))
                .map_err(|e| e.to_string())?;
            cache.insert(account.to_string(), Some(secret.to_string()));
            Ok(())
        })
    }
    fn delete(&self, account: &str) -> Result<bool, String> {
        with_cache(|cache| {
            let entry = keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?;
            let had = match entry.delete_credential() {
                Ok(()) => true,
                Err(keyring::Error::NoEntry) => false,
                Err(e) => return Err(e.to_string()),
            };
            cache.insert(account.to_string(), None);
            Ok(had)
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Secrets for OsSecrets {
    fn get(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Err("this platform has no supported credential store".into())
    }
    fn delete(&self, _: &str) -> Result<bool, String> {
        Ok(false)
    }
}

/// The OS keychain, or `None` where there isn't one (or `LEXI_KEY_STORE=file`).
pub(crate) fn current_secrets() -> Option<&'static dyn Secrets> {
    match KeyStore::current() {
        KeyStore::Keychain => Some(&OsSecrets),
        KeyStore::File => None,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// An in-memory secret store; `fail` makes every call error. `reads`
    /// counts secret reads (what makes macOS ask for access).
    #[derive(Default)]
    pub(crate) struct MemSecrets {
        pub(crate) items: RefCell<HashMap<String, String>>,
        pub(crate) fail: bool,
        pub(crate) reads: std::cell::Cell<usize>,
    }

    impl super::Secrets for MemSecrets {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            if self.fail {
                return Err("keychain locked".into());
            }
            self.reads.set(self.reads.get() + 1);
            Ok(self.items.borrow().get(account).cloned())
        }
        fn exists(&self, account: &str) -> Result<bool, String> {
            if self.fail {
                return Err("keychain locked".into());
            }
            Ok(self.items.borrow().contains_key(account))
        }
        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            if self.fail {
                return Err("keychain locked".into());
            }
            self.items
                .borrow_mut()
                .insert(account.to_string(), secret.to_string());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<bool, String> {
            if self.fail {
                return Err("keychain locked".into());
            }
            Ok(self.items.borrow_mut().remove(account).is_some())
        }
    }
}

#[cfg(test)]
mod bundle_tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    use super::bundle::{Bundled, Raw};
    use super::{Secrets, BUNDLE_ACCOUNT};

    /// The Keychain in memory: items by account, each with a secret and a
    /// comment. `reads` counts secret reads — what macOS asks about.
    #[derive(Default)]
    struct MemRaw {
        items: RefCell<BTreeMap<String, (String, String)>>,
        reads: Cell<usize>,
        refuse_reads: Cell<bool>,
    }

    impl MemRaw {
        fn with_singles(keys: &[(&str, &str)]) -> MemRaw {
            let raw = MemRaw::default();
            for (a, k) in keys {
                raw.items
                    .borrow_mut()
                    .insert(a.to_string(), (k.to_string(), String::new()));
            }
            raw
        }
        fn accounts(&self) -> Vec<String> {
            self.items.borrow().keys().cloned().collect()
        }
        fn comment(&self) -> String {
            self.items.borrow()[BUNDLE_ACCOUNT].1.clone()
        }
    }

    impl Raw for &MemRaw {
        fn bundle_names(&self) -> Result<Option<Vec<String>>, String> {
            Ok(self
                .items
                .borrow()
                .get(BUNDLE_ACCOUNT)
                .map(|(_, c)| c.lines().map(String::from).collect()))
        }
        fn read_bundle(&self) -> Result<Option<String>, String> {
            self.read_single(BUNDLE_ACCOUNT)
        }
        fn write_bundle(&self, data: &str, comment: &str, exists: bool) -> Result<(), String> {
            assert_eq!(exists, self.items.borrow().contains_key(BUNDLE_ACCOUNT));
            self.items.borrow_mut().insert(
                BUNDLE_ACCOUNT.to_string(),
                (data.to_string(), comment.to_string()),
            );
            Ok(())
        }
        fn single_exists(&self, account: &str) -> Result<bool, String> {
            Ok(self.items.borrow().contains_key(account))
        }
        fn read_single(&self, account: &str) -> Result<Option<String>, String> {
            if self.refuse_reads.get() {
                return Err("access denied".into());
            }
            self.reads.set(self.reads.get() + 1);
            Ok(self.items.borrow().get(account).map(|(s, _)| s.clone()))
        }
        fn delete_single(&self, account: &str) -> Result<bool, String> {
            Ok(self.items.borrow_mut().remove(account).is_some())
        }
    }

    #[test]
    fn older_one_key_items_move_into_the_one_item_as_they_are_read() {
        let raw = MemRaw::with_singles(&[("openai", "sk-o"), ("anthropic", "sk-a")]);
        let store = Bundled::new(&raw);
        // Checking for keys never reads one.
        assert!(store.exists("openai").unwrap());
        assert!(!store.exists("nvidia").unwrap());
        assert_eq!(raw.reads.get(), 0);

        assert_eq!(store.get("openai").unwrap().as_deref(), Some("sk-o"));
        assert_eq!(store.get("anthropic").unwrap().as_deref(), Some("sk-a"));
        assert_eq!(store.get("nvidia").unwrap(), None);
        assert_eq!(raw.accounts(), vec![BUNDLE_ACCOUNT.to_string()]);
        assert_eq!(raw.comment(), "anthropic\nopenai");

        // A new build (a fresh process) reads the one item once for every key.
        raw.reads.set(0);
        let next = Bundled::new(&raw);
        assert!(next.exists("anthropic").unwrap());
        assert_eq!(raw.reads.get(), 0);
        assert_eq!(next.get("openai").unwrap().as_deref(), Some("sk-o"));
        assert_eq!(next.get("anthropic").unwrap().as_deref(), Some("sk-a"));
        assert_eq!(next.get("nvidia").unwrap(), None);
        assert_eq!(raw.reads.get(), 1);
    }

    #[test]
    fn saving_and_removing_keep_the_other_keys() {
        let raw = MemRaw::default();
        let store = Bundled::new(&raw);
        store.set("openai", "sk-o").unwrap();
        store.set("nvidia", "nv").unwrap();
        store.set("openai", "sk-o2").unwrap();
        assert_eq!(raw.comment(), "nvidia\nopenai");

        assert!(store.delete("openai").unwrap());
        assert!(!store.delete("openai").unwrap());
        assert_eq!(raw.comment(), "nvidia");
        let next = Bundled::new(&raw);
        assert_eq!(next.get("openai").unwrap(), None);
        assert_eq!(next.get("nvidia").unwrap().as_deref(), Some("nv"));

        // Saving a key that had an item of its own removes that item.
        raw.items
            .borrow_mut()
            .insert("anthropic".into(), ("old".into(), String::new()));
        next.set("anthropic", "new").unwrap();
        assert_eq!(raw.accounts(), vec![BUNDLE_ACCOUNT.to_string()]);
        assert_eq!(next.get("anthropic").unwrap().as_deref(), Some("new"));
    }

    #[test]
    fn a_key_another_program_added_is_found_and_a_refused_read_is_an_error() {
        let raw = MemRaw::default();
        let app = Bundled::new(&raw);
        let cli = Bundled::new(&raw);
        app.set("openai", "sk-o").unwrap();
        assert_eq!(cli.get("openai").unwrap().as_deref(), Some("sk-o"));
        cli.set("nvidia", "nv").unwrap();
        // The app has the item read already; the new name makes it read again.
        assert_eq!(app.get("nvidia").unwrap().as_deref(), Some("nv"));
        assert_eq!(app.get("openai").unwrap().as_deref(), Some("sk-o"));

        let fresh = Bundled::new(&raw);
        raw.refuse_reads.set(true);
        assert!(fresh.exists("openai").unwrap());
        assert_eq!(fresh.get("openai").unwrap_err(), "access denied");
    }

    #[test]
    fn a_name_like_the_items_own_is_just_a_key() {
        let raw = MemRaw::default();
        let store = Bundled::new(&raw);
        assert_eq!(store.get(BUNDLE_ACCOUNT).unwrap(), None);
        store.set(BUNDLE_ACCOUNT, "k").unwrap();
        assert_eq!(
            Bundled::new(&raw).get(BUNDLE_ACCOUNT).unwrap().as_deref(),
            Some("k")
        );
        assert!(store.delete(BUNDLE_ACCOUNT).unwrap());
        assert_eq!(raw.accounts(), vec![BUNDLE_ACCOUNT.to_string()]);
    }
}
