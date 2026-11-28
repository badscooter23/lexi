//! Config, API-definition, and API-key loading.
//!
//! Reads `~/.lexi-cli/` (overridable via `LEXI_HOME`), shared with the Python
//! CLI. The API definitions are embedded in the binary, with a per-user
//! override under `~/.lexi-cli/apis/`.

use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value};

use crate::key_store::{self, Slot, Store, KEY_STORE_SETTING};
use crate::keystore::{self, Lookup, LookupOpts, Source};
use crate::HostError;

/// A JSON object map (shorthand used by the config writers).
type Map = JsonMap<String, Value>;

/// API definitions embedded at build time (the same files lexi-core bundles).
const EMBEDDED_APIS: &[(&str, &str)] = &[
    (
        "messages",
        include_str!("../../lexi-core/apis/messages.yaml"),
    ),
    (
        "chat-completions",
        include_str!("../../lexi-core/apis/chat-completions.yaml"),
    ),
    (
        "responses",
        include_str!("../../lexi-core/apis/responses.yaml"),
    ),
    (
        "image-generations",
        include_str!("../../lexi-core/apis/image-generations.yaml"),
    ),
    (
        "embeddings",
        include_str!("../../lexi-core/apis/embeddings.yaml"),
    ),
    (
        "nvidia-genai-image",
        include_str!("../../lexi-core/apis/nvidia-genai-image.yaml"),
    ),
];

/// Provider templates embedded at build time (`lexi-core/providers/`). A
/// provider entry with `template: <name>` is the template merged with the entry.
const EMBEDDED_TEMPLATES: &[(&str, &str)] = &[
    (
        "openai",
        include_str!("../../lexi-core/providers/openai.yaml"),
    ),
    (
        "anthropic",
        include_str!("../../lexi-core/providers/anthropic.yaml"),
    ),
    (
        "nvidia",
        include_str!("../../lexi-core/providers/nvidia.yaml"),
    ),
];

/// The names of the shipped provider templates.
pub fn template_names() -> Vec<&'static str> {
    EMBEDDED_TEMPLATES.iter().map(|(n, _)| *n).collect()
}

/// A shipped provider template.
pub fn provider_template(name: &str) -> Result<Value, HostError> {
    let (_, text) = EMBEDDED_TEMPLATES
        .iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| {
            HostError::Config(format!(
                "unknown provider template {name:?} (shipped: {})",
                template_names().join(", ")
            ))
        })?;
    Ok(serde_yaml::from_str(text)?)
}

/// A provider entry with its `template:` (if any) applied.
fn resolve_template(entry: &Value) -> Result<Value, HostError> {
    match entry.get("template").and_then(Value::as_str) {
        Some(name) => {
            let mut merged = lexi_core::merge_provider_template(&provider_template(name)?, entry);
            merged["template"] = Value::from(name); // keep where it came from
                                                    // An entry's api_key list replaces the template's (never merged).
            if let Some(k) = entry.get("api_key") {
                merged["api_key"] = k.clone();
            }
            Ok(merged)
        }
        None => Ok(entry.clone()),
    }
}

/// The Lexi config dir: `$LEXI_HOME`, else `~/.lexi-cli`.
pub fn lexi_home() -> PathBuf {
    if let Ok(h) = std::env::var("LEXI_HOME") {
        return PathBuf::from(h);
    }
    dirs::home_dir().unwrap_or_default().join(".lexi-cli")
}

/// The names of the API definitions shipped (embedded) in the binary.
pub fn embedded_api_names() -> Vec<&'static str> {
    EMBEDDED_APIS.iter().map(|(n, _)| *n).collect()
}

/// The raw YAML text of a shipped API definition, if `name` is one.
fn embedded_api_text(name: &str) -> Option<&'static str> {
    EMBEDDED_APIS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

/// Load an API definition by name.
///
/// Shipped definitions always use the current **embedded** copy (so a stale
/// `~/.lexi-cli/apis/` seeded by an older CLI can't mask new fields); the user
/// dir supplies only *additional* custom definitions.
pub fn load_api_def(name: &str) -> Result<Value, HostError> {
    if let Some((_, text)) = EMBEDDED_APIS.iter().find(|(n, _)| *n == name) {
        return Ok(serde_yaml::from_str(text)?);
    }
    let user = lexi_home().join("apis").join(format!("{name}.yaml"));
    if user.exists() {
        return Ok(serde_yaml::from_str(&std::fs::read_to_string(&user)?)?);
    }
    Err(HostError::UnknownApi(name.to_string()))
}

/// Load `providers-config.yaml`, with each entry's `template:` applied (the
/// file itself is read and written raw by the provider writers below).
pub fn load_providers() -> Result<Value, HostError> {
    let path = lexi_home().join("providers").join("providers-config.yaml");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| HostError::Config(format!("no providers-config at {}", path.display())))?;
    let mut doc: Value = serde_yaml::from_str(&text)?;
    if let Some(providers) = doc.get_mut("providers").and_then(Value::as_object_mut) {
        for entry in providers.values_mut() {
            *entry = resolve_template(entry)?;
        }
    }
    Ok(doc)
}

/// Deep-merge `overlay` over `base`: dicts recurse, lists/scalars are replaced
/// (the provider-override semantics from the spec).
pub fn deep_merge(base: &Value, overlay: &Value) -> Value {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, v) in o {
                let merged = match out.get(k) {
                    Some(existing) => deep_merge(existing, v),
                    None => v.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        _ => overlay.clone(),
    }
}

/// Effective config for the provider's catch-all binding (its first binding
/// without `for:`) — the API definition with the provider entry merged over it
/// (provider wins). See [`binding_config`] for a specific binding.
pub fn effective_config(provider_name: &str, provider_cfg: &Value) -> Result<Value, HostError> {
    let bindings = lexi_core::provider_bindings(provider_cfg);
    let Some(binding) = bindings
        .iter()
        .find(|b| b.get("for").is_none())
        .or(bindings.first())
    else {
        // No API reference: the provider may define its API inline.
        if provider_cfg.get("request").is_some_and(Value::is_object) {
            return Ok(provider_cfg.clone());
        }
        return Err(HostError::MissingApi(provider_name.to_string()));
    };
    binding_config(provider_cfg, binding)
}

/// Provider keys that describe the provider (its APIs and models, how to get a
/// key) rather than configure a request; not merged into an effective config.
const CLASSIFICATION_KEYS: &[&str] = &["apis", "api", "tag_rules", "template", "replace", "setup"];

/// Effective config for one binding: its API definition, then the provider
/// entry, then the binding's own keys (`url`, `auth`, `request`, `response`,
/// `default_model`, …).
pub fn binding_config(provider_cfg: &Value, binding: &Value) -> Result<Value, HostError> {
    let api_name = binding.get("api").and_then(Value::as_str).unwrap_or("");
    let api_def = load_api_def(api_name)?;
    let mut provider = provider_cfg.clone();
    if let Some(p) = provider.as_object_mut() {
        for k in CLASSIFICATION_KEYS {
            p.shift_remove(*k);
        }
    }
    let mut over = binding.clone();
    if let Some(b) = over.as_object_mut() {
        b.shift_remove("for");
    }
    let mut eff = deep_merge(&deep_merge(&api_def, &provider), &over);
    eff["api"] = Value::from(api_name);
    // An API's own api_key replaces the provider's (never merged), and its
    // saved key is the API's own.
    if let Some(k) = binding.get("api_key") {
        eff["api_key"] = k.clone();
        eff[KEY_SCOPE] = serde_json::json!({ "api": api_name });
    }
    Ok(eff)
}

/// The `api_key` entry that marks where the saved key is looked in.
pub const SAVED_SOURCE: &str = keystore::SAVED;

/// In an effective config: whose `api_key` it carries, when it isn't the
/// provider's — `{api: NAME}` (an API's own, with its own saved key),
/// `{model: FILE}` (a configured model's own, listed in FILE; no saved key)
/// or `{key: NAME}` (one of the provider's named keys).
pub(crate) const KEY_SCOPE: &str = "_key_scope";

/// The name that means a provider's unnamed key (`openai#default`).
pub const DEFAULT_KEY: &str = "default";

tokio::task_local! {
    /// The named key chosen for one operation by [`with_key`].
    static KEY_CHOICE: Option<String>;
}

/// Run `fut` with `key` as the provider key its requests use (`--key work`,
/// Python `key="work"`, the `provider#key` shorthand): a provider's named
/// key, or `default` for its unnamed one; `None` changes nothing. Every key
/// lookup `fut` makes is covered — requests, uploads, listings, job checks
/// (`docs/key-storage-design.md` §11.4).
pub async fn with_key<F: std::future::Future>(key: Option<String>, fut: F) -> F::Output {
    KEY_CHOICE.scope(key, fut).await
}

/// [`with_key`] for code that doesn't await (preparing a request).
pub fn with_key_sync<R>(key: Option<String>, f: impl FnOnce() -> R) -> R {
    KEY_CHOICE.sync_scope(key, f)
}

fn chosen_key() -> Option<String> {
    KEY_CHOICE.try_with(|k| k.clone()).ok().flatten()
}

/// A named key's name: letters, digits, `-` and `_`, at most 32.
fn check_key_name(name: &str) -> Result<(), HostError> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(HostError::Invalid(format!(
            "invalid key name {name:?}: use letters, digits, - and _ (at most 32)"
        )))
    }
}

/// A provider argument and a key: `openai#work` is `openai` with the key
/// `work`; `key` (`--key`, `key=`) may name it too, but not differently.
/// Returns the provider and the key, if any (`default` included).
pub fn split_key_arg(
    provider: &str,
    key: Option<&str>,
) -> Result<(String, Option<String>), HostError> {
    let (name, inline) = match provider.split_once('#') {
        Some((p, k)) => (p, Some(k)),
        None => (provider, None),
    };
    for k in inline.iter().chain(key.iter()) {
        check_key_name(k)?;
    }
    match (inline, key) {
        (Some(a), Some(b)) if a != b => Err(HostError::Invalid(format!(
            "{provider} and the key {b:?} name different keys: give just one"
        ))),
        (a, b) => Ok((name.to_string(), a.or(b).map(String::from))),
    }
}

/// The names of the provider's named keys in `cfg` (a provider entry or an
/// effective config).
fn named_key_names(cfg: &Value) -> Vec<String> {
    cfg.get("keys")
        .and_then(Value::as_object)
        .map(|k| k.keys().cloned().collect())
        .unwrap_or_default()
}

fn unknown_key(provider: &str, name: &str, cfg: &Value) -> HostError {
    let mut names = vec![DEFAULT_KEY.to_string()];
    names.extend(named_key_names(cfg));
    HostError::Invalid(format!(
        "{provider} has no key named {name:?} (its keys: {})",
        names.join(", ")
    ))
}

/// `cfg` with the key this operation uses (§11.4): the one chosen with
/// [`with_key`] — its sources replace any other `api_key`, for every API —
/// else, unless an API's or model's own `api_key` applies, the provider's
/// `default_key`; else `cfg` as it is (the unnamed key).
fn choose_key<'a>(
    provider: &str,
    cfg: &'a Value,
) -> Result<std::borrow::Cow<'a, Value>, HostError> {
    use std::borrow::Cow;
    let own = cfg
        .get(KEY_SCOPE)
        .is_some_and(|s| s.get("api").is_some() || s.get("model").is_some());
    let name = match chosen_key() {
        Some(k) if k == DEFAULT_KEY => return Ok(Cow::Borrowed(cfg)),
        Some(k) => k,
        None => match cfg.get("default_key").and_then(Value::as_str) {
            Some(d) if !own && d != DEFAULT_KEY => d.to_string(),
            _ => return Ok(Cow::Borrowed(cfg)),
        },
    };
    let Some(entry) = cfg.get("keys").and_then(|k| k.get(&name)) else {
        return Err(unknown_key(provider, &name, cfg));
    };
    let mut out = cfg.clone();
    out["api_key"] = entry
        .get("api_key")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    out[KEY_SCOPE] = serde_json::json!({ "key": name });
    Ok(Cow::Owned(out))
}

/// For `cfg` (a provider entry or an effective config): its saved key (none
/// for a model's own `api_key`), and the config files its `api_key` list
/// comes from.
fn key_context(home: &Path, provider: &str, cfg: &Value) -> (Option<Slot>, Vec<PathBuf>) {
    let mut guard = vec![providers_config_path(home)];
    let scope = cfg.get(KEY_SCOPE);
    if let Some(file) = scope.and_then(|s| s.get("model")).and_then(Value::as_str) {
        guard.push(home.join(file));
        return (None, guard);
    }
    if let Some(k) = scope.and_then(|s| s.get("key")).and_then(Value::as_str) {
        return (Some(Slot::named(provider, k)), guard);
    }
    let api = scope.and_then(|s| s.get("api")).and_then(Value::as_str);
    (Some(Slot::new(provider, api)), guard)
}

/// The `api_key` sources in `cfg` — a provider entry, or a binding's or
/// model's effective config (whose own `api_key` replaces the provider's) — in
/// the order they're tried.
fn key_sources(cfg: &Value) -> Result<Vec<Source>, HostError> {
    keystore::parse_sources(cfg.get("api_key")).map_err(HostError::Config)
}

/// Look for the key `cfg`'s `api_key` sources (then its saved key) lead
/// to; `run_commands` false leaves `command:` sources unrun (a listing).
pub(crate) fn lookup_api_key(
    provider: &str,
    cfg: &Value,
    run_commands: bool,
) -> Result<Lookup, HostError> {
    lookup_api_key_in(&lexi_home(), provider, cfg, run_commands)
}

fn lookup_api_key_in(
    home: &Path,
    provider: &str,
    cfg: &Value,
    run_commands: bool,
) -> Result<Lookup, HostError> {
    let cfg = choose_key(provider, cfg)?;
    let cfg = cfg.as_ref();
    let (slot, guard) = key_context(home, provider, cfg);
    let listed = key_sources(cfg)?;
    let store = Store::open(home)?;
    let sources =
        keystore::with_saved(&listed, slot.as_ref().map(|s| store.location(s)).as_deref());
    Ok(keystore::lookup_with(
        home,
        &sources,
        &LookupOpts {
            run_commands,
            guard,
            saved: slot.map(|s| (store, s)),
        },
    ))
}

/// Resolve an API key from `cfg`'s `api_key` sources, then its saved key (a
/// provider entry, or a binding's/model's effective config — pass the
/// effective config when there is one, so an API's or model's own `api_key`
/// applies). The first source with a key wins; a read error (an unreadable
/// file, a failing command) stops the lookup and is the error.
pub fn resolve_api_key(provider_name: &str, provider_cfg: &Value) -> Result<String, HostError> {
    let lookup = lookup_api_key(provider_name, provider_cfg, true)?;
    if let Some(key) = lookup.key.clone() {
        return Ok(key);
    }
    if let Some((source, err)) = lookup.failure() {
        return Err(read_error(provider_name, source, err));
    }
    Err(HostError::MissingApiKey(if lookup.checked.is_empty() {
        format!("no API key for provider {provider_name:?}: its api_key names no source")
    } else {
        format!(
            "no API key for provider {provider_name:?} (looked in {})",
            lookup.summary()
        )
    }))
}

fn read_error(provider_name: &str, source: &Source, err: &str) -> HostError {
    HostError::Config(format!(
        "couldn't read {provider_name}'s API key from {}: {err}",
        source.describe()
    ))
}

/// Which kind of source the key comes from — `"env"`, `"file"`,
/// `"command"`, `"saved"`, `"config"` (a literal) — without revealing it;
/// `None` when no source has one. Same order as [`resolve_api_key`] (a read
/// error is an error), but commands aren't run: one that would be is the
/// answer (so listing providers never sets off a password manager).
pub fn api_key_source(
    provider_name: &str,
    provider_cfg: &Value,
) -> Result<Option<&'static str>, HostError> {
    let lookup = lookup_api_key(provider_name, provider_cfg, false)?;
    if let Some((source, err)) = lookup.failure() {
        return Err(read_error(provider_name, source, err));
    }
    Ok(lookup.source().map(Source::kind))
}

/// Where a saved key went (`set_api_key`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedKey {
    /// Where it's kept: the OS store's name (`the macOS Keychain`, `Windows
    /// Credential Manager`, `your desktop keyring`), or its key file (under
    /// `$LEXI_HOME`).
    pub location: String,
    /// The kind of store: `keychain` or `file`.
    pub store: String,
    /// Whether providers-config.yaml changed: an API that used the
    /// provider's key got its own `api_key`, or a key written in the config
    /// was taken out.
    pub added: bool,
    /// Where the key is now read from — `location` when it's the saved key,
    /// else an earlier source that has one (e.g. a set environment variable).
    pub used: Option<String>,
}

impl SavedKey {
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "location": self.location,
            "store": self.store,
            "added": self.added,
            "used": self.used,
        })
    }
}

/// The `api_key` list a key operation works on: the provider's, (with
/// `api`) that API's own, or one of its named keys' (`provider#key`), as
/// listed (without the saved key unless it's marked). `own` is false when
/// the API has none and uses the provider's.
struct KeyScope {
    doc: Map,
    entry: Map,
    provider: String,
    api: Option<String>,
    key: Option<String>,
    sources: Vec<Source>,
    own: bool,
}

impl KeyScope {
    /// The saved key the scope's sources lead to.
    fn slot(&self) -> Slot {
        match (&self.key, self.own) {
            (Some(k), _) => Slot::named(&self.provider, k),
            (None, true) => Slot::new(&self.provider, self.api.as_deref()),
            (None, false) => Slot::new(&self.provider, None),
        }
    }
}

/// The scope `provider` (which may be `provider#key`) and `api` name. A named
/// key that isn't there yet is an error unless `create` (saving a key
/// creates one).
fn key_scope(
    home: &Path,
    provider: &str,
    api: Option<&str>,
    create: bool,
) -> Result<KeyScope, HostError> {
    let (provider, key) = split_key_arg(provider, None)?;
    let key = key.filter(|k| k != DEFAULT_KEY);
    check_provider_name(&provider)?;
    if key.is_some() && api.is_some() {
        return Err(HostError::Invalid(
            "a named key covers every API of the provider: leave out the API".into(),
        ));
    }
    let mut doc = read_providers_doc(home)?;
    let entry = raw_entry(&mut doc, &provider)?;
    let resolved = resolve_template(&Value::Object(entry.clone()))?;
    let (sources, own) = match (&key, api) {
        (Some(k), _) => match resolved.get("keys").and_then(|ks| ks.get(k)) {
            Some(named) => (key_sources(named)?, true),
            None if create => (Vec::new(), true),
            None => return Err(unknown_key(&provider, k, &resolved)),
        },
        (None, None) => (key_sources(&resolved)?, true),
        (None, Some(a)) => {
            let bindings = lexi_core::provider_bindings(&resolved);
            let binding = bindings.iter().find(|b| b["api"] == a).ok_or_else(|| {
                HostError::Invalid(format!(
                    "provider {provider:?} has no {a:?} API (it has: {})",
                    bindings
                        .iter()
                        .filter_map(|b| b["api"].as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            match binding.get("api_key") {
                Some(_) => (key_sources(binding)?, true),
                None => (Vec::new(), false),
            }
        }
    };
    Ok(KeyScope {
        doc,
        entry,
        provider,
        api: api.map(String::from),
        key,
        sources,
        own,
    })
}

/// The key file Lexi saves (or once saved) `provider`'s key in (or its
/// `api`'s own key).
fn key_file(provider: &str, api: Option<&str>) -> String {
    Slot::new(provider, api).file()
}

/// Write `scope`'s list back into providers-config.yaml (a named key that
/// wasn't there is added).
fn write_scope_sources(home: &Path, scope: KeyScope, sources: &[Source]) -> Result<(), HostError> {
    let provider = scope.provider.clone();
    match (scope.key, scope.api) {
        (Some(k), _) => {
            let (mut doc, mut entry) = (scope.doc, scope.entry);
            let keys = entry
                .entry("keys")
                .or_insert_with(|| Value::Object(Map::new()));
            if !keys.is_object() {
                *keys = Value::Object(Map::new());
            }
            let named = keys
                .as_object_mut()
                .expect("an object")
                .entry(k)
                .or_insert_with(|| Value::Object(Map::new()));
            if !named.is_object() {
                *named = Value::Object(Map::new());
            }
            let named = named.as_object_mut().expect("an object");
            if sources.is_empty() {
                named.shift_remove("api_key");
            } else {
                named.insert("api_key".into(), keystore::sources_value(sources));
            }
            providers_mut(&mut doc).insert(provider, Value::Object(entry));
            write_yaml(&providers_config_path(home), &Value::Object(doc))
        }
        (None, None) => {
            let (mut doc, mut entry) = (scope.doc, scope.entry);
            entry.insert("api_key".into(), keystore::sources_value(sources));
            providers_mut(&mut doc).insert(provider, Value::Object(entry));
            write_yaml(&providers_config_path(home), &Value::Object(doc))
        }
        (None, Some(api)) => {
            let mut b = Map::new();
            b.insert("api".into(), Value::from(api));
            b.insert("api_key".into(), keystore::sources_value(sources));
            add_provider_api_in(home, &provider, &b).map(|_| ())
        }
    }
}

/// The sources a scope's key is looked for in — the API's own (and its saved
/// key), else the provider's; a named key's own — with the saved key they
/// lead to and whether they're the scope's own.
struct ScopeSources {
    sources: Vec<Source>,
    slot: Slot,
    own: bool,
}

fn scope_sources(
    home: &Path,
    store: &Store,
    provider: &str,
    api: Option<&str>,
) -> Result<ScopeSources, HostError> {
    let scope = key_scope(home, provider, api, false)?;
    let slot = scope.slot();
    let own = scope.own;
    let listed = if own {
        scope.sources
    } else {
        key_scope(home, &scope.provider, None, false)?.sources
    };
    Ok(ScopeSources {
        sources: keystore::with_saved(&listed, Some(&store.location(&slot))),
        slot,
        own,
    })
}

fn scope_lookup(home: &Path, store: &Store, scope: &ScopeSources, run_commands: bool) -> Lookup {
    keystore::lookup_with(
        home,
        &scope.sources,
        &LookupOpts {
            run_commands,
            guard: vec![providers_config_path(home)],
            saved: Some((store.clone(), scope.slot.clone())),
        },
    )
}

/// Where a scope's key is read from now: where the saved key is kept when
/// it's the one, else the source's description (a command isn't run, so one
/// that would be is the answer).
fn used_source(home: &Path, store: &Store, provider: &str, api: Option<&str>) -> Option<String> {
    let scope = scope_sources(home, store, provider, api).ok()?;
    match scope_lookup(home, store, &scope, false).source()? {
        Source::Saved(place) => Some(place.clone()),
        other => Some(other.describe()),
    }
}

/// What a key operation on `provider` (or `provider#key`, or its `api`) sees:
/// the sources that apply (the API's own, else the provider's, with the
/// saved key in its place), whether they're the scope's own, where a saved
/// key goes (and the kind of store), and what looking through the sources
/// found (commands run only with `run_commands`).
pub(crate) struct KeyPlan {
    pub(crate) sources: Vec<Source>,
    pub(crate) own: bool,
    pub(crate) save_to: String,
    pub(crate) store: &'static str,
    pub(crate) lookup: Lookup,
}

pub(crate) fn key_plan(
    provider: &str,
    api: Option<&str>,
    run_commands: bool,
) -> Result<KeyPlan, HostError> {
    let home = lexi_home();
    let store = Store::open(&home)?;
    let scope = scope_sources(&home, &store, provider, api)?;
    let lookup = scope_lookup(&home, &store, &scope, run_commands);
    let (name, key) = split_key_arg(provider, None)?;
    let save_slot = match key.as_deref().filter(|k| *k != DEFAULT_KEY) {
        Some(k) => Slot::named(&name, k),
        None => Slot::new(&name, api),
    };
    Ok(KeyPlan {
        save_to: store.location(&save_slot),
        store: store.kind(),
        sources: scope.sources,
        own: scope.own,
        lookup,
    })
}

/// Save an API key for `provider` — with `api`, a key for that API only; as
/// `provider#key`, one of its named keys (created if it isn't there yet) —
/// as its saved key: in the OS key store, else an owner-only file,
/// `providers/<name>/api-key` (`api-key-<api>`, `keys/<key>`) — one copy, in
/// one place (`key_store` in config.json). The sources `api_key` lists are
/// never written. providers-config.yaml changes only when an API that used
/// the provider's key gets its own (`api_key: [saved]`), a named key is
/// added, or to take out a key written in the config itself.
pub fn set_api_key(
    provider_name: &str,
    api: Option<&str>,
    key: &str,
) -> Result<SavedKey, HostError> {
    set_api_key_in(&lexi_home(), provider_name, api, key)
}

fn set_api_key_in(
    home: &Path,
    provider_name: &str,
    api: Option<&str>,
    key: &str,
) -> Result<SavedKey, HostError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(HostError::Config("API key cannot be empty".into()));
    }
    let scope = key_scope(home, provider_name, api, true)?;
    let new_key = scope.key.as_ref().is_some_and(|k| {
        resolve_template(&Value::Object(scope.entry.clone()))
            .ok()
            .and_then(|r| r.get("keys").and_then(|ks| ks.get(k)).cloned())
            .is_none()
    });
    let store = Store::open(home)?;
    let slot = if scope.key.is_some() {
        scope.slot()
    } else {
        Slot::new(&scope.provider, api)
    };
    store.write(&slot, key)?;
    keystore::forget_commands();
    let has_literal = scope
        .sources
        .iter()
        .any(|s| matches!(s, Source::Literal(_)));
    let added = has_literal || !scope.own || new_key;
    if added {
        let list: Vec<Source> = if scope.own {
            scope
                .sources
                .iter()
                .filter(|s| !matches!(s, Source::Literal(_)))
                .cloned()
                .collect()
        } else {
            vec![Source::Saved(String::new())]
        };
        write_scope_sources(home, scope, &list)?;
    }
    Ok(SavedKey {
        used: used_source(home, &store, provider_name, api),
        location: store.location(&slot),
        store: store.kind().to_string(),
        added,
    })
}

fn uses_provider_key(provider: &str, api: &str) -> HostError {
    HostError::Invalid(format!(
        "the {api} API has no api_key of its own; it uses {provider}'s key (leave out the API)"
    ))
}

/// Delete `provider`'s (its `api`'s, or `provider#key`'s) saved key (from
/// the OS store and any key file). The sources `api_key` lists (yours) and
/// providers-config.yaml are left as they are. Returns whether there was
/// one.
pub fn remove_api_key(provider_name: &str, api: Option<&str>) -> Result<bool, HostError> {
    remove_api_key_in(&lexi_home(), provider_name, api)
}

fn remove_api_key_in(
    home: &Path,
    provider_name: &str,
    api: Option<&str>,
) -> Result<bool, HostError> {
    let scope = key_scope(home, provider_name, api, false)?;
    if let (Some(a), false) = (api, scope.own) {
        return Err(uses_provider_key(&scope.provider, a));
    }
    keystore::forget_commands();
    Store::open(home)?.delete(&scope.slot())
}

/// The sources `provider`'s (its `api`'s, or `provider#key`'s) key is looked
/// for in, in order: `{provider, api, key_name, own, sources: [{kind, from,
/// value, implicit?}]}` — `value` as written in providers-config.yaml; the
/// saved key is `implicit` when the list doesn't mark its place.
pub fn key_source_list(provider: &str, api: Option<&str>) -> Result<Value, HostError> {
    key_source_list_in(&lexi_home(), provider, api)
}

fn key_source_list_in(home: &Path, provider: &str, api: Option<&str>) -> Result<Value, HostError> {
    let scope = key_scope(home, provider, api, false)?;
    let listed = if scope.own {
        scope.sources.clone()
    } else {
        key_scope(home, &scope.provider, None, false)?.sources
    };
    let marked = listed.iter().any(|s| matches!(s, Source::Saved(_)));
    let saved_at = Store::open(home)?.location(&scope.slot());
    let rows: Vec<Value> = keystore::with_saved(&listed, Some(&saved_at))
        .iter()
        .map(|s| {
            let mut row = serde_json::json!({
                "kind": s.kind(),
                "from": s.describe(),
                "value": s.to_value(),
            });
            if matches!(s, Source::Saved(_)) && !marked {
                row["implicit"] = Value::Bool(true);
            }
            row
        })
        .collect();
    Ok(serde_json::json!({
        "provider": scope.provider,
        "api": api,
        "key_name": scope.key,
        "own": scope.own,
        "sources": rows,
    }))
}

/// An `api_key` entry to add or remove, checked: `{env: VAR}`, `{file:
/// PATH}`, `{command: [...]}` or `saved` (never a key itself).
fn source_arg(source: &Value) -> Result<Source, HostError> {
    match keystore::Source::parse(source).map_err(HostError::Invalid)? {
        None => Err(HostError::Invalid("no source given".into())),
        Some(Source::Literal(_)) => Err(HostError::Invalid(
            "that's a key, not a place to find one: save it with `providers api-key set`".into(),
        )),
        Some(s) => Ok(s),
    }
}

/// Add a place to look for `provider`'s (with `api`, that API's own; as
/// `provider#key`, that named key's) key: at the end of its `api_key` list,
/// or first with `first` (one already listed moves there). An API that used
/// the provider's key gets its own list. Returns the sources afterwards
/// ([`key_source_list`]).
pub fn add_key_source(
    provider: &str,
    api: Option<&str>,
    source: &Value,
    first: bool,
) -> Result<Value, HostError> {
    add_key_source_in(&lexi_home(), provider, api, source, first)
}

fn add_key_source_in(
    home: &Path,
    provider: &str,
    api: Option<&str>,
    source: &Value,
    first: bool,
) -> Result<Value, HostError> {
    let new = source_arg(source)?;
    let scope = key_scope(home, provider, api, false)?;
    let mut list = if scope.own {
        scope.sources.clone()
    } else {
        Vec::new()
    };
    let had = list.iter().any(|s| s.same_place(&new));
    if !had || first {
        list.retain(|s| !s.same_place(&new));
        if first {
            list.insert(0, new);
        } else {
            list.push(new);
        }
        write_scope_sources(home, scope, &list)?;
    }
    keystore::forget_commands();
    key_source_list_in(home, provider, api)
}

/// Stop looking in a place for `provider`'s (its `api`'s own, or
/// `provider#key`'s) key — taking it out of the `api_key` list (`saved`
/// takes out only the marker: the saved key is looked in last again).
/// Returns whether it was listed.
pub fn remove_key_source(
    provider: &str,
    api: Option<&str>,
    source: &Value,
) -> Result<bool, HostError> {
    remove_key_source_in(&lexi_home(), provider, api, source)
}

fn remove_key_source_in(
    home: &Path,
    provider: &str,
    api: Option<&str>,
    source: &Value,
) -> Result<bool, HostError> {
    let gone = source_arg(source)?;
    let scope = key_scope(home, provider, api, false)?;
    if let (Some(a), false) = (api, scope.own) {
        return Err(uses_provider_key(&scope.provider, a));
    }
    if !scope.sources.iter().any(|s| s.same_place(&gone)) {
        return Ok(false);
    }
    let list: Vec<Source> = scope
        .sources
        .iter()
        .filter(|s| !s.same_place(&gone))
        .cloned()
        .collect();
    write_scope_sources(home, scope, &list)?;
    keystore::forget_commands();
    Ok(true)
}

/// A provider's keys: `{provider, default_key, keys: [{name, default,
/// source, from, error?}]}` — the unnamed key (`default`) first, then the
/// named ones — with where each key comes from (`source`/`from`, null when
/// none has one; commands aren't run). Never the keys themselves.
pub fn key_names(provider: &str) -> Result<Value, HostError> {
    key_names_in(&lexi_home(), provider)
}

fn key_names_in(home: &Path, provider: &str) -> Result<Value, HostError> {
    check_provider_name(provider)?;
    let mut doc = read_providers_doc(home)?;
    let entry = raw_entry(&mut doc, provider)?;
    let resolved = resolve_template(&Value::Object(entry))?;
    let default = resolved
        .get("default_key")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_KEY)
        .to_string();
    let store = Store::open(home)?;
    let mut names = vec![DEFAULT_KEY.to_string()];
    names.extend(named_key_names(&resolved));
    let keys: Vec<Value> = names
        .iter()
        .map(|name| {
            let arg = format!("{provider}#{name}");
            let found = scope_sources(home, &store, &arg, None)
                .map(|scope| scope_lookup(home, &store, &scope, false));
            let (source, from, error) = match &found {
                Ok(l) => match l.failure() {
                    Some((s, e)) => (None, Some(s.describe()), Some(e.to_string())),
                    None => (
                        l.source().map(Source::kind),
                        l.source().map(Source::describe),
                        None,
                    ),
                },
                Err(e) => (None, None, Some(e.to_string())),
            };
            let mut row = serde_json::json!({
                "name": name,
                "default": *name == default,
                "source": source,
                "from": from,
            });
            if let Some(e) = error {
                row["error"] = Value::from(e);
            }
            row
        })
        .collect();
    Ok(serde_json::json!({
        "provider": provider,
        "default_key": default,
        "keys": keys,
    }))
}

/// Add the named key `name` to `provider` (`keys: {name: {}}` — its saved key
/// alone until sources are added). Whether it's new.
pub fn add_named_key(provider: &str, name: &str) -> Result<bool, HostError> {
    add_named_key_in(&lexi_home(), provider, name)
}

fn add_named_key_in(home: &Path, provider: &str, name: &str) -> Result<bool, HostError> {
    check_new_key_name(name)?;
    let scope = key_scope(home, &format!("{provider}#{name}"), None, true)?;
    let exists = scope.entry.get("keys").and_then(|k| k.get(name)).is_some();
    if !exists {
        let sources = scope.sources.clone();
        write_scope_sources(home, scope, &sources)?;
    }
    Ok(!exists)
}

/// A name a new named key may take (not `default`).
fn check_new_key_name(name: &str) -> Result<(), HostError> {
    check_key_name(name)?;
    if name == DEFAULT_KEY {
        return Err(HostError::Invalid(
            "\"default\" is the provider's unnamed key: choose another name".into(),
        ));
    }
    Ok(())
}

/// Rename `provider`'s named key `old` to `new`: its config, its saved key
/// and `default_key` if it named `old`.
pub fn rename_named_key(provider: &str, old: &str, new: &str) -> Result<(), HostError> {
    rename_named_key_in(&lexi_home(), provider, old, new)
}

fn rename_named_key_in(home: &Path, provider: &str, old: &str, new: &str) -> Result<(), HostError> {
    check_new_key_name(new)?;
    let mut doc = read_providers_doc(home)?;
    let mut entry = raw_entry(&mut doc, provider)?;
    let has_old = entry.get("keys").and_then(|k| k.get(old)).is_some();
    if !has_old {
        return Err(unknown_key(provider, old, &Value::Object(entry)));
    }
    if entry.get("keys").and_then(|k| k.get(new)).is_some() {
        return Err(HostError::Invalid(format!(
            "{provider} already has a key named {new:?}"
        )));
    }
    if let Some(keys) = entry.get_mut("keys").and_then(Value::as_object_mut) {
        let renamed: Map = std::mem::take(keys)
            .into_iter()
            .map(|(k, v)| {
                if k == old {
                    (new.to_string(), v)
                } else {
                    (k, v)
                }
            })
            .collect();
        *keys = renamed;
    }
    if entry.get("default_key").and_then(Value::as_str) == Some(old) {
        entry.insert("default_key".into(), Value::from(new));
    }
    let (from, to) = (Slot::named(provider, old), Slot::named(provider, new));
    Store::open(home)?.rename(&from, &to)?;
    let (from_file, to_file) = (
        keystore::file_path(home, &from.file()),
        keystore::file_path(home, &to.file()),
    );
    if from_file.exists() {
        std::fs::rename(from_file, to_file)?;
    }
    providers_mut(&mut doc).insert(provider.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    keystore::forget_commands();
    Ok(())
}

/// Remove `provider`'s named key `name`: its config entry and its saved key
/// (and `default_key`, if it named it).
pub fn remove_named_key(provider: &str, name: &str) -> Result<(), HostError> {
    remove_named_key_in(&lexi_home(), provider, name)
}

fn remove_named_key_in(home: &Path, provider: &str, name: &str) -> Result<(), HostError> {
    check_new_key_name(name)?;
    let mut doc = read_providers_doc(home)?;
    let mut entry = raw_entry(&mut doc, provider)?;
    let known = entry
        .get_mut("keys")
        .and_then(Value::as_object_mut)
        .is_some_and(|k| k.shift_remove(name).is_some());
    if !known {
        return Err(unknown_key(provider, name, &Value::Object(entry)));
    }
    if entry
        .get("keys")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        entry.shift_remove("keys");
    }
    if entry.get("default_key").and_then(Value::as_str) == Some(name) {
        entry.shift_remove("default_key");
    }
    Store::open(home)?.delete(&Slot::named(provider, name))?;
    providers_mut(&mut doc).insert(provider.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    keystore::forget_commands();
    Ok(())
}

/// Make `name` the key `provider`'s requests use when they don't name one
/// (`default_key`); `None` or `default`: its unnamed key.
pub fn set_default_key(provider: &str, name: Option<&str>) -> Result<(), HostError> {
    set_default_key_in(&lexi_home(), provider, name)
}

fn set_default_key_in(home: &Path, provider: &str, name: Option<&str>) -> Result<(), HostError> {
    let mut doc = read_providers_doc(home)?;
    let mut entry = raw_entry(&mut doc, provider)?;
    match name.filter(|n| *n != DEFAULT_KEY) {
        None => {
            entry.shift_remove("default_key");
        }
        Some(n) => {
            if entry.get("keys").and_then(|k| k.get(n)).is_none() {
                return Err(unknown_key(provider, n, &Value::Object(entry)));
            }
            entry.insert("default_key".into(), Value::from(n));
        }
    }
    providers_mut(&mut doc).insert(provider.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    keystore::forget_commands();
    Ok(())
}

/// One-time upgrades of an older config's `api_key` lists (best effort):
/// - an entry still in the old single form with a key file
///   (`providers/<name>/api-key`, which older versions read first) gets
///   `api_key: [saved, …]`, so the same key keeps being used;
/// - a `{keychain: …}` entry (a development build kept keys in the OS
///   keychain) becomes `saved`;
/// - Lexi's own key file listed as a `{file: …}` (0.3.0 listed it when it
///   saved a key) comes out — the saved key is looked in anyway — or becomes
///   `saved` where it isn't last; a provider's list left equal to its
///   template's is taken out (the template's applies).
pub(crate) fn upgrade_key_sources(home: &Path) {
    let Ok(mut doc) = read_providers_doc(home) else {
        return;
    };
    let mut changed = false;
    for (name, entry) in providers_mut(&mut doc).iter_mut() {
        changed |= replace_keychain_sources(entry);
        let legacy = matches!(entry.get("api_key"), None | Some(Value::String(_)));
        if legacy && home.join(key_file(name, None)).is_file() {
            if let Ok(resolved) = resolve_template(entry) {
                let mut list = vec![Source::Saved(String::new())];
                list.extend(key_sources(&resolved).unwrap_or_default());
                if let Some(o) = entry.as_object_mut() {
                    o.insert("api_key".into(), keystore::sources_value(&list));
                    changed = true;
                }
            }
        }
        changed |= unlist_own_key_files(home, name, entry);
    }
    if changed {
        let _ = write_yaml(&providers_config_path(home), &Value::Object(doc));
    }
}

/// Move the key files Lexi saved into the OS store, when that's where saved
/// keys are kept now (`docs/key-storage-design.md` §6 step 2); what moved, as
/// messages. Best effort: a key that can't move stays in its file and is
/// read from there.
pub(crate) fn move_saved_keys(home: &Path) -> Vec<String> {
    let Ok(doc) = read_providers_doc(home) else {
        return Vec::new();
    };
    let names: Vec<String> = doc
        .get("providers")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    key_store::move_into_store(home, &names)
}

/// In a raw provider entry, Lexi's own key file listed in its `api_key` (and
/// its APIs') comes out — or becomes `saved` when sources follow it — and a
/// trailing `saved` goes (it's where the saved key is anyway). A provider
/// list left the same as its template's is removed; an API's left empty
/// stays as `[saved]` (it still has its own key). Whether anything changed.
fn unlist_own_key_files(home: &Path, provider: &str, entry: &mut Value) -> bool {
    let tidy = |list: &[Source], own: &str| -> Option<Vec<Source>> {
        let own_path = home.join(own);
        let is_own =
            |s: &Source| matches!(s, Source::File(f) if keystore::file_path(home, f) == own_path);
        if !list.iter().any(is_own) {
            return None;
        }
        let mut out: Vec<Source> = Vec::new();
        for s in list {
            let s = if is_own(s) {
                Source::Saved(String::new())
            } else {
                s.clone()
            };
            if !(matches!(s, Source::Saved(_)) && out.iter().any(|o| matches!(o, Source::Saved(_))))
            {
                out.push(s);
            }
        }
        if matches!(out.last(), Some(Source::Saved(_))) {
            out.pop();
        }
        Some(out)
    };
    let mut changed = false;
    if let Ok(list) = keystore::parse_sources(entry.get("api_key")) {
        if let Some(out) = tidy(&list, &key_file(provider, None)) {
            let mut without = entry.clone();
            if let Some(o) = without.as_object_mut() {
                o.shift_remove("api_key");
            }
            let template_list = resolve_template(&without)
                .ok()
                .and_then(|r| key_sources(&r).ok());
            if let Some(o) = entry.as_object_mut() {
                if template_list.as_ref() == Some(&out) && entry_has_template(o) {
                    o.shift_remove("api_key");
                } else {
                    o.insert("api_key".into(), keystore::sources_value(&out));
                }
            }
            changed = true;
        }
    }
    if let Some(apis) = entry.get_mut("apis").and_then(Value::as_array_mut) {
        for b in apis.iter_mut().filter(|b| b.get("api_key").is_some()) {
            let Some(api) = b.get("api").and_then(Value::as_str).map(String::from) else {
                continue;
            };
            let Ok(list) = keystore::parse_sources(b.get("api_key")) else {
                continue;
            };
            if let Some(mut out) = tidy(&list, &key_file(provider, Some(&api))) {
                if out.is_empty() {
                    out.push(Source::Saved(String::new()));
                }
                b["api_key"] = keystore::sources_value(&out);
                changed = true;
            }
        }
    }
    changed
}

fn entry_has_template(entry: &Map) -> bool {
    entry.get("template").and_then(Value::as_str).is_some()
}

/// In a raw provider entry, each `{keychain: …}` in its `api_key` (and its
/// APIs') becomes `saved`, listed once. Whether anything changed.
fn replace_keychain_sources(entry: &mut Value) -> bool {
    let mut changed = false;
    let mut fix = |v: &mut Value| {
        let items: Vec<Value> = match v.get("api_key") {
            Some(Value::Array(a)) => a.clone(),
            Some(one) => vec![one.clone()],
            None => return,
        };
        if !items.iter().any(|i| i.get("keychain").is_some()) {
            return;
        }
        let mut out: Vec<Value> = Vec::new();
        for item in items {
            let item = if item.get("keychain").is_some() {
                Value::from(keystore::SAVED)
            } else {
                item
            };
            if !out.contains(&item) {
                out.push(item);
            }
        }
        v["api_key"] = Value::Array(out);
        changed = true;
    };
    fix(entry);
    if let Some(apis) = entry.get_mut("apis").and_then(Value::as_array_mut) {
        for b in apis.iter_mut() {
            fix(b);
        }
    }
    changed
}

/// Extract provider names from a loaded providers config (declaration order).
pub fn provider_names(providers: &Value) -> Vec<String> {
    providers
        .get("providers")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Load the providers config and return the configured provider names.
pub fn list_providers() -> Result<Vec<String>, HostError> {
    Ok(provider_names(&load_providers()?))
}

/// Read a per-provider YAML file (e.g. `configured-models.yaml`) under `home`,
/// returning the parsed value or `Value::Null` if the file is absent.
fn read_provider_file(home: &Path, provider: &str, file: &str) -> Result<Value, HostError> {
    let path = home.join("providers").join(provider).join(file);
    if !path.exists() {
        return Ok(Value::Null);
    }
    Ok(serde_yaml::from_str(&std::fs::read_to_string(&path)?)?)
}

/// A configured model for a provider, as surfaced to the GUI/CLI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelInfo {
    pub id: String,
    /// Display name (`name` in configured-models.yaml).
    pub name: Option<String>,
    pub description: Option<String>,
    /// Whether it's your default model for some kind (`default_for`).
    pub default: bool,
    /// The kinds it's your default for (`chat`, `image-generation`, …).
    pub default_for: Vec<String>,
    /// Aliases that resolve to this model id.
    pub aliases: Vec<String>,
    /// The model's full metadata entry (name, description, max_tokens, …).
    pub metadata: Value,
}

/// Load a provider's configured models, enriched with its default model and
/// aliases. Reads `configured-models.yaml`, `default-model.yaml`, and
/// `model-aliases.yaml` under `providers/<provider>/`. Preserves declaration
/// order; returns an empty list if the provider has no configured models.
pub fn load_models(provider: &str) -> Result<Vec<ModelInfo>, HostError> {
    load_models_in(&lexi_home(), provider)
}

fn load_models_in(home: &Path, provider: &str) -> Result<Vec<ModelInfo>, HostError> {
    let models = read_provider_file(home, provider, "configured-models.yaml")?;
    let defaults = default_models_in(home, provider)?;
    let aliases = read_provider_file(home, provider, "model-aliases.yaml")?;
    let aliases = aliases.get("aliases").and_then(Value::as_object);

    let entries = models.get("models").and_then(Value::as_object);
    let Some(entries) = entries else {
        return Ok(Vec::new());
    };

    Ok(entries
        .iter()
        .map(|(id, meta)| {
            let alias_list = aliases
                .map(|a| {
                    a.iter()
                        .filter(|(_, target)| target.as_str() == Some(id.as_str()))
                        .map(|(alias, _)| alias.clone())
                        .collect()
                })
                .unwrap_or_default();
            ModelInfo {
                id: id.clone(),
                name: meta.get("name").and_then(Value::as_str).map(str::to_string),
                description: meta
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                default: defaults.values().any(|m| m.as_str() == Some(id.as_str())),
                default_for: defaults
                    .iter()
                    .filter(|(_, m)| m.as_str() == Some(id.as_str()))
                    .map(|(k, _)| k.clone())
                    .collect(),
                aliases: alias_list,
                metadata: meta.clone(),
            }
        })
        .collect())
}

/// Path to a per-provider config file under `providers/<provider>/`.
fn provider_file(home: &Path, provider: &str, file: &str) -> PathBuf {
    home.join("providers").join(provider).join(file)
}

/// Write `value` as YAML, creating the parent dir if needed.
fn write_yaml(path: &Path, value: &Value) -> Result<(), HostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_yaml::to_string(value)?)?;
    Ok(())
}

/// Your default model for each kind (`default-model.yaml`), as `{chat: …,
/// image-generation: …}`. The chat default is the file's `default_model` (as
/// earlier versions wrote it, which they still read); other kinds are under
/// `defaults:`. Defaults you haven't set come from the provider (see
/// `models::defaults`).
pub fn default_models(provider: &str) -> Result<Map, HostError> {
    default_models_in(&lexi_home(), provider)
}

fn default_models_in(home: &Path, provider: &str) -> Result<Map, HostError> {
    let data = read_provider_file(home, provider, "default-model.yaml")?;
    let mut out = Map::new();
    if let Some(m) = data.get("default_model").and_then(Value::as_str) {
        out.insert("chat".into(), Value::from(m));
    }
    if let Some(kinds) = data.get("defaults").and_then(Value::as_object) {
        for (kind, m) in kinds {
            if let Some(m) = m.as_str() {
                out.entry(kind.clone()).or_insert_with(|| Value::from(m));
            }
        }
    }
    Ok(out)
}

/// Your default chat model, if you set one.
pub(crate) fn default_model(provider: &str) -> Result<Option<String>, HostError> {
    default_model_in(&lexi_home(), provider)
}

fn default_model_in(home: &Path, provider: &str) -> Result<Option<String>, HostError> {
    Ok(default_models_in(home, provider)?
        .get("chat")
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// Set (or clear, with `None`) your default model for `kind`. The model isn't
/// checked here; `models::set_default` checks it and finds its kind.
pub(crate) fn set_kind_default(
    provider: &str,
    kind: &str,
    model: Option<&str>,
) -> Result<(), HostError> {
    set_kind_default_in(&lexi_home(), provider, kind, model)
}

fn set_kind_default_in(
    home: &Path,
    provider: &str,
    kind: &str,
    model: Option<&str>,
) -> Result<(), HostError> {
    lexi_core::validate_tag(&format!("kind:{kind}")).map_err(HostError::Invalid)?;
    let mut all = default_models_in(home, provider)?;
    match model {
        Some(m) => {
            all.insert(kind.to_string(), Value::from(m));
        }
        None => {
            all.shift_remove(kind);
        }
    }
    let mut data = Map::new();
    if let Some(chat) = all.get("chat") {
        data.insert("default_model".into(), chat.clone());
    }
    let others: Map = all.into_iter().filter(|(k, _)| k != "chat").collect();
    if !others.is_empty() {
        data.insert("defaults".into(), Value::Object(others));
    }
    write_yaml(
        &provider_file(home, provider, "default-model.yaml"),
        &Value::Object(data),
    )
}

/// The provider's `alias -> model` map (empty if none configured).
pub fn model_aliases(provider: &str) -> Result<Map, HostError> {
    model_aliases_in(&lexi_home(), provider)
}

fn model_aliases_in(home: &Path, provider: &str) -> Result<Map, HostError> {
    let raw = read_provider_file(home, provider, "model-aliases.yaml")?;
    // Files use either a top-level `aliases:` wrapper or a flat map.
    let map = raw
        .get("aliases")
        .and_then(Value::as_object)
        .or_else(|| raw.as_object())
        .cloned()
        .unwrap_or_default();
    Ok(map)
}

/// Save the alias map, preserving the file's existing wrapper format (a
/// top-level `aliases:` key if present, else a flat map — matching the CLI).
fn save_model_aliases_in(home: &Path, provider: &str, aliases: &Map) -> Result<(), HostError> {
    let path = provider_file(home, provider, "model-aliases.yaml");
    let wrapped = path.exists()
        && read_provider_file(home, provider, "model-aliases.yaml")?
            .get("aliases")
            .is_some();
    let data = if wrapped {
        serde_json::json!({ "aliases": aliases })
    } else {
        Value::Object(aliases.clone())
    };
    write_yaml(&path, &data)
}

/// Set an alias to point at a model.
pub fn set_model_alias(provider: &str, alias: &str, model: &str) -> Result<(), HostError> {
    set_model_alias_in(&lexi_home(), provider, alias, model)
}

fn set_model_alias_in(
    home: &Path,
    provider: &str,
    alias: &str,
    model: &str,
) -> Result<(), HostError> {
    let mut aliases = model_aliases_in(home, provider)?;
    aliases.insert(alias.to_string(), Value::String(model.to_string()));
    save_model_aliases_in(home, provider, &aliases)
}

/// Remove an alias. Errors if it doesn't exist.
pub fn delete_model_alias(provider: &str, alias: &str) -> Result<(), HostError> {
    delete_model_alias_in(&lexi_home(), provider, alias)
}

fn delete_model_alias_in(home: &Path, provider: &str, alias: &str) -> Result<(), HostError> {
    let mut aliases = model_aliases_in(home, provider)?;
    if aliases.shift_remove(alias).is_none() {
        return Err(HostError::Config(format!(
            "alias {alias:?} not found for {provider}"
        )));
    }
    save_model_aliases_in(home, provider, &aliases)
}

/// Add or update a model's metadata (merging over any existing entry), optionally
/// making it the default for its kind (`models::set_default`). Returns whether
/// the model already existed. Mirrors the CLI's `models add`/`set` (both
/// upsert-merge).
pub fn upsert_model(
    provider: &str,
    model: &str,
    meta: &Map,
    set_default: bool,
) -> Result<bool, HostError> {
    let existed = upsert_model_in(&lexi_home(), provider, model, meta, None)?;
    if set_default {
        crate::models::set_default(provider, model, None)?;
    }
    Ok(existed)
}

/// `default_kind`: make it your default for that kind.
fn upsert_model_in(
    home: &Path,
    provider: &str,
    model: &str,
    meta: &Map,
    default_kind: Option<&str>,
) -> Result<bool, HostError> {
    let mut data = match read_provider_file(home, provider, "configured-models.yaml")? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let models = data
        .entry("models")
        .or_insert_with(|| Value::Object(Map::new()));
    if !models.is_object() {
        *models = Value::Object(Map::new());
    }
    let models = models.as_object_mut().unwrap();
    let existed = models.contains_key(model);
    let mut merged = models
        .get(model)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (k, v) in meta {
        merged.insert(k.clone(), v.clone());
    }
    models.insert(model.to_string(), Value::Object(merged));

    write_yaml(
        &provider_file(home, provider, "configured-models.yaml"),
        &Value::Object(data),
    )?;
    if let Some(kind) = default_kind {
        set_kind_default_in(home, provider, kind, Some(model))?;
    }
    Ok(existed)
}

/// Remove a model. Errors if absent. If it was your default for a kind, that
/// default falls to the first remaining model of the same kind (or is cleared,
/// so the provider's default applies, when there's none).
pub fn delete_model(provider: &str, model: &str) -> Result<(), HostError> {
    delete_model_in(&lexi_home(), provider, model, |m| {
        crate::models::classify(provider, m)
            .ok()
            .and_then(|c| c["kind"].as_str().map(String::from))
    })
}

/// `kind_of`: a model's kind (read after the model is gone).
fn delete_model_in(
    home: &Path,
    provider: &str,
    model: &str,
    kind_of: impl Fn(&str) -> Option<String>,
) -> Result<(), HostError> {
    let mut data = match read_provider_file(home, provider, "configured-models.yaml")? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let remaining: Vec<String>;
    {
        let models = data
            .get_mut("models")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                HostError::Config(format!("model {model:?} not found for {provider}"))
            })?;
        if models.shift_remove(model).is_none() {
            return Err(HostError::Config(format!(
                "model {model:?} not found for {provider}"
            )));
        }
        remaining = models.keys().cloned().collect();
    }
    write_yaml(
        &provider_file(home, provider, "configured-models.yaml"),
        &Value::Object(data),
    )?;
    let was_default_for: Vec<String> = default_models_in(home, provider)?
        .into_iter()
        .filter(|(_, m)| m.as_str() == Some(model))
        .map(|(k, _)| k)
        .collect();
    for kind in was_default_for {
        let next = remaining
            .iter()
            .find(|m| kind_of(m).as_deref() == Some(kind.as_str()));
        set_kind_default_in(home, provider, &kind, next.map(String::as_str))?;
    }
    Ok(())
}

/// Load `config.json` (the CLI's app settings), or an empty object if absent.
/// Caller-supplied defaults are layered on top by the frontend.
pub fn load_config() -> Result<Value, HostError> {
    load_config_in(&lexi_home())
}

pub(crate) fn load_config_in(home: &Path) -> Result<Value, HostError> {
    let path = home.join("config.json");
    if !path.exists() {
        return Ok(Value::Object(Default::default()));
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(&path)?)?)
}

/// Load the saved prompts as `(name, body)` pairs, where `body` is the prompt's
/// JSON object (`prompt`, `max_tokens`, …). Reads `prompts/*.json`; the active
/// prompt is the one stored under the `$$` name. Sorted by name for stable order.
pub fn load_prompts() -> Result<Vec<(String, Value)>, HostError> {
    load_prompts_in(&lexi_home())
}

/// The payload of a saved prompt: `name`, or the active prompt (`$$`) when `None`.
pub fn resolve_prompt(name: Option<&str>) -> Result<Value, HostError> {
    let name = name.unwrap_or(ACTIVE_PROMPT);
    match load_prompts()?.into_iter().find(|(n, _)| n == name) {
        Some((_, body)) => Ok(body),
        None if name == ACTIVE_PROMPT => Err(HostError::Prompt(
            "no active prompt; save one with `prompts set <text>`, name a saved prompt, \
             or pass text directly"
                .into(),
        )),
        None => Err(HostError::Prompt(format!("prompt {name:?} not found"))),
    }
}

fn load_prompts_in(home: &Path) -> Result<Vec<(String, Value)>, HostError> {
    let dir = home.join("prompts");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for ent in std::fs::read_dir(&dir)? {
        let path = ent?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        entries.push((name.to_string(), body));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

/// Merge `updates` into `config.json` (created if absent) and return the result.
/// Existing keys not in `updates` are preserved.
pub fn set_config(updates: &Map) -> Result<Value, HostError> {
    set_config_in(&lexi_home(), updates)
}

/// `request_timeout` when it isn't set: 5 minutes.
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 300;

/// A `request_timeout` value as seconds (a number, or its text).
fn timeout_secs(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

tokio::task_local! {
    /// The time limit set for one operation by [`with_request_timeout`].
    static TIMEOUT_OVERRIDE: Option<std::time::Duration>;
}

/// Run `fut` with its own time limit for provider requests: `secs` seconds
/// (0 = no limit) instead of config.json's `request_timeout`, meaning the
/// same thing (a buffered request's whole wait, a stream's longest silence).
/// Every request `fut` makes is covered, including a background job's polls.
/// The CLI's `--timeout` and Python's `timeout=`.
pub async fn with_request_timeout<F: std::future::Future>(secs: u64, fut: F) -> F::Output {
    let limit = (secs > 0).then(|| std::time::Duration::from_secs(secs));
    TIMEOUT_OVERRIDE.scope(limit, fut).await
}

/// Where the time limit in force comes from, for messages.
pub(crate) fn timeout_source() -> &'static str {
    if TIMEOUT_OVERRIDE.try_with(|_| ()).is_ok() {
        "the timeout given for this request"
    } else {
        "request_timeout in config.json"
    }
}

/// How long a provider request may wait: the limit [`with_request_timeout`]
/// set, else config.json's `request_timeout` in seconds (default
/// [`DEFAULT_REQUEST_TIMEOUT`]; 0 = no limit). A buffered request must finish
/// within it; a streamed one may run as long as it keeps sending, but stops
/// after that long with nothing received.
pub(crate) fn request_timeout() -> Option<std::time::Duration> {
    if let Ok(limit) = TIMEOUT_OVERRIDE.try_with(|l| *l) {
        return limit;
    }
    let secs = load_config()
        .ok()
        .and_then(|c| c.get("request_timeout").and_then(timeout_secs))
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT);
    (secs > 0).then(|| std::time::Duration::from_secs(secs))
}

fn set_config_in(home: &Path, updates: &Map) -> Result<Value, HostError> {
    let mut cfg = match load_config_in(home)? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    for (k, v) in updates {
        let v = if k == "request_timeout" {
            Value::from(timeout_secs(v).ok_or_else(|| {
                HostError::Invalid(format!(
                    "request_timeout is a whole number of seconds (0 = no limit), not {v}"
                ))
            })?)
        } else if k == KEY_STORE_SETTING {
            let mode = v.as_str().and_then(key_store::parse_mode).ok_or_else(|| {
                HostError::Invalid(format!(
                    "key_store is {}, not {v}",
                    key_store::KEY_STORE_MODES.join(", ")
                ))
            })?;
            Value::from(mode)
        } else {
            v.clone()
        };
        cfg.insert(k.clone(), v);
    }
    write_json(&home.join("config.json"), &Value::Object(cfg.clone()))?;
    Ok(Value::Object(cfg))
}

/// The `prompts/` dir name of the active prompt slot.
/// The saved-prompt name that holds the active prompt.
pub const ACTIVE_PROMPT: &str = "$$";

/// Save a prompt under `name` (or the active `$$` slot). `body` is written
/// verbatim as the prompt's JSON object (e.g. `{prompt, max_tokens, …}`).
pub fn save_prompt(name: &str, body: &Value) -> Result<(), HostError> {
    save_prompt_in(&lexi_home(), name, body)
}

fn save_prompt_in(home: &Path, name: &str, body: &Value) -> Result<(), HostError> {
    write_json(&prompt_path(home, name)?, body)
}

/// Delete the named prompt. Errors if the name is invalid or the file is absent.
pub fn delete_prompt(name: &str) -> Result<(), HostError> {
    delete_prompt_in(&lexi_home(), name)
}

fn delete_prompt_in(home: &Path, name: &str) -> Result<(), HostError> {
    let path = prompt_path(home, name)?;
    if !path.exists() {
        return Err(HostError::Config(format!("prompt {name:?} not found")));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

/// Make `name` the active prompt by copying its body into the `$$` slot.
pub fn set_active_prompt(name: &str) -> Result<(), HostError> {
    set_active_prompt_in(&lexi_home(), name)
}

fn set_active_prompt_in(home: &Path, name: &str) -> Result<(), HostError> {
    let src = prompt_path(home, name)?;
    if !src.exists() {
        return Err(HostError::Config(format!("prompt {name:?} not found")));
    }
    let body: Value = serde_json::from_str(&std::fs::read_to_string(&src)?)?;
    save_prompt_in(home, ACTIVE_PROMPT, &body)
}

/// Resolve a prompt name to its `prompts/<name>.json` path, rejecting names that
/// contain path separators or `..` (so a name can't escape the prompts dir).
fn prompt_path(home: &Path, name: &str) -> Result<PathBuf, HostError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.split(['/', '\\']).any(|seg| seg == "..")
    {
        return Err(HostError::Config(format!("invalid prompt name {name:?}")));
    }
    Ok(home.join("prompts").join(format!("{name}.json")))
}

/// Write `value` as pretty JSON, creating the parent dir if needed.
fn write_json(path: &Path, value: &Value) -> Result<(), HostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

/// List available API definitions as `(name, description)` pairs — the union of
/// shipped (embedded) definitions and any custom ones under `~/.lexi-cli/apis/`.
pub fn list_api_defs() -> Result<Vec<(String, Option<String>)>, HostError> {
    list_api_defs_in(&lexi_home())
}

fn list_api_defs_in(home: &Path) -> Result<Vec<(String, Option<String>)>, HostError> {
    use std::collections::BTreeSet;
    let mut names: BTreeSet<String> = embedded_api_names().iter().map(|s| s.to_string()).collect();
    let dir = home.join("apis");
    if dir.exists() {
        for ent in std::fs::read_dir(&dir)? {
            let path = ent?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names.insert(stem.to_string());
                }
            }
        }
    }
    Ok(names
        .into_iter()
        .map(|name| {
            let desc = load_api_raw_in(home, &name)
                .ok()
                .and_then(|t| serde_yaml::from_str::<Value>(&t).ok())
                .and_then(|v| {
                    v.get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            (name, desc)
        })
        .collect())
}

/// Return the raw YAML text of an API definition. Shipped definitions resolve to
/// their embedded copy (matching [`load_api_def`]); otherwise the user file.
pub fn load_api_raw(name: &str) -> Result<String, HostError> {
    load_api_raw_in(&lexi_home(), name)
}

fn load_api_raw_in(home: &Path, name: &str) -> Result<String, HostError> {
    if let Some(text) = embedded_api_text(name) {
        return Ok(text.to_string());
    }
    let user = home.join("apis").join(format!("{name}.yaml"));
    if user.exists() {
        return Ok(std::fs::read_to_string(&user)?);
    }
    Err(HostError::UnknownApi(name.to_string()))
}

/// The user-dir path for a custom API definition, rejecting names that contain
/// path separators or `..` (so a name can't escape the apis dir).
fn api_def_path(home: &Path, name: &str) -> Result<PathBuf, HostError> {
    let name = name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.split(['/', '\\']).any(|seg| seg == "..")
    {
        return Err(HostError::Config(format!(
            "invalid definition name {name:?}"
        )));
    }
    Ok(home.join("apis").join(format!("{name}.yaml")))
}

/// Set a definition's request path and/or response text path. A text path
/// switches extraction to simple mode, so the walked-mode keys are dropped.
fn apply_path_overrides(obj: &mut Map, path: Option<&str>, text_path: Option<&str>) {
    if let Some(p) = path {
        obj.entry("request")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(req) = obj.get_mut("request").and_then(Value::as_object_mut) {
            req.insert("path".into(), Value::String(p.to_string()));
        }
    }
    if let Some(tp) = text_path {
        obj.entry("response")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(resp) = obj.get_mut("response").and_then(Value::as_object_mut) {
            let te = resp
                .entry("text_extract")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(te) = te.as_object_mut() {
                te.insert("text_path".into(), Value::String(tp.to_string()));
                for drop in [
                    "items_path",
                    "item_filter",
                    "content_path",
                    "content_filter",
                ] {
                    te.shift_remove(drop);
                }
            }
        }
    }
}

/// Create a new API definition. With `from`, deep-copies that definition as a
/// template (embedded or user); otherwise scaffolds a default chat-style schema.
/// `path`/`text_path` override the request path and response text path. Mirrors
/// the CLI's `api create`.
pub fn create_api_def(
    name: &str,
    from: Option<&str>,
    path: Option<&str>,
    text_path: Option<&str>,
) -> Result<(), HostError> {
    create_api_def_in(&lexi_home(), name, from, path, text_path)
}

fn create_api_def_in(
    home: &Path,
    name: &str,
    from: Option<&str>,
    path: Option<&str>,
    text_path: Option<&str>,
) -> Result<(), HostError> {
    let dest = api_def_path(home, name)?;
    let name = name.trim();

    let mut data = match from {
        Some(src) => load_api_def_from(home, src)?,
        None => default_api_scaffold(name, path, text_path),
    };
    let obj = data
        .as_object_mut()
        .ok_or_else(|| HostError::Config("definition must be a mapping".into()))?;
    obj.insert("name".into(), Value::String(name.to_string()));
    apply_path_overrides(obj, path, text_path);
    write_yaml(&dest, &data)
}

/// Load a definition to use as a `create --from` template: the embedded copy for
/// shipped names, else the user file.
fn load_api_def_from(home: &Path, name: &str) -> Result<Value, HostError> {
    if let Some(text) = embedded_api_text(name) {
        return Ok(serde_yaml::from_str(text)?);
    }
    let user = home.join("apis").join(format!("{name}.yaml"));
    if user.exists() {
        return Ok(serde_yaml::from_str(&std::fs::read_to_string(&user)?)?);
    }
    Err(HostError::Config(format!(
        "source API definition {name:?} not found"
    )))
}

/// The default chat-style definition scaffold the CLI seeds a new def with.
fn default_api_scaffold(name: &str, path: Option<&str>, text_path: Option<&str>) -> Value {
    serde_json::json!({
        "name": name,
        "description": "",
        "auth": { "header": "Authorization", "value_prefix": "Bearer " },
        "request": {
            "method": "POST",
            "path": path.unwrap_or("/chat/completions"),
            "body_schema": {
                "type": "object",
                "required": ["model", "messages"],
                "properties": {
                    "model": { "type": "string", "x-bind": { "from": "model" } },
                    "messages": {
                        "type": "array",
                        "x-render": { "template": [{ "role": "{{role}}", "content": "{{prompt}}" }] }
                    },
                    "temperature": {
                        "type": "number", "minimum": 0, "maximum": 2,
                        "x-cli": { "flag": "--temperature", "alias": "-t" }
                    },
                    "max_tokens": {
                        "type": "integer", "minimum": 1,
                        "x-cli": { "flag": "--max-tokens", "alias": "-m" }
                    }
                }
            }
        },
        "response": {
            "text_extract": { "text_path": text_path.unwrap_or("choices[0].message.content") }
        }
    })
}

/// Save raw YAML for a (custom) API definition, then validate it. Errors if the
/// YAML doesn't parse; otherwise writes the file and returns the validation
/// errors (empty = valid). Mirrors the CLI's save-then-validate.
pub fn save_api_raw(name: &str, yaml: &str) -> Result<Vec<String>, HostError> {
    save_api_raw_in(&lexi_home(), name, yaml)
}

fn save_api_raw_in(home: &Path, name: &str, yaml: &str) -> Result<Vec<String>, HostError> {
    let dest = api_def_path(home, name)?;
    let parsed: Value =
        serde_yaml::from_str(yaml).map_err(|e| HostError::Config(format!("Invalid YAML: {e}")))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, yaml)?;
    Ok(lexi_core::validate_definition(&parsed))
}

/// Build the editor live-preview for a definition given either its raw `yaml`
/// (which takes priority) or its `name`. Unparseable/non-object YAML yields a
/// `{valid:false, errors:[...]}` result rather than an error, matching the CLI.
pub fn preview_api_def(
    name: Option<&str>,
    yaml: Option<&str>,
    prompt: &str,
    model: &str,
    role: &str,
) -> Result<Value, HostError> {
    let def = if let Some(y) = yaml {
        match serde_yaml::from_str::<Value>(y) {
            Ok(v) if v.is_object() => v,
            Ok(_) => {
                return Ok(serde_json::json!({
                    "valid": false,
                    "errors": ["YAML top level must be an object"],
                }))
            }
            Err(e) => {
                return Ok(serde_json::json!({
                    "valid": false,
                    "errors": [format!("YAML parse error: {e}")],
                }))
            }
        }
    } else if let Some(n) = name {
        load_api_def(n)?
    } else {
        return Err(HostError::Config("Provide either yaml or name".into()));
    };
    Ok(lexi_core::preview_definition(&def, prompt, model, role))
}

/// Validate a stored API definition, returning `(valid, errors)`.
pub fn validate_api_def(name: &str) -> Result<(bool, Vec<String>), HostError> {
    let def = load_api_def(name)?;
    let errors = lexi_core::validate_definition(&def);
    Ok((errors.is_empty(), errors))
}

/// Delete a custom API definition. Errors if no user file exists (shipped
/// definitions have no user file to remove).
pub fn delete_api_def(name: &str) -> Result<(), HostError> {
    delete_api_def_in(&lexi_home(), name)
}

fn delete_api_def_in(home: &Path, name: &str) -> Result<(), HostError> {
    let path = api_def_path(home, name)?;
    if !path.exists() {
        return Err(HostError::Config(format!(
            "API definition {name:?} not found"
        )));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

/// Remove a key from `config.json`; returns whether it was present.
pub fn delete_config(key: &str) -> Result<bool, HostError> {
    delete_config_in(&lexi_home(), key)
}

fn delete_config_in(home: &Path, key: &str) -> Result<bool, HostError> {
    let mut cfg = match load_config_in(home)? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    if cfg.shift_remove(key).is_none() {
        return Ok(false);
    }
    write_json(&home.join("config.json"), &Value::Object(cfg))?;
    Ok(true)
}

fn providers_config_path(home: &Path) -> PathBuf {
    home.join("providers").join("providers-config.yaml")
}

/// Read providers-config.yaml as a mutable mapping (empty if absent), with a
/// `providers` mapping guaranteed present.
fn read_providers_doc(home: &Path) -> Result<Map, HostError> {
    let path = providers_config_path(home);
    let mut doc = if path.exists() {
        match serde_yaml::from_str::<Value>(&std::fs::read_to_string(&path)?)? {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            _ => {
                return Err(HostError::Config(
                    "providers-config.yaml must be a mapping".into(),
                ))
            }
        }
    } else {
        Map::new()
    };
    if !doc.get("providers").is_some_and(Value::is_object) {
        doc.insert("providers".into(), Value::Object(Map::new()));
    }
    Ok(doc)
}

fn providers_mut(doc: &mut Map) -> &mut Map {
    doc.get_mut("providers")
        .and_then(Value::as_object_mut)
        .expect("read_providers_doc guarantees a providers mapping")
}

/// A provider name usable as a directory: non-empty, no separators or `..`.
fn check_provider_name(name: &str) -> Result<(), HostError> {
    if name.trim().is_empty() || name.contains(['/', '\\', '#']) || name == ".." || name == "." {
        return Err(HostError::Config(format!("invalid provider name {name:?}")));
    }
    Ok(())
}

/// Add a provider entry to providers-config.yaml. Errors if it exists.
pub fn add_provider(name: &str, provider_cfg: &Map) -> Result<(), HostError> {
    add_provider_in(&lexi_home(), name, provider_cfg)
}

fn add_provider_in(home: &Path, name: &str, provider_cfg: &Map) -> Result<(), HostError> {
    check_provider_name(name)?;
    let mut doc = read_providers_doc(home)?;
    let providers = providers_mut(&mut doc);
    if providers.contains_key(name) {
        return Err(HostError::Config(format!(
            "provider {name:?} already exists"
        )));
    }
    providers.insert(name.to_string(), Value::Object(provider_cfg.clone()));
    write_yaml(&providers_config_path(home), &Value::Object(doc))
}

/// Rename a provider: its config entry (position kept) and its
/// `providers/<name>/` directory (key files in it follow, and the config's
/// `api_key` lists point at their new place).
pub fn rename_provider(old: &str, new: &str) -> Result<(), HostError> {
    rename_provider_in(&lexi_home(), old, new)
}

fn rename_provider_in(home: &Path, old: &str, new: &str) -> Result<(), HostError> {
    check_provider_name(new)?;
    let mut doc = read_providers_doc(home)?;
    let providers = providers_mut(&mut doc);
    if !providers.contains_key(old) {
        return Err(HostError::UnknownProvider(old.to_string()));
    }
    if providers.contains_key(new) {
        return Err(HostError::Config(format!(
            "provider {new:?} already exists"
        )));
    }
    let renamed: Map = std::mem::take(providers)
        .into_iter()
        .map(|(k, v)| {
            if k == old {
                (new.to_string(), v)
            } else {
                (k, v)
            }
        })
        .collect();
    *providers = renamed;
    // Saved keys in the keychain go along (key files move with their folder).
    let store = Store::open(home)?;
    if let Some(entry) = providers.get(new) {
        for slot in saved_slots(home, old, entry) {
            store.rename(&slot, &slot.for_provider(new))?;
        }
    }
    if let Some(entry) = providers.get_mut(new) {
        let old_dir = format!("providers/{old}/");
        rewrite_entry_sources(entry, |list| {
            list.into_iter()
                .map(|s| match s {
                    Source::File(p) => match p.strip_prefix(&old_dir) {
                        Some(rest) => Source::File(format!("providers/{new}/{rest}")),
                        None => Source::File(p),
                    },
                    other => other,
                })
                .collect()
        });
    }
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    let old_dir = home.join("providers").join(old);
    if old_dir.exists() {
        std::fs::rename(&old_dir, home.join("providers").join(new))?;
    }
    Ok(())
}

/// Rewrite each `api_key` list written in a raw provider entry (its own and
/// its APIs').
fn rewrite_entry_sources(entry: &mut Value, mut f: impl FnMut(Vec<Source>) -> Vec<Source>) {
    let mut apply = |v: &mut Value| {
        if let Some(k) = v.get("api_key") {
            if let Ok(list) = keystore::parse_sources(Some(k)) {
                let before = list.clone();
                let after = f(list);
                if after != before {
                    v["api_key"] = keystore::sources_value(&after);
                }
            }
        }
    };
    apply(entry);
    if let Some(apis) = entry.get_mut("apis").and_then(Value::as_array_mut) {
        for b in apis.iter_mut().filter(|b| b.is_object()) {
            apply(b);
        }
    }
}

/// Every saved key `provider` may have: its own, and one per API it has or
/// could have (each shipped and custom API definition).
fn saved_slots(home: &Path, provider: &str, entry: &Value) -> Vec<Slot> {
    let mut apis: Vec<String> = embedded_api_names().into_iter().map(String::from).collect();
    if let Ok(defs) = list_api_defs_in(home) {
        apis.extend(defs.into_iter().map(|(name, _)| name));
    }
    if let Some(bindings) = entry.get("apis").and_then(Value::as_array) {
        apis.extend(
            bindings
                .iter()
                .filter_map(|b| b.get("api").and_then(Value::as_str))
                .map(String::from),
        );
    }
    apis.sort();
    apis.dedup();
    std::iter::once(Slot::new(provider, None))
        .chain(apis.iter().map(|a| Slot::new(provider, Some(a))))
        .chain(
            named_key_names(entry)
                .iter()
                .map(|k| Slot::named(provider, k)),
        )
        .collect()
}

/// Remove a provider: its config entry, its saved keys and its
/// `providers/<name>/` directory (models, aliases, default model, key files).
pub fn remove_provider(name: &str) -> Result<(), HostError> {
    remove_provider_in(&lexi_home(), name)
}

fn remove_provider_in(home: &Path, name: &str) -> Result<(), HostError> {
    check_provider_name(name)?;
    let mut doc = read_providers_doc(home)?;
    let Some(entry) = providers_mut(&mut doc).shift_remove(name) else {
        return Err(HostError::UnknownProvider(name.to_string()));
    };
    let store = Store::open(home)?;
    for slot in saved_slots(home, name, &entry) {
        store.delete(&slot)?;
    }
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    let dir = home.join("providers").join(name);
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// Edit a custom API definition in place: request path, response text path,
/// and `response.metadata` entries (`key -> path`). Shipped definitions are
/// read-only (the runtime always loads their embedded copy) — copy one with
/// [`create_api_def`] `from` first.
pub fn edit_api_def(
    name: &str,
    path: Option<&str>,
    text_path: Option<&str>,
    set_meta: &[(String, String)],
) -> Result<(), HostError> {
    edit_api_def_in(&lexi_home(), name, path, text_path, set_meta)
}

fn edit_api_def_in(
    home: &Path,
    name: &str,
    path: Option<&str>,
    text_path: Option<&str>,
    set_meta: &[(String, String)],
) -> Result<(), HostError> {
    if embedded_api_text(name).is_some() {
        return Err(HostError::Config(format!(
            "{name:?} is a shipped definition and can't be edited; copy it first \
             (create a new definition from it) and edit the copy"
        )));
    }
    let dest = api_def_path(home, name)?;
    if !dest.exists() {
        return Err(HostError::UnknownApi(name.to_string()));
    }
    let mut data: Value = serde_yaml::from_str(&std::fs::read_to_string(&dest)?)?;
    let obj = data
        .as_object_mut()
        .ok_or_else(|| HostError::Config("definition must be a mapping".into()))?;
    apply_path_overrides(obj, path, text_path);
    if !set_meta.is_empty() {
        let resp = obj
            .entry("response")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(resp) = resp.as_object_mut() {
            let meta = resp
                .entry("metadata")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(meta) = meta.as_object_mut() {
                for (k, v) in set_meta {
                    meta.insert(k.clone(), Value::String(v.clone()));
                }
            }
        }
    }
    write_yaml(&dest, &data)
}

// ---------------------------------------------------------------------------
// Templates and bindings: `providers upgrade`, `providers api …`.
// ---------------------------------------------------------------------------

/// The providers-config.yaml shipped before templates; `upgrade_provider`
/// drops values still at these defaults.
const LEGACY_DEFAULTS: &str =
    include_str!("../../lexi-core/providers/legacy-providers-config.yaml");

fn legacy_entry(template: &str) -> Value {
    serde_yaml::from_str::<Value>(LEGACY_DEFAULTS)
        .ok()
        .and_then(|d| d.get("providers")?.get(template).cloned())
        .unwrap_or(Value::Null)
}

/// `name`'s entry as written in providers-config.yaml (no template applied).
pub(crate) fn raw_provider_entry(name: &str) -> Result<Map, HostError> {
    raw_entry(&mut read_providers_doc(&lexi_home())?, name)
}

fn raw_entry(doc: &mut Map, name: &str) -> Result<Map, HostError> {
    providers_mut(doc)
        .get(name)
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| HostError::UnknownProvider(name.to_string()))
}

/// Copy providers-config.yaml aside (`.bak-<unix secs>[-n]`) before a rewrite.
fn backup_providers_config(home: &Path) -> Result<PathBuf, HostError> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let src = providers_config_path(home);
    // Never reuse a name: several rewrites in one second must each keep theirs
    // (the first holds the untouched original).
    let dest = (1..)
        .map(|n| match n {
            1 => src.with_file_name(format!("providers-config.yaml.bak-{secs}")),
            n => src.with_file_name(format!("providers-config.yaml.bak-{secs}-{n}")),
        })
        .find(|p| !p.exists())
        .expect("an unused backup name");
    std::fs::copy(&src, &dest)?;
    Ok(dest)
}

/// Convert a provider entry to the template form: `template: <name>` plus only
/// the values that differ from both the template and the old shipped
/// defaults. A custom single `api` / `default_model` become a binding (so the
/// template's own bindings don't hide them). `template` defaults to the
/// provider's name. With `apply`, backs up and rewrites providers-config.yaml;
/// otherwise reports what would change. Returns `{provider, template, changed,
/// before, after, dropped, kept, backup?}`.
pub fn upgrade_provider(
    name: &str,
    template: Option<&str>,
    apply: bool,
) -> Result<Value, HostError> {
    let home = lexi_home();
    let mut doc = read_providers_doc(&home)?;
    let entry = raw_entry(&mut doc, name)?;
    if let Some(t) = entry.get("template").and_then(Value::as_str) {
        if template.is_none_or(|want| want == t) {
            return Ok(serde_json::json!({
                "provider": name, "template": t, "changed": false,
                "before": entry, "after": entry, "dropped": [], "kept": [],
            }));
        }
    }
    let tname = template.unwrap_or(name);
    let tmpl = provider_template(tname)?;
    let legacy = legacy_entry(tname);
    let resolved = lexi_core::merge_provider_template(&tmpl, &serde_json::json!({}));
    let catch_all_api = lexi_core::provider_bindings(&resolved)
        .into_iter()
        .find(|b| b.get("for").is_none())
        .and_then(|b| b["api"].as_str().map(String::from));

    let mut after = Map::new();
    after.insert("template".into(), Value::from(tname));
    let (mut dropped, mut kept) = (Vec::new(), Vec::new());
    let mut user_bindings: Vec<Value> = Vec::new();
    let custom_api = entry
        .get("api")
        .and_then(Value::as_str)
        .filter(|a| Some(*a) != legacy.get("api").and_then(Value::as_str))
        .filter(|a| Some(*a) != catch_all_api.as_deref())
        .map(String::from);
    for (k, v) in &entry {
        if k == "template" {
            continue;
        }
        let at_default = legacy.get(k) == Some(v) || resolved.get(k) == Some(v);
        match k.as_str() {
            _ if at_default => dropped.push(k.clone()),
            "api" => {
                // A custom API becomes the catch-all binding (listed first).
                user_bindings.insert(0, serde_json::json!({ "api": v }));
                kept.push(k.clone());
            }
            "default_model" => {
                let api = custom_api
                    .clone()
                    .or(catch_all_api.clone())
                    .unwrap_or_default();
                match user_bindings.iter_mut().find(|b| b["api"] == api.as_str()) {
                    Some(b) => b["default_model"] = v.clone(),
                    None => {
                        user_bindings.push(serde_json::json!({ "api": api, "default_model": v }))
                    }
                }
                kept.push(k.clone());
            }
            _ => {
                after.insert(k.clone(), v.clone());
                kept.push(k.clone());
            }
        }
    }
    if !user_bindings.is_empty() {
        after.insert("apis".into(), Value::Array(user_bindings));
    }
    let mut out = serde_json::json!({
        "provider": name, "template": tname, "changed": true,
        "before": entry, "after": after, "dropped": dropped, "kept": kept,
    });
    if apply {
        out["backup"] = Value::from(backup_providers_config(&home)?.display().to_string());
        providers_mut(&mut doc).insert(name.to_string(), Value::Object(after));
        write_yaml(&providers_config_path(&home), &Value::Object(doc))?;
    }
    Ok(out)
}

/// A provider's bindings with where each comes from:
/// `[{api, for?, default_model?, url?, enabled, source: "template"|"user"|"config"}]`.
pub fn provider_apis(name: &str) -> Result<Vec<Value>, HostError> {
    let home = lexi_home();
    let mut doc = read_providers_doc(&home)?;
    let entry = Value::Object(raw_entry(&mut doc, name)?);
    let resolved = resolve_template(&entry)?;
    let user: Vec<String> = entry
        .get("apis")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            b.get("api")
                .or(Some(b))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    let templated = entry.get("template").is_some();
    let all: Vec<Value> = resolved
        .get("apis")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| lexi_core::provider_bindings(&resolved));
    Ok(all
        .into_iter()
        .map(|b| {
            let mut b = match b {
                Value::String(s) => serde_json::json!({ "api": s }),
                other => other,
            };
            let api = b["api"].as_str().unwrap_or("").to_string();
            let source = if !templated {
                "config"
            } else if user.contains(&api) {
                "user"
            } else {
                "template"
            };
            let enabled = b.get("enabled") != Some(&Value::Bool(false));
            if let Some(o) = b.as_object_mut() {
                o.insert("enabled".into(), Value::Bool(enabled));
                o.insert("source".into(), Value::from(source));
            }
            b
        })
        .collect())
}

/// Add (or update) a binding on a provider: `binding` is `{api, for?,
/// default_model?, url?}`. An older single-`api` entry is converted to an
/// `apis` list first. Returns true if the binding already existed.
pub fn add_provider_api(name: &str, binding: &Map) -> Result<bool, HostError> {
    add_provider_api_in(&lexi_home(), name, binding)
}

fn add_provider_api_in(home: &Path, name: &str, binding: &Map) -> Result<bool, HostError> {
    let api = binding
        .get("api")
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::Invalid("a binding needs an `api`".into()))?
        .to_string();
    load_api_def(&api)?;
    for t in binding
        .get("for")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        lexi_core::validate_tag(t).map_err(HostError::Invalid)?;
    }
    let mut doc = read_providers_doc(home)?;
    let mut entry = raw_entry(&mut doc, name)?;
    let mut list: Vec<Value> = match entry.get("apis").and_then(Value::as_array) {
        Some(l) => l.clone(),
        None if entry.get("template").is_none() => {
            // Older entry: its single `api` becomes the first binding.
            let b = lexi_core::provider_bindings(&Value::Object(entry.clone()));
            entry.shift_remove("api");
            b
        }
        None => Vec::new(),
    };
    let existed = match list
        .iter_mut()
        .find(|b| b.get("api").or(Some(*b)).and_then(Value::as_str) == Some(api.as_str()))
    {
        Some(b) => {
            if b.is_string() {
                *b = serde_json::json!({ "api": api });
            }
            if let Some(o) = b.as_object_mut() {
                o.shift_remove("enabled");
                for (k, v) in binding {
                    o.insert(k.clone(), v.clone());
                }
            }
            true
        }
        None => {
            list.push(Value::Object(binding.clone()));
            false
        }
    };
    // A binding the template has counts as existing too; and a user entry
    // that only names it (re-enabling it) is redundant — drop it.
    let in_template = entry
        .get("template")
        .and_then(Value::as_str)
        .map(provider_template)
        .transpose()?
        .is_some_and(|t| {
            lexi_core::provider_bindings(&t)
                .iter()
                .any(|b| b["api"] == api.as_str())
        });
    let existed = existed || in_template;
    if in_template {
        list.retain(|b| {
            !(b.get("api").and_then(Value::as_str) == Some(api.as_str())
                && b.as_object().is_some_and(|o| o.len() == 1))
        });
    }
    if list.is_empty() {
        entry.shift_remove("apis");
    } else {
        entry.insert("apis".into(), Value::Array(list));
    }
    providers_mut(&mut doc).insert(name.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    Ok(existed)
}

/// Remove a binding: one you added is deleted; one the template provides is
/// turned off (`enabled: false`). Refuses to leave the provider with no API.
pub fn remove_provider_api(name: &str, api: &str) -> Result<(), HostError> {
    let home = lexi_home();
    let mut doc = read_providers_doc(&home)?;
    let mut entry = raw_entry(&mut doc, name)?;
    let resolved = resolve_template(&Value::Object(entry.clone()))?;
    let active: Vec<String> = lexi_core::provider_bindings(&resolved)
        .into_iter()
        .filter_map(|b| b["api"].as_str().map(String::from))
        .collect();
    if !active.iter().any(|a| a == api) {
        return Err(HostError::Invalid(format!(
            "provider {name:?} has no {api:?} API (it has: {})",
            active.join(", ")
        )));
    }
    if active.len() == 1 {
        return Err(HostError::Invalid(format!(
            "{api:?} is {name:?}'s only API; add another first, or remove the provider"
        )));
    }
    let in_template = entry
        .get("template")
        .and_then(Value::as_str)
        .map(provider_template)
        .transpose()?
        .is_some_and(|t| {
            lexi_core::provider_bindings(&t)
                .iter()
                .any(|b| b["api"] == api)
        });
    let mut list: Vec<Value> = match entry.get("apis").and_then(Value::as_array) {
        Some(l) => l.clone(),
        None => {
            let b = lexi_core::provider_bindings(&Value::Object(entry.clone()));
            entry.shift_remove("api");
            b
        }
    };
    list.retain(|b| b.get("api").or(Some(b)).and_then(Value::as_str) != Some(api));
    if in_template {
        list.push(serde_json::json!({ "api": api, "enabled": false }));
    }
    entry.insert("apis".into(), Value::Array(list));
    providers_mut(&mut doc).insert(name.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(&home), &Value::Object(doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_names_in_order() {
        let cfg = json!({ "providers": { "anthropic": {}, "openai": {}, "nvidia": {} } });
        assert_eq!(provider_names(&cfg), ["anthropic", "openai", "nvidia"]);
    }

    #[test]
    fn load_models_enriches_with_default_and_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let pdir = dir.path().join("providers").join("openai");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join("configured-models.yaml"),
            "models:\n  gpt-4o:\n    description: cap\n  gpt-4o-mini:\n    description: mini\n",
        )
        .unwrap();
        std::fs::write(pdir.join("default-model.yaml"), "default_model: gpt-4o\n").unwrap();
        std::fs::write(
            pdir.join("model-aliases.yaml"),
            "aliases:\n  gpt4: gpt-4o\n  mini: gpt-4o-mini\n",
        )
        .unwrap();

        let models = load_models_in(dir.path(), "openai").unwrap();
        assert_eq!(models.len(), 2);
        // Declaration order is preserved.
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(models[0].description.as_deref(), Some("cap"));
        assert!(models[0].default);
        assert_eq!(models[0].aliases, ["gpt4"]);
        assert_eq!(models[1].id, "gpt-4o-mini");
        assert!(!models[1].default);
        assert_eq!(models[1].aliases, ["mini"]);
    }

    #[test]
    fn load_models_empty_for_unknown_provider() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_models_in(dir.path(), "nope").unwrap().is_empty());
    }

    #[test]
    fn upsert_model_adds_then_merges_and_sets_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut meta = Map::new();
        meta.insert("description".into(), json!("cap"));
        meta.insert("max_tokens".into(), json!(4096));

        // First upsert creates the entry and sets it default.
        let existed = upsert_model_in(dir.path(), "openai", "gpt-4o", &meta, Some("chat")).unwrap();
        assert!(!existed);
        assert_eq!(
            default_model_in(dir.path(), "openai").unwrap().as_deref(),
            Some("gpt-4o")
        );

        // Second upsert merges new fields over the existing entry.
        let mut meta2 = Map::new();
        meta2.insert("context_window".into(), json!(128000));
        let existed = upsert_model_in(dir.path(), "openai", "gpt-4o", &meta2, None).unwrap();
        assert!(existed);
        let models = load_models_in(dir.path(), "openai").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].description.as_deref(), Some("cap"));
        assert!(models[0].default);
    }

    #[test]
    fn delete_model_fixes_default_and_errors_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let empty = Map::new();
        let kind_of = |m: &str| {
            Some(
                if m.starts_with("img") {
                    "image-generation"
                } else {
                    "chat"
                }
                .to_string(),
            )
        };
        upsert_model_in(dir.path(), "openai", "a", &empty, Some("chat")).unwrap();
        upsert_model_in(
            dir.path(),
            "openai",
            "img-1",
            &empty,
            Some("image-generation"),
        )
        .unwrap();
        upsert_model_in(dir.path(), "openai", "b", &empty, None).unwrap();

        // Deleting the chat default promotes the first remaining chat model.
        delete_model_in(dir.path(), "openai", "a", kind_of).unwrap();
        assert_eq!(
            default_model_in(dir.path(), "openai").unwrap().as_deref(),
            Some("b")
        );
        // Deleting the last chat model clears the chat default only.
        delete_model_in(dir.path(), "openai", "b", kind_of).unwrap();
        assert_eq!(default_model_in(dir.path(), "openai").unwrap(), None);
        assert_eq!(
            default_models_in(dir.path(), "openai").unwrap()["image-generation"],
            "img-1"
        );
        delete_model_in(dir.path(), "openai", "img-1", kind_of).unwrap();
        assert!(default_models_in(dir.path(), "openai").unwrap().is_empty());

        assert!(delete_model_in(dir.path(), "openai", "gone", kind_of).is_err());
    }

    #[test]
    fn kind_defaults_keep_the_chat_default_where_earlier_versions_read_it() {
        let dir = tempfile::tempdir().unwrap();
        set_kind_default_in(dir.path(), "openai", "chat", Some("gpt-4o")).unwrap();
        set_kind_default_in(dir.path(), "openai", "image-generation", Some("img")).unwrap();
        let text = std::fs::read_to_string(dir.path().join("providers/openai/default-model.yaml"))
            .unwrap();
        let data: Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(
            data,
            json!({ "default_model": "gpt-4o", "defaults": { "image-generation": "img" } })
        );
        let models = default_models_in(dir.path(), "openai").unwrap();
        assert_eq!(models["chat"], "gpt-4o");
        assert_eq!(models["image-generation"], "img");

        set_kind_default_in(dir.path(), "openai", "chat", None).unwrap();
        assert_eq!(default_model_in(dir.path(), "openai").unwrap(), None);
        assert!(set_kind_default_in(dir.path(), "openai", "pictures", Some("x")).is_err());
    }

    #[test]
    fn alias_set_delete_preserves_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let pdir = dir.path().join("providers").join("openai");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join("model-aliases.yaml"),
            "aliases:\n  gpt4: gpt-4o\n",
        )
        .unwrap();

        set_model_alias_in(dir.path(), "openai", "mini", "gpt-4o-mini").unwrap();
        let aliases = model_aliases_in(dir.path(), "openai").unwrap();
        assert_eq!(aliases["gpt4"], "gpt-4o");
        assert_eq!(aliases["mini"], "gpt-4o-mini");
        // Wrapper preserved on disk.
        let raw = std::fs::read_to_string(pdir.join("model-aliases.yaml")).unwrap();
        assert!(raw.contains("aliases:"));

        delete_model_alias_in(dir.path(), "openai", "gpt4").unwrap();
        assert!(!model_aliases_in(dir.path(), "openai")
            .unwrap()
            .contains_key("gpt4"));
        assert!(delete_model_alias_in(dir.path(), "openai", "nope").is_err());
    }

    #[test]
    fn load_config_reads_json_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_config_in(dir.path()).unwrap(), json!({}));
        std::fs::write(dir.path().join("config.json"), r#"{"edit_mode":"vi"}"#).unwrap();
        assert_eq!(
            load_config_in(dir.path()).unwrap(),
            json!({ "edit_mode": "vi" })
        );
    }

    #[test]
    fn set_config_merges_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), r#"{"theme":"dark"}"#).unwrap();

        let mut updates = Map::new();
        updates.insert("edit_mode".into(), json!("vi"));
        let merged = set_config_in(dir.path(), &updates).unwrap();
        assert_eq!(merged, json!({ "theme": "dark", "edit_mode": "vi" }));
        // Persisted to disk, with the prior key preserved.
        assert_eq!(load_config_in(dir.path()).unwrap(), merged);
    }

    #[test]
    fn set_config_creates_file_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mut updates = Map::new();
        updates.insert("prompt".into(), json!("hi"));
        let merged = set_config_in(dir.path(), &updates).unwrap();
        assert_eq!(merged, json!({ "prompt": "hi" }));
    }

    #[test]
    fn save_and_delete_and_activate_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let body = json!({ "prompt": "tell a joke", "max_tokens": 256, "role": "user" });
        save_prompt_in(dir.path(), "joke", &body).unwrap();

        let prompts = load_prompts_in(dir.path()).unwrap();
        assert_eq!(prompts, vec![("joke".to_string(), body.clone())]);

        // Activating copies the body into the $$ slot.
        set_active_prompt_in(dir.path(), "joke").unwrap();
        let active = load_prompts_in(dir.path()).unwrap();
        assert!(active.iter().any(|(n, b)| n == "$$" && *b == body));

        // Delete removes just the named file.
        delete_prompt_in(dir.path(), "joke").unwrap();
        let after = load_prompts_in(dir.path()).unwrap();
        assert_eq!(
            after.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["$$"]
        );
    }

    #[test]
    fn prompt_ops_reject_bad_names_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save_prompt_in(dir.path(), "../escape", &json!({})).is_err());
        assert!(save_prompt_in(dir.path(), "a/b", &json!({})).is_err());
        assert!(save_prompt_in(dir.path(), "", &json!({})).is_err());
        assert!(delete_prompt_in(dir.path(), "nope").is_err());
        assert!(set_active_prompt_in(dir.path(), "nope").is_err());
    }

    #[test]
    fn load_prompts_reads_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let pdir = dir.path().join("prompts");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("$$.json"), r#"{"prompt":"active","role":"user"}"#).unwrap();
        std::fs::write(pdir.join("joke.json"), r#"{"prompt":"a joke"}"#).unwrap();
        std::fs::write(pdir.join("notes.txt"), "ignored").unwrap();

        let prompts = load_prompts_in(dir.path()).unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].0, "$$");
        assert_eq!(prompts[0].1["prompt"], "active");
        assert_eq!(prompts[1].0, "joke");
    }

    #[test]
    fn list_api_defs_unions_embedded_and_user() {
        let dir = tempfile::tempdir().unwrap();
        let adir = dir.path().join("apis");
        std::fs::create_dir_all(&adir).unwrap();
        std::fs::write(adir.join("custom.yaml"), "description: my api\n").unwrap();

        let defs = list_api_defs_in(dir.path()).unwrap();
        let names: Vec<&str> = defs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"messages")); // embedded
        assert!(names.contains(&"custom")); // user
        let custom = defs.iter().find(|(n, _)| n == "custom").unwrap();
        assert_eq!(custom.1.as_deref(), Some("my api"));
    }

    #[test]
    fn create_api_def_scaffolds_valid_definition() {
        let dir = tempfile::tempdir().unwrap();
        create_api_def_in(dir.path(), "mine", None, None, None).unwrap();
        let raw = load_api_raw_in(dir.path(), "mine").unwrap();
        let def: Value = serde_yaml::from_str(&raw).unwrap();
        assert_eq!(def["name"], "mine");
        // The scaffold is itself a valid definition.
        assert!(lexi_core::validate_definition(&def).is_empty());
    }

    #[test]
    fn create_api_def_from_copies_source_and_applies_overrides() {
        let dir = tempfile::tempdir().unwrap();
        create_api_def_in(
            dir.path(),
            "copy",
            Some("messages"),
            Some("/v2/x"),
            Some("a.b"),
        )
        .unwrap();
        let def: Value =
            serde_yaml::from_str(&load_api_raw_in(dir.path(), "copy").unwrap()).unwrap();
        assert_eq!(def["name"], "copy");
        assert_eq!(def["request"]["path"], "/v2/x");
        assert_eq!(def["response"]["text_extract"]["text_path"], "a.b");
    }

    #[test]
    fn save_api_raw_writes_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        // Invalid YAML is rejected before any write.
        assert!(save_api_raw_in(dir.path(), "bad", "a: [unclosed").is_err());
        assert!(!dir.path().join("apis").join("bad.yaml").exists());

        // Valid YAML writes; incomplete definition returns errors (still saved).
        let errors = save_api_raw_in(dir.path(), "wip", "name: wip\n").unwrap();
        assert!(!errors.is_empty());
        assert!(dir.path().join("apis").join("wip.yaml").exists());

        // A complete definition validates clean.
        let good = "name: ok\nrequest:\n  path: /x\n  body_schema:\n    type: object\nresponse:\n  text_extract:\n    text_path: a.b\n";
        assert!(save_api_raw_in(dir.path(), "ok", good).unwrap().is_empty());
    }

    #[test]
    fn delete_api_def_removes_user_file_or_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(delete_api_def_in(dir.path(), "nope").is_err());
        save_api_raw_in(dir.path(), "temp", "name: temp\n").unwrap();
        delete_api_def_in(dir.path(), "temp").unwrap();
        assert!(!dir.path().join("apis").join("temp.yaml").exists());
    }

    #[test]
    fn api_def_name_guard_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        assert!(create_api_def_in(dir.path(), "../evil", None, None, None).is_err());
        assert!(save_api_raw_in(dir.path(), "a/b", "name: x\n").is_err());
    }

    #[test]
    fn preview_api_def_handles_yaml_name_and_bad_input() {
        // Raw YAML of a valid scaffold composes a body.
        let good = "name: ok\nrequest:\n  path: /x\n  body_schema:\n    type: object\nresponse:\n  text_extract:\n    text_path: a.b\n";
        let out = preview_api_def(None, Some(good), "hi", "m", "user").unwrap();
        assert_eq!(out["valid"], true);
        assert!(out.get("body").is_some());

        // By name: a shipped definition resolves via the embedded copy.
        let out = preview_api_def(Some("messages"), None, "hi", "m", "user").unwrap();
        assert_eq!(out["name"], "messages");

        // Unparseable YAML -> valid:false result, not an error.
        let out = preview_api_def(None, Some("a: [bad"), "hi", "m", "user").unwrap();
        assert_eq!(out["valid"], false);

        // Neither name nor yaml -> error.
        assert!(preview_api_def(None, None, "hi", "m", "user").is_err());
    }

    #[test]
    fn load_api_raw_prefers_embedded_then_user() {
        let dir = tempfile::tempdir().unwrap();
        // Shipped definition resolves to embedded text even without a user file.
        assert!(load_api_raw_in(dir.path(), "messages")
            .unwrap()
            .contains("description"));
        // Custom definition reads the user file.
        let adir = dir.path().join("apis");
        std::fs::create_dir_all(&adir).unwrap();
        std::fs::write(adir.join("custom.yaml"), "description: my api\n").unwrap();
        assert_eq!(
            load_api_raw_in(dir.path(), "custom").unwrap(),
            "description: my api\n"
        );
        assert!(load_api_raw_in(dir.path(), "nope").is_err());
    }

    #[test]
    fn provider_names_empty_when_absent() {
        assert!(provider_names(&json!({})).is_empty());
    }

    #[test]
    fn delete_config_removes_only_present_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Map::new();
        m.insert("a".into(), json!("1"));
        m.insert("b".into(), json!("2"));
        set_config_in(dir.path(), &m).unwrap();
        assert!(delete_config_in(dir.path(), "a").unwrap());
        assert!(!delete_config_in(dir.path(), "a").unwrap());
        assert_eq!(load_config_in(dir.path()).unwrap(), json!({"b": "2"}));
    }

    #[test]
    fn provider_add_rename_remove() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("providers")).unwrap();
        std::fs::write(
            providers_config_path(home),
            "providers:\n  first:\n    url: u1\n  last:\n    url: u3\ndefaults:\n  max_tokens: 1\n",
        )
        .unwrap();
        let mut cfg = Map::new();
        cfg.insert("url".into(), json!("u2"));
        add_provider_in(home, "mid", &cfg).unwrap();
        assert!(add_provider_in(home, "mid", &cfg).is_err());
        assert!(add_provider_in(home, "../x", &cfg).is_err());

        set_api_key_in(home, "mid", None, "mid-key").unwrap();
        rename_provider_in(home, "mid", "middle").unwrap();
        // Key files Lexi saved follow the provider to its new name, and the
        // config doesn't list them.
        assert!(home.join("providers/middle/api-key").exists());
        assert!(entry_key(home, "middle").is_null());
        rename_provider_in(home, "first", "alpha").unwrap();
        assert!(rename_provider_in(home, "nope", "x").is_err());
        assert!(rename_provider_in(home, "alpha", "last").is_err());

        let doc = read_providers_doc(home).unwrap();
        let names: Vec<&String> = doc["providers"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["alpha", "last", "middle"]);
        assert_eq!(doc["defaults"]["max_tokens"], 1); // other sections kept

        remove_provider_in(home, "middle").unwrap();
        assert!(!home.join("providers/middle").exists());
        assert!(remove_provider_in(home, "middle").is_err());
    }

    #[test]
    fn edit_api_def_patches_custom_and_refuses_shipped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        create_api_def_in(home, "mine", Some("messages"), None, None).unwrap();
        edit_api_def_in(
            home,
            "mine",
            Some("/v2/messages"),
            Some("content[0].text"),
            &[("stop".into(), "stop_reason".into())],
        )
        .unwrap();
        let def = load_api_def_from(home, "mine").unwrap();
        assert_eq!(def["request"]["path"], "/v2/messages");
        assert_eq!(
            def["response"]["text_extract"]["text_path"],
            "content[0].text"
        );
        assert!(def["response"]["text_extract"].get("items_path").is_none());
        assert_eq!(def["response"]["metadata"]["stop"], "stop_reason");

        assert!(edit_api_def_in(home, "messages", Some("/x"), None, &[]).is_err());
        assert!(edit_api_def_in(home, "missing", Some("/x"), None, &[]).is_err());
    }

    fn write_providers(home: &Path, yaml: &str) {
        std::fs::create_dir_all(home.join("providers")).unwrap();
        std::fs::write(providers_config_path(home), yaml).unwrap();
    }

    fn entry_key(home: &Path, provider: &str) -> Value {
        read_providers_doc(home).unwrap()["providers"][provider]["api_key"].clone()
    }

    fn lookup_in(home: &Path, cfg: &Value) -> Lookup {
        keystore::lookup(home, &key_sources(cfg).unwrap())
    }

    /// The key a request for `provider` (with `choice`) uses, read through
    /// the provider's entry as configured.
    fn key_for(
        home: &Path,
        provider: &str,
        choice: Option<&str>,
    ) -> Result<Option<String>, HostError> {
        let entry = resolve_template(&Value::Object(
            raw_entry(&mut read_providers_doc(home).unwrap(), provider).unwrap(),
        ))
        .unwrap();
        with_key_sync(choice.map(String::from), || {
            lookup_api_key_in(home, provider, &entry, true).map(|l| l.key)
        })
    }

    #[test]
    fn the_provider_key_shorthand_is_split_and_checked() {
        let split = |p: &str, k: Option<&str>| split_key_arg(p, k);
        assert_eq!(split("openai", None).unwrap(), ("openai".into(), None));
        assert_eq!(
            split("openai#work", None).unwrap(),
            ("openai".into(), Some("work".into()))
        );
        assert_eq!(
            split("openai", Some("work")).unwrap(),
            ("openai".into(), Some("work".into()))
        );
        assert!(split("openai#work", Some("work")).is_ok());
        let err = split("openai#work", Some("home")).unwrap_err().to_string();
        assert!(err.contains("name different keys"), "{err}");
        assert!(split("openai#no good", None).is_err());
        assert!(check_provider_name("a#b").is_err());
    }

    #[test]
    fn a_named_key_is_chosen_per_request_or_by_default_key() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::env::set_var("LEXI_NK_UNNAMED", "k-unnamed");
        std::env::set_var("LEXI_NK_WORK", "k-work");
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api: chat-completions\n    api_key: [{env: LEXI_NK_UNNAMED}]\n    keys:\n      work: {api_key: [{env: LEXI_NK_WORK}]}\n      personal: {}\n",
        );
        assert_eq!(
            key_for(home, "p", None).unwrap().as_deref(),
            Some("k-unnamed")
        );
        assert_eq!(
            key_for(home, "p", Some("work")).unwrap().as_deref(),
            Some("k-work")
        );
        // A named key with no sources is its saved key alone.
        assert_eq!(key_for(home, "p", Some("personal")).unwrap(), None);
        let saved = set_api_key_in(home, "p#personal", None, "k-personal").unwrap();
        assert_eq!(saved.location, "providers/p/keys/personal");
        assert!(!saved.added);
        assert_eq!(
            key_for(home, "p", Some("personal")).unwrap().as_deref(),
            Some("k-personal")
        );
        // The unnamed key's saved key is a different one.
        assert!(!home.join("providers/p/api-key").exists());
        let err = key_for(home, "p", Some("nope")).unwrap_err().to_string();
        assert!(
            err.contains("p has no key named \"nope\" (its keys: default, work, personal)"),
            "{err}"
        );

        // default_key applies when a request names none; `default` is the
        // unnamed key.
        set_default_key_in(home, "p", Some("work")).unwrap();
        assert_eq!(key_for(home, "p", None).unwrap().as_deref(), Some("k-work"));
        assert_eq!(
            key_for(home, "p", Some("default")).unwrap().as_deref(),
            Some("k-unnamed")
        );
        assert!(set_default_key_in(home, "p", Some("nope")).is_err());
        let names = key_names_in(home, "p").unwrap();
        assert_eq!(names["default_key"], "work");
        let rows: Vec<(&str, bool, &str)> = names["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["name"].as_str().unwrap(),
                    r["default"] == true,
                    r["source"].as_str().unwrap_or("-"),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("default", false, "env"),
                ("work", true, "env"),
                ("personal", false, "saved")
            ]
        );
        set_default_key_in(home, "p", None).unwrap();
        assert!(read_providers_doc(home).unwrap()["providers"]["p"]
            .get("default_key")
            .is_none());

        // A named key covers every API: --api with one is refused.
        assert!(set_api_key_in(home, "p#work", Some("embeddings"), "x").is_err());
        std::env::remove_var("LEXI_NK_UNNAMED");
        std::env::remove_var("LEXI_NK_WORK");
    }

    #[test]
    fn named_keys_are_added_renamed_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api: chat-completions\n",
        );
        // Saving under a new name creates it.
        let saved = set_api_key_in(home, "p#client-a", None, "k-a").unwrap();
        assert!(saved.added);
        let doc = read_providers_doc(home).unwrap();
        assert_eq!(
            doc["providers"]["p"]["keys"]["client-a"],
            serde_json::json!({})
        );
        assert!(add_named_key_in(home, "p", "client-b").unwrap());
        assert!(!add_named_key_in(home, "p", "client-b").unwrap());
        assert!(add_named_key_in(home, "p", "default").is_err());
        add_key_source_in(
            home,
            "p#client-b",
            None,
            &serde_json::json!({"env": "B"}),
            false,
        )
        .unwrap();
        assert_eq!(
            read_providers_doc(home).unwrap()["providers"]["p"]["keys"]["client-b"]["api_key"],
            serde_json::json!([{"env": "B"}])
        );
        set_default_key_in(home, "p", Some("client-a")).unwrap();

        // Renaming moves its saved key and the default along.
        rename_named_key_in(home, "p", "client-a", "acme").unwrap();
        let entry = &read_providers_doc(home).unwrap()["providers"]["p"];
        assert_eq!(entry["default_key"], "acme");
        assert!(entry["keys"].get("client-a").is_none());
        assert!(!home.join("providers/p/keys/client-a").exists());
        assert_eq!(key_for(home, "p", None).unwrap().as_deref(), Some("k-a"));
        assert!(rename_named_key_in(home, "p", "acme", "client-b").is_err());

        // Removing takes its saved key and the default with it.
        remove_named_key_in(home, "p", "acme").unwrap();
        assert!(!home.join("providers/p/keys/acme").exists());
        let entry = &read_providers_doc(home).unwrap()["providers"]["p"];
        assert!(entry.get("default_key").is_none());
        remove_named_key_in(home, "p", "client-b").unwrap();
        assert!(read_providers_doc(home).unwrap()["providers"]["p"]
            .get("keys")
            .is_none());
        assert!(remove_named_key_in(home, "p", "client-b").is_err());
    }

    #[test]
    fn an_apis_own_key_gives_way_to_a_chosen_named_key() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::env::set_var("LEXI_NK_API", "k-api");
        std::env::set_var("LEXI_NK_TEAM", "k-team");
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    apis:\n      - api: chat-completions\n      - {api: embeddings, api_key: [{env: LEXI_NK_API}]}\n    keys:\n      team: {api_key: [{env: LEXI_NK_TEAM}]}\n    default_key: team\n",
        );
        let entry = resolve_template(&Value::Object(
            raw_entry(&mut read_providers_doc(home).unwrap(), "p").unwrap(),
        ))
        .unwrap();
        let bindings = lexi_core::provider_bindings(&entry);
        let emb = binding_config(
            &entry,
            bindings.iter().find(|b| b["api"] == "embeddings").unwrap(),
        )
        .unwrap();
        let chat = binding_config(
            &entry,
            bindings
                .iter()
                .find(|b| b["api"] == "chat-completions")
                .unwrap(),
        )
        .unwrap();
        let key = |cfg: &Value, choice: Option<&str>| {
            with_key_sync(choice.map(String::from), || {
                lookup_api_key_in(home, "p", cfg, true).unwrap().key
            })
        };
        // No choice: the API's own key wins over default_key; other APIs use it.
        assert_eq!(key(&emb, None).as_deref(), Some("k-api"));
        assert_eq!(key(&chat, None).as_deref(), Some("k-team"));
        // A chosen key is used for every API.
        assert_eq!(key(&emb, Some("team")).as_deref(), Some("k-team"));
        std::env::remove_var("LEXI_NK_API");
        std::env::remove_var("LEXI_NK_TEAM");
    }

    #[test]
    fn key_sources_are_tried_in_order_and_only_a_missing_key_moves_on() {
        use crate::keystore::Outcome;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("empty.txt"), "\n").unwrap();
        std::fs::write(home.join("k.txt"), "from-file\n").unwrap();
        std::fs::write(home.join("later.txt"), "later\n").unwrap();
        let cfg = json!({"api_key": [
            "${LEXI_TEST_SURELY_UNSET_VAR}",
            {"file": "nothing-here"},
            {"file": "empty.txt"},
            {"file": "k.txt"},
            {"file": "later.txt"},
        ]});
        let found = lookup_in(home, &cfg);
        assert_eq!(found.key.as_deref(), Some("from-file"));
        let states: Vec<&str> = found.checked.iter().map(|(_, o)| o.state()).collect();
        assert_eq!(
            states,
            ["missing", "missing", "missing", "used", "not checked"]
        );
        assert_eq!(
            found.summary(),
            "$LEXI_TEST_SURELY_UNSET_VAR (not set), nothing-here (no file), empty.txt (empty), k.txt, later.txt"
        );

        // A set variable comes first when it's listed first.
        let env_first = json!({"api_key": [{"env": "PATH"}, {"file": "k.txt"}]});
        assert_eq!(
            lookup_in(home, &env_first).used().map(Source::kind),
            Some("env")
        );

        // An unreadable file stops the lookup — the file after it isn't used.
        std::fs::create_dir(home.join("a-folder")).unwrap();
        let unreadable = json!({"api_key": [{"file": "a-folder"}, {"file": "k.txt"}]});
        let found = lookup_in(home, &unreadable);
        assert_eq!(found.key, None);
        assert!(found.failure().is_some());
        assert_eq!(found.checked[1].1, Outcome::NotChecked);

        // The older forms still read; bad entries are config errors, and a
        // keychain entry says what to use instead.
        assert_eq!(
            lookup_in(home, &json!({"api_key": "literal"}))
                .key
                .as_deref(),
            Some("literal")
        );
        assert!(key_sources(&json!({"api_key": {"vault": "x"}})).is_err());
        assert!(key_sources(&json!({"api_key": [{"env": ""}]})).is_err());
        assert!(key_sources(&json!({})).unwrap().is_empty());
        let Err(e) = key_sources(&json!({"api_key": [{"keychain": "openai"}]})) else {
            panic!("a keychain entry is an error");
        };
        assert!(e.to_string().contains("list it as `saved`"), "{e}");

        // The marker `saved` and commands parse; a command needs a program.
        assert!(
            key_sources(&json!({"api_key": ["saved", {"command": ["echo", "k"]}]})).unwrap()
                == [
                    Source::Saved(String::new()),
                    Source::Command(vec!["echo".into(), "k".into()])
                ]
        );
        assert!(key_sources(&json!({"api_key": {"command": []}})).is_err());
        assert!(key_sources(&json!({"api_key": {"command": "echo k"}})).is_err());
    }

    #[test]
    fn the_saved_key_comes_after_the_listed_sources_unless_marked() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("providers/p")).unwrap();
        std::fs::write(home.join("providers/p/api-key"), "saved-key\n").unwrap();
        std::fs::write(home.join("mine.key"), "mine\n").unwrap();
        let found = |cfg: Value| lookup_api_key_in(home, "p", &cfg, true).unwrap();

        // Last by default: a listed source with a key wins.
        let last = found(json!({"api_key": [{"file": "mine.key"}]}));
        assert_eq!(last.key.as_deref(), Some("mine"));
        assert!(last.checked[1].0 == Source::Saved("providers/p/api-key".into()));
        assert_eq!(last.checked[1].1.state(), "not checked");
        // Marked first, it wins.
        let first = found(json!({"api_key": ["saved", {"file": "mine.key"}]}));
        assert_eq!(first.key.as_deref(), Some("saved-key"));
        assert_eq!(first.used().map(Source::kind), Some("saved"));
        // An empty list still has the saved key; none saved is "missing".
        assert_eq!(
            found(json!({"api_key": []})).key.as_deref(),
            Some("saved-key")
        );
        let none = lookup_api_key_in(home, "q", &json!({}), true).unwrap();
        assert_eq!(none.summary(), "providers/q/api-key (none saved)");
        // A model's own api_key has no saved key.
        let model =
            json!({"api_key": [], KEY_SCOPE: {"model": "providers/p/configured-models.yaml"}});
        assert!(found(model).checked.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_command_prints_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(home, "providers: {}\n");
        let sh = |script: &str| json!({"command": ["sh", "-c", script]});
        let found = |cfg: Value, run: bool| lookup_api_key_in(home, "p", &cfg, run).unwrap();

        let ok = found(json!({"api_key": [sh("echo '  cmd-key  '")]}), true);
        assert_eq!(ok.key.as_deref(), Some("cmd-key"));
        assert_eq!(ok.used().map(Source::kind), Some("command"));
        // Remembered: a listing (commands not run) knows it now.
        let listed = found(json!({"api_key": [sh("echo '  cmd-key  '")]}), false);
        assert_eq!(listed.key.as_deref(), Some("cmd-key"));
        keystore::forget_commands();

        // Not run: it decides the lookup without a key.
        let not_run = found(
            json!({"api_key": [sh("echo other"), {"env": "PATH"}]}),
            false,
        );
        assert_eq!(not_run.key, None);
        assert_eq!(not_run.source().map(Source::kind), Some("command"));
        assert_eq!(not_run.checked[1].1.state(), "not checked");

        // Printing nothing moves on; failing stops, with stderr (not stdout).
        let empty = found(json!({"api_key": [sh("true"), {"env": "PATH"}]}), true);
        assert_eq!(
            empty.checked[0].1.note().as_deref(),
            Some("printed nothing")
        );
        assert_eq!(empty.used().map(Source::kind), Some("env"));
        let failed = found(
            json!({"api_key": [sh("echo secret; echo 'locked vault' >&2; exit 3"), {"env": "PATH"}]}),
            true,
        );
        let (_, err) = failed.failure().unwrap();
        assert_eq!(err, "`sh` exited with status 3: locked vault");
        assert_eq!(failed.key, None);
        let missing = found(
            json!({"api_key": {"command": ["lexi-no-such-program-x"]}}),
            true,
        );
        assert!(missing
            .failure()
            .unwrap()
            .1
            .starts_with("couldn't run `lexi-no-such-program-x`"));
        let slow = found(json!({"api_key": [sh("sleep 5")]}), true);
        assert!(slow
            .failure()
            .unwrap()
            .1
            .contains("didn't finish within 2 seconds"));

        // Not from a config other users can change.
        use std::os::unix::fs::PermissionsExt;
        let cfg_file = providers_config_path(home);
        std::fs::set_permissions(&cfg_file, std::fs::Permissions::from_mode(0o666)).unwrap();
        let refused = found(json!({"api_key": [sh("echo k2")]}), true);
        assert!(refused.failure().unwrap().1.contains("chmod go-w"));
        std::fs::set_permissions(&cfg_file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            found(json!({"api_key": [sh("echo k2")]}), true)
                .key
                .as_deref(),
            Some("k2")
        );
        keystore::forget_commands();
    }

    #[test]
    fn saving_goes_to_the_saved_key_never_a_listed_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api_key: ${LEXI_TEST_SURELY_UNSET_VAR}\n  q:\n    url: u\n    api_key: sk-literal\n  m:\n    url: u\n    api_key: [{file: my-keys/m.key}]\n",
        );
        let before = std::fs::read_to_string(providers_config_path(home)).unwrap();

        // The provider's key file; the config doesn't change.
        let saved = set_api_key_in(home, "p", None, " k1 ").unwrap();
        assert_eq!(
            (saved.location.as_str(), saved.added),
            ("providers/p/api-key", false)
        );
        assert_eq!(saved.used.as_deref(), Some("providers/p/api-key"));
        assert_eq!(
            std::fs::read_to_string(home.join("providers/p/api-key")).unwrap(),
            "k1\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.join("providers/p/api-key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(!set_api_key_in(home, "p", None, "k2").unwrap().added);

        // A listed file is yours: never written, and it still comes first.
        std::fs::create_dir_all(home.join("my-keys")).unwrap();
        std::fs::write(home.join("my-keys/m.key"), "mine\n").unwrap();
        let mine = set_api_key_in(home, "m", None, "k4").unwrap();
        assert_eq!(mine.location, "providers/m/api-key");
        assert_eq!(mine.used.as_deref(), Some("my-keys/m.key"));
        assert_eq!(
            std::fs::read_to_string(home.join("my-keys/m.key")).unwrap(),
            "mine\n"
        );
        let after_p_m = std::fs::read_to_string(providers_config_path(home)).unwrap();
        assert_eq!(after_p_m, before);

        // A key written in the config is taken out.
        let q = set_api_key_in(home, "q", None, "k3").unwrap();
        assert!(q.added);
        assert_eq!(entry_key(home, "q"), json!([]));
        assert!(set_api_key_in(home, "q", None, "  ").is_err());
        assert!(set_api_key_in(home, "nope", None, "k").is_err());

        // Remove deletes the saved key only.
        assert!(remove_api_key_in(home, "m", None).unwrap());
        assert!(!remove_api_key_in(home, "m", None).unwrap());
        assert!(home.join("my-keys/m.key").exists());
    }

    #[test]
    fn sources_can_be_added_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(home, "providers:\n  openai:\n    template: openai\n");
        let kinds = |v: &Value| -> Vec<String> {
            v["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["from"].as_str().unwrap().to_string())
                .collect()
        };
        let listed = key_source_list_in(home, "openai", None).unwrap();
        assert_eq!(
            kinds(&listed),
            ["$OPENAI_API_KEY", "providers/openai/api-key"]
        );
        assert_eq!(listed["sources"][1]["implicit"], true);

        let after =
            add_key_source_in(home, "openai", None, &json!({"file": "~/k"}), false).unwrap();
        assert_eq!(kinds(&after)[1], "~/k");
        let after = add_key_source_in(home, "openai", None, &json!("saved"), true).unwrap();
        assert_eq!(kinds(&after)[0], "providers/openai/api-key");
        assert!(after["sources"][0].get("implicit").is_none());
        assert_eq!(
            entry_key(home, "openai"),
            json!(["saved", {"env": "OPENAI_API_KEY"}, {"file": "~/k"}])
        );
        // Adding one that's listed changes nothing; with first it moves.
        add_key_source_in(home, "openai", None, &json!({"file": "~/k"}), false).unwrap();
        add_key_source_in(home, "openai", None, &json!({"file": "~/k"}), true).unwrap();
        assert_eq!(entry_key(home, "openai")[0], json!({"file": "~/k"}));
        // A key itself isn't a source.
        assert!(add_key_source_in(home, "openai", None, &json!("sk-123"), false).is_err());

        assert!(remove_key_source_in(home, "openai", None, &json!("saved")).unwrap());
        assert!(
            remove_key_source_in(home, "openai", None, &json!({"env": "OPENAI_API_KEY"})).unwrap()
        );
        assert!(
            !remove_key_source_in(home, "openai", None, &json!({"env": "OPENAI_API_KEY"})).unwrap()
        );
        assert_eq!(entry_key(home, "openai"), json!([{"file": "~/k"}]));

        // An API that used the provider's key gets a list of its own.
        assert!(
            remove_key_source_in(home, "openai", Some("image-generations"), &json!("saved"))
                .is_err()
        );
        let own = add_key_source_in(
            home,
            "openai",
            Some("image-generations"),
            &json!({"command": ["op", "read", "op://x"]}),
            false,
        )
        .unwrap();
        assert_eq!(own["own"], true);
        assert_eq!(
            kinds(&own),
            [
                "the command `op read op://x`",
                "providers/openai/api-key-image-generations"
            ]
        );
    }

    #[test]
    fn an_api_can_have_its_own_key() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api_key: {file: main.key}\n    apis:\n      - api: chat-completions\n      - api: embeddings\n        for: [kind:embedding]\n",
        );
        std::fs::write(home.join("main.key"), "main-key\n").unwrap();

        // Without a list of its own an API uses the provider's key.
        assert!(!key_scope(home, "p", Some("embeddings"), false).unwrap().own);
        assert!(remove_api_key_in(home, "p", Some("embeddings")).is_err());
        assert!(key_scope(home, "p", Some("nope"), false).is_err());

        let saved = set_api_key_in(home, "p", Some("embeddings"), "emb-key").unwrap();
        assert!(saved.added);
        assert_eq!(saved.location, "providers/p/api-key-embeddings");
        let doc = read_providers_doc(home).unwrap();
        let entry = &doc["providers"]["p"];
        assert_eq!(entry["apis"][1]["api_key"], json!(["saved"]));
        assert_eq!(entry["apis"][1]["for"], json!(["kind:embedding"]));
        assert_eq!(entry["api_key"], json!({"file": "main.key"}));
        assert!(
            !set_api_key_in(home, "p", Some("embeddings"), "emb-key")
                .unwrap()
                .added
        );

        // The binding's effective config carries its own list and saved key,
        // not the provider's.
        let eff = binding_config(entry, &entry["apis"][1]).unwrap();
        let found = lookup_api_key_in(home, "p", &eff, true).unwrap();
        assert_eq!(found.key.as_deref(), Some("emb-key"));
        assert!(found.used() == Some(&Source::Saved("providers/p/api-key-embeddings".into())));
        let chat = binding_config(entry, &entry["apis"][0]).unwrap();
        assert_eq!(
            lookup_api_key_in(home, "p", &chat, true)
                .unwrap()
                .key
                .as_deref(),
            Some("main-key")
        );
    }

    #[test]
    fn an_older_config_gets_its_key_file_listed_first() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  openai:\n    template: openai\n  nvidia:\n    template: nvidia\n  mine:\n    url: u\n    api_key: [{env: X}]\n",
        );
        std::fs::create_dir_all(home.join("providers/openai")).unwrap();
        std::fs::write(home.join("providers/openai/api-key"), "k\n").unwrap();
        std::fs::create_dir_all(home.join("providers/mine")).unwrap();
        std::fs::write(home.join("providers/mine/api-key"), "k\n").unwrap();
        upgrade_key_sources(home);
        assert_eq!(
            entry_key(home, "openai"),
            json!(["saved", {"env": "OPENAI_API_KEY"}])
        );
        // No key file, or already the new form: left alone.
        assert!(entry_key(home, "nvidia").is_null());
        assert_eq!(entry_key(home, "mine"), json!([{"env": "X"}]));
        let before = std::fs::read_to_string(providers_config_path(home)).unwrap();
        upgrade_key_sources(home);
        assert_eq!(
            std::fs::read_to_string(providers_config_path(home)).unwrap(),
            before
        );
    }

    #[test]
    fn keychain_entries_become_saved() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  openai:\n    template: openai\n    api_key: [{keychain: openai}, {env: OPENAI_API_KEY}]\n    apis:\n      - api: image-generations\n        api_key: {keychain: openai/image-generations}\n  both:\n    url: u\n    api_key: [{file: providers/both/api-key}, {keychain: both}, {env: B}]\n",
        );
        upgrade_key_sources(home);
        assert_eq!(
            entry_key(home, "openai"),
            json!(["saved", {"env": "OPENAI_API_KEY"}])
        );
        let doc = read_providers_doc(home).unwrap();
        assert_eq!(
            doc["providers"]["openai"]["apis"][0]["api_key"],
            json!(["saved"])
        );
        // Listed once, where the file already was.
        assert_eq!(entry_key(home, "both"), json!(["saved", {"env": "B"}]));
        let resolved = resolve_template(&doc["providers"]["openai"]).unwrap();
        assert!(key_sources(&resolved).is_ok());
    }

    #[test]
    fn lexis_own_key_files_come_out_of_the_lists() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // What 0.3.0 wrote when it saved keys.
        write_providers(
            home,
            "providers:\n  openai:\n    template: openai\n    api_key: [{env: OPENAI_API_KEY}, {file: providers/openai/api-key}]\n    apis:\n      - api: image-generations\n        api_key: [{file: providers/openai/api-key-image-generations}]\n  anthropic:\n    template: anthropic\n    api_key: [{file: providers/anthropic/api-key}, {env: ANTHROPIC_API_KEY}]\n  mine:\n    url: u\n    api_key: [{env: X}, {file: providers/mine/api-key}, {file: ~/elsewhere.key}]\n",
        );
        upgrade_key_sources(home);
        let doc = read_providers_doc(home).unwrap();
        // Equal to the template's list now: taken out.
        assert!(doc["providers"]["openai"].get("api_key").is_none());
        // An API's own list keeps saying it has its own key.
        assert_eq!(
            doc["providers"]["openai"]["apis"][0]["api_key"],
            json!(["saved"])
        );
        // First, or followed by another source: it becomes the marker.
        assert_eq!(
            entry_key(home, "anthropic"),
            json!(["saved", {"env": "ANTHROPIC_API_KEY"}])
        );
        assert_eq!(
            entry_key(home, "mine"),
            json!([{"env": "X"}, "saved", {"file": "~/elsewhere.key"}])
        );
        // Once only.
        let before = std::fs::read_to_string(providers_config_path(home)).unwrap();
        upgrade_key_sources(home);
        assert_eq!(
            std::fs::read_to_string(providers_config_path(home)).unwrap(),
            before
        );
    }

    #[test]
    fn deleting_an_alias_keeps_the_others_in_file_order() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for (a, m) in [("one", "m1"), ("two", "m2"), ("three", "m3")] {
            set_model_alias_in(home, "p", a, m).unwrap();
        }
        delete_model_alias_in(home, "p", "one").unwrap();
        let names: Vec<String> = model_aliases_in(home, "p")
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(names, ["two", "three"]);
    }
}
