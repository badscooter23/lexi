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
            let template = provider_template(name)?;
            let mut merged = lexi_core::merge_provider_template(&template, entry);
            merged["template"] = Value::from(name); // keep where it came from
                                                    // The template's api_key names the provider's variable, which
                                                    // isn't one of its keys (docs/key-storage-design.md §12.2 rule
                                                    // 5); the entry's own api_key is its unnamed key's.
            let variable = keystore::parse_sources(template.get("api_key"))
                .ok()
                .and_then(|list| {
                    list.into_iter().find_map(|s| match s {
                        Source::Env(v) => Some(v),
                        _ => None,
                    })
                });
            if let Some(o) = merged.as_object_mut() {
                o.shift_remove("api_key");
                if let Some(k) = entry.get("api_key") {
                    o.insert("api_key".into(), k.clone());
                }
                if let Some(v) = variable {
                    o.insert(VARIABLE.into(), Value::from(v));
                }
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
    if let Some(b) = over.as_object_mut() {
        b.shift_remove("api_key");
    }
    let mut eff = deep_merge(&deep_merge(&api_def, &provider), &over);
    eff["api"] = Value::from(api_name);
    // Keys belong to the provider (docs/key-storage-design.md §12.2 rule 6):
    // an API's own api_key isn't used, and the lookup says so.
    if binding.get("api_key").is_some() {
        eff[OWN_API_KEY] = Value::from(format!("the {api_name} API"));
    }
    Ok(eff)
}

/// The `api_key` entry that marks where the saved key is looked in.
pub const SAVED_SOURCE: &str = keystore::SAVED;

/// In an effective config: which key it carries — `{key: NAME}` (`""` for
/// the unnamed key) or `{variable: VAR}` (the provider's variable). Set by
/// the key lookup (`docs/key-storage-design.md` §12.4).
pub(crate) const KEY_SCOPE: &str = "_key_scope";

/// In a resolved provider entry: the provider's variable, the one its
/// template's `api_key` names (`OPENAI_API_KEY`). It isn't one of the
/// provider's keys (§12.2 rule 5).
const VARIABLE: &str = "_variable";

/// In an effective config: an API binding or configured model that still
/// has an `api_key` of its own, which Lexi no longer uses (§12.2 rule 6).
pub(crate) const OWN_API_KEY: &str = "_own_api_key";

/// The key choice that means the provider's variable, on its own: what
/// `providers status` checks it with. Not a key name.
pub const VARIABLE_KEY: &str = "$";

tokio::task_local! {
    /// The key chosen for one operation by [`with_key`].
    static KEY_CHOICE: Option<String>;
}

/// Run `fut` with `key` as the key its requests use (`--key work`, Python
/// `key="work"`, the `provider#key` shorthand): one of the provider's keys
/// by name (`""` for the unnamed one, [`VARIABLE_KEY`] for its variable);
/// `None` changes nothing (the variable when it's set, else the default).
/// Every key lookup `fut` makes is covered — requests, uploads, listings,
/// job checks (`docs/key-storage-design.md` §12.4).
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

/// A key's name: letters, digits, `-` and `_`, at most 32 — or `""`, the
/// unnamed key (`openai#`). `default` isn't one: it was the unnamed key
/// before §12.
fn check_key_name(name: &str) -> Result<(), HostError> {
    if name == "default" {
        return Err(HostError::Invalid(
            "`default` isn't a key name: a request that names no key uses the default one, and \
             a key saved before keys had names is written with nothing after the # (`openai#`)"
                .into(),
        ));
    }
    let ok = name.len() <= 32
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

/// A name a new key may take: [`check_key_name`], and not empty (only a key
/// saved before names existed is unnamed).
fn check_new_key_name(name: &str) -> Result<(), HostError> {
    if name.is_empty() {
        return Err(HostError::Invalid(
            "a key needs a name: letters, digits, - and _ (at most 32)".into(),
        ));
    }
    check_key_name(name)
}

/// A provider argument and a key: `openai#work` is `openai` with the key
/// `work`, and `openai#` the unnamed key (`""`); `key` (`--key`, `key=`)
/// may name it too, but not differently. Returns the provider and the key,
/// if any.
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

/// How a key is written after its provider: `#work`, and `#` for the
/// unnamed key.
pub fn key_label(name: &str) -> String {
    format!("#{name}")
}

/// The names of the provider's named keys in `cfg` (a provider entry or an
/// effective config).
fn named_key_names(cfg: &Value) -> Vec<String> {
    cfg.get("keys")
        .and_then(Value::as_object)
        .map(|k| k.keys().cloned().collect())
        .unwrap_or_default()
}

/// The provider's variable in `cfg` (a resolved entry or an effective
/// config), if its template names one.
fn provider_variable(cfg: &Value) -> Option<String> {
    cfg.get(VARIABLE).and_then(Value::as_str).map(String::from)
}

/// Whether a variable is set to something (an empty one is unset).
fn variable_set(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| !v.trim().is_empty())
}

/// Whether `provider` has an unnamed key — one saved before keys had names
/// (§12.2 rule 1): its entry lists `api_key` sources, or its own slot holds
/// a saved key. A store that can't be read counts as having one, so the
/// lookup reports the error.
fn has_unnamed_key(store: &Store, provider: &str, cfg: &Value) -> bool {
    cfg.get("api_key").is_some() || store.read(&Slot::new(provider, None)) != Ok(None)
}

/// The provider's keys, in order: the unnamed one (`""`) if it has one,
/// then its named keys.
fn key_list(store: &Store, provider: &str, cfg: &Value) -> Vec<String> {
    let mut names = Vec::new();
    if has_unnamed_key(store, provider, cfg) {
        names.push(String::new());
    }
    names.extend(named_key_names(cfg));
    names
}

/// `#, #work ★, #personal` — the keys for a message, the default marked.
fn keys_phrase(keys: &[String], default: Option<&str>) -> String {
    if keys.is_empty() {
        return "none".into();
    }
    keys.iter()
        .map(|k| {
            let star = if Some(k.as_str()) == default {
                " ★"
            } else {
                ""
            };
            format!("{}{star}", key_label(k))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn unknown_key(provider: &str, name: &str, keys: &[String]) -> HostError {
    if name.is_empty() {
        return HostError::Invalid(format!(
            "{provider} has no unnamed key (`{provider}#` is a key saved before keys had \
             names); its keys: {}",
            keys_phrase(keys, None)
        ));
    }
    HostError::Invalid(format!(
        "{provider} has no key named {name:?} (its keys: {})",
        keys_phrase(keys, None)
    ))
}

/// Why a request that names no key has nothing to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoDefault {
    /// The provider has no key.
    NoKey,
    /// Several keys, and none is the default.
    NoDefaultKey,
}

impl NoDefault {
    fn reason(self) -> &'static str {
        match self {
            NoDefault::NoKey => "no key",
            NoDefault::NoDefaultKey => "no default key",
        }
    }
}

/// The ★ default among `keys` (§12.2 rule 3): the only key; else the one
/// `default_key` names (`""` the unnamed key; null: none chosen); with no
/// `default_key`, the unnamed key when there is one (as before §12).
fn default_of(cfg: &Value, keys: &[String]) -> Result<String, NoDefault> {
    if keys.is_empty() {
        return Err(NoDefault::NoKey);
    }
    if keys.len() == 1 {
        return Ok(keys[0].clone());
    }
    match cfg.get("default_key") {
        Some(Value::String(d)) if keys.contains(d) => Ok(d.clone()),
        None if keys.iter().any(String::is_empty) => Ok(String::new()),
        _ => Err(NoDefault::NoDefaultKey),
    }
}

/// The "not ready" error for a request that names no key and has nothing
/// to use.
fn not_ready(provider: &str, why: NoDefault, keys: &[String], var: Option<&str>) -> HostError {
    let or_var = var.map(|v| format!(", or set ${v}")).unwrap_or_default();
    HostError::MissingApiKey(match why {
        NoDefault::NoKey => format!(
            "{provider} has no key: add one with `lexi providers api-key set {provider}#NAME`{or_var}"
        ),
        NoDefault::NoDefaultKey => format!(
            "{provider} has several keys and no default: name one, like `{provider}{}`, or \
             choose one with `lexi providers keys default {provider} NAME`{or_var} (its keys: {})",
            key_label(keys.iter().find(|k| !k.is_empty()).unwrap_or(&keys[0])),
            keys_phrase(keys, None)
        ),
    })
}

/// `cfg` with the key this operation uses (§12.4): the key chosen with
/// [`with_key`] (its sources as `api_key`); else the provider's variable when
/// it's set; else the ★ default; else the "not ready" error.
fn choose_key(store: &Store, provider: &str, cfg: &Value) -> Result<Value, HostError> {
    if let Some(what) = cfg.get(OWN_API_KEY).and_then(Value::as_str) {
        return Err(HostError::Config(format!(
            "{what} has its own api_key, which Lexi no longer uses: keys belong to the provider \
             (name one in the request, like `{provider}#NAME`). Make it one of {provider}'s keys \
             and take it out of the config"
        )));
    }
    let var = provider_variable(cfg);
    let use_variable = |var: &str| {
        let mut out = cfg.clone();
        out["api_key"] = serde_json::json!([{ "env": var }]);
        out[KEY_SCOPE] = serde_json::json!({ "variable": var });
        out
    };
    let name = match chosen_key() {
        Some(k) if k == VARIABLE_KEY => {
            return match var {
                Some(v) => Ok(use_variable(&v)),
                None => Err(HostError::Invalid(format!(
                    "{provider} has no variable: its template doesn't name one"
                ))),
            };
        }
        Some(k) => k,
        None => {
            if let Some(v) = var.as_deref().filter(|v| variable_set(v)) {
                return Ok(use_variable(v));
            }
            let keys = key_list(store, provider, cfg);
            default_of(cfg, &keys).map_err(|why| not_ready(provider, why, &keys, var.as_deref()))?
        }
    };
    let sources = if name.is_empty() {
        if !has_unnamed_key(store, provider, cfg) {
            return Err(unknown_key(provider, "", &key_list(store, provider, cfg)));
        }
        cfg.get("api_key").cloned()
    } else {
        match cfg.get("keys").and_then(|k| k.get(&name)) {
            Some(entry) => entry.get("api_key").cloned(),
            None => {
                return Err(unknown_key(
                    provider,
                    &name,
                    &key_list(store, provider, cfg),
                ))
            }
        }
    };
    let mut out = cfg.clone();
    out["api_key"] = sources.unwrap_or_else(|| Value::Array(Vec::new()));
    out[KEY_SCOPE] = serde_json::json!({ "key": name });
    Ok(out)
}

/// The saved slot of `provider`'s key `name` (`""`: the unnamed key's, the
/// provider's own).
fn key_slot(provider: &str, name: &str) -> Slot {
    if name.is_empty() {
        Slot::new(provider, None)
    } else {
        Slot::named(provider, name)
    }
}

/// How a chosen key is written in messages: `openai#work`, `openai#`, or
/// `$OPENAI_API_KEY`.
fn scope_label(provider: &str, cfg: &Value) -> String {
    let scope = cfg.get(KEY_SCOPE);
    if let Some(v) = scope
        .and_then(|s| s.get("variable"))
        .and_then(Value::as_str)
    {
        return format!("${v}");
    }
    match scope.and_then(|s| s.get("key")).and_then(Value::as_str) {
        Some(k) => format!("{provider}{}", key_label(k)),
        None => provider.to_string(),
    }
}

/// For an effective config with its key chosen: the saved key it leads to
/// (none for the variable), and the config files its `api_key` list comes
/// from.
fn key_context(home: &Path, provider: &str, cfg: &Value) -> (Option<Slot>, Vec<PathBuf>) {
    let guard = vec![providers_config_path(home)];
    let scope = cfg.get(KEY_SCOPE);
    if scope.and_then(|s| s.get("variable")).is_some() {
        return (None, guard);
    }
    let name = scope
        .and_then(|s| s.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("");
    (Some(key_slot(provider, name)), guard)
}

/// The `api_key` sources in `cfg`, in the order they're tried.
fn key_sources(cfg: &Value) -> Result<Vec<Source>, HostError> {
    keystore::parse_sources(cfg.get("api_key")).map_err(HostError::Config)
}

/// Look for the key the operation uses (§12.4): the chosen key's sources,
/// then its saved key — or the provider's variable. `run_commands` false
/// leaves `command:` sources unrun (a listing). Returns the lookup and the
/// effective config with the key chosen.
pub(crate) fn lookup_api_key(
    provider: &str,
    cfg: &Value,
    run_commands: bool,
) -> Result<Lookup, HostError> {
    lookup_api_key_in(&lexi_home(), provider, cfg, run_commands).map(|(l, _)| l)
}

/// [`lookup_api_key`], with the key it chose, written as `#work`, `#` or
/// `$OPENAI_API_KEY`.
pub(crate) fn lookup_chosen_key(
    provider: &str,
    cfg: &Value,
    run_commands: bool,
) -> Result<(Lookup, String), HostError> {
    let (lookup, chosen) = lookup_api_key_in(&lexi_home(), provider, cfg, run_commands)?;
    let label = scope_label(provider, &chosen);
    let short = label.strip_prefix(provider).unwrap_or(&label).to_string();
    Ok((lookup, short))
}

fn lookup_api_key_in(
    home: &Path,
    provider: &str,
    cfg: &Value,
    run_commands: bool,
) -> Result<(Lookup, Value), HostError> {
    let store = Store::open(home)?;
    let cfg = choose_key(&store, provider, cfg)?;
    let (slot, guard) = key_context(home, provider, &cfg);
    let listed = key_sources(&cfg)?;
    let sources =
        keystore::with_saved(&listed, slot.as_ref().map(|s| store.location(s)).as_deref());
    let lookup = keystore::lookup_with(
        home,
        &sources,
        &LookupOpts {
            run_commands,
            guard,
            saved: slot.map(|s| (store, s)),
        },
    );
    Ok((lookup, cfg))
}

/// Resolve the API key a request uses (§12.4) from `cfg` (a provider entry,
/// or an effective config): the chosen key, else the provider's variable,
/// else the default key. The first of the key's sources with a key wins; a
/// read error (an unreadable file, a failing command) stops the lookup and
/// is the error.
pub fn resolve_api_key(provider_name: &str, provider_cfg: &Value) -> Result<String, HostError> {
    let (lookup, chosen) = lookup_api_key_in(&lexi_home(), provider_name, provider_cfg, true)?;
    if let Some(key) = lookup.key.clone() {
        return Ok(key);
    }
    if let Some((source, err)) = lookup.failure() {
        return Err(read_error(provider_name, source, err));
    }
    let who = scope_label(provider_name, &chosen);
    Err(HostError::MissingApiKey(if lookup.checked.is_empty() {
        format!("no API key for {who}: its api_key names no source")
    } else {
        format!("no API key for {who} (looked in {})", lookup.summary())
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
/// `None` when no source has one (or the provider isn't ready). Same order
/// as [`resolve_api_key`] (a read error is an error), but commands aren't
/// run: one that would be is the answer (so listing providers never sets
/// off a password manager).
pub fn api_key_source(
    provider_name: &str,
    provider_cfg: &Value,
) -> Result<Option<&'static str>, HostError> {
    let lookup = match lookup_api_key(provider_name, provider_cfg, false) {
        Ok(l) => l,
        Err(HostError::MissingApiKey(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
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
    /// Whether providers-config.yaml changed: a new key was added, or a key
    /// written in the config itself was taken out.
    pub added: bool,
    /// Where the key is now read from — `location` when it's the saved key,
    /// else an earlier source of that key's that has one.
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

/// One of a provider's keys, for a command that acts on it by name
/// (`openai#work`, `openai#` for the unnamed key): its raw entry (to write
/// back) and its sources as listed (without the saved key unless marked).
struct KeyRef {
    doc: Map,
    entry: Map,
    provider: String,
    key: String,
    sources: Vec<Source>,
    /// Whether it isn't in the config yet (a new named key being saved).
    new: bool,
}

/// The "name the key" refusal for a command that changes a key (§12.5).
fn name_required(home: &Path, provider: &str, example: &str) -> HostError {
    let keys = read_providers_doc(home)
        .ok()
        .and_then(|mut doc| raw_entry(&mut doc, provider).ok())
        .and_then(|e| resolve_template(&Value::Object(e)).ok())
        .and_then(|r| {
            Store::open(home)
                .ok()
                .map(|s| (key_list(&s, provider, &r), r))
        });
    match keys {
        Some((keys, _)) if keys.is_empty() => HostError::Invalid(format!(
            "{provider} has no key: add one with `lexi {example} {provider}#NAME`"
        )),
        Some((keys, r)) => {
            let default = default_of(&r, &keys).ok();
            HostError::Invalid(format!(
                "name the key: `lexi {example} {provider}#NAME` ({provider}'s keys: {})",
                keys_phrase(&keys, default.as_deref())
            ))
        }
        None => HostError::Invalid(format!("name the key: `lexi {example} {provider}#NAME`")),
    }
}

/// The key `target` (`provider#key`) names, for a command that changes it;
/// `example` is the command, for the refusal when `target` names no key. A
/// named key that isn't there yet is an error unless `create` (saving a key
/// creates one); the unnamed key is never created.
fn key_ref(home: &Path, target: &str, create: bool, example: &str) -> Result<KeyRef, HostError> {
    let (provider, key) = split_key_arg(target, None)?;
    check_provider_name(&provider)?;
    let Some(key) = key else {
        return Err(name_required(home, &provider, example));
    };
    let mut doc = read_providers_doc(home)?;
    let entry = raw_entry(&mut doc, &provider)?;
    let resolved = resolve_template(&Value::Object(entry.clone()))?;
    let store = Store::open(home)?;
    let (sources, new) = if key.is_empty() {
        if !has_unnamed_key(&store, &provider, &resolved) {
            return Err(unknown_key(
                &provider,
                "",
                &key_list(&store, &provider, &resolved),
            ));
        }
        (key_sources(&resolved)?, false)
    } else {
        match resolved.get("keys").and_then(|ks| ks.get(&key)) {
            Some(named) => (key_sources(named)?, false),
            None if create => {
                check_new_key_name(&key)?;
                (Vec::new(), true)
            }
            None => {
                return Err(unknown_key(
                    &provider,
                    &key,
                    &key_list(&store, &provider, &resolved),
                ))
            }
        }
    };
    Ok(KeyRef {
        doc,
        entry,
        provider,
        key,
        sources,
        new,
    })
}

impl KeyRef {
    fn slot(&self) -> Slot {
        key_slot(&self.provider, &self.key)
    }

    fn target(&self) -> String {
        format!("{}{}", self.provider, key_label(&self.key))
    }

    /// Write `sources` as the key's list (a new named key is added).
    fn write_sources(self, home: &Path, sources: &[Source]) -> Result<(), HostError> {
        let KeyRef {
            mut doc,
            mut entry,
            provider,
            key,
            ..
        } = self;
        if key.is_empty() {
            entry.insert("api_key".into(), keystore::sources_value(sources));
        } else {
            let keys = entry
                .entry("keys")
                .or_insert_with(|| Value::Object(Map::new()));
            if !keys.is_object() {
                *keys = Value::Object(Map::new());
            }
            let named = keys
                .as_object_mut()
                .expect("an object")
                .entry(key)
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
        }
        providers_mut(&mut doc).insert(provider, Value::Object(entry));
        write_yaml(&providers_config_path(home), &Value::Object(doc))
    }
}

/// A key's sources with its saved key in its place, and where the saved
/// key is.
fn ref_lookup(home: &Path, store: &Store, r: &KeyRef, run_commands: bool) -> Lookup {
    let slot = r.slot();
    let sources = keystore::with_saved(&r.sources, Some(&store.location(&slot)));
    keystore::lookup_with(
        home,
        &sources,
        &LookupOpts {
            run_commands,
            guard: vec![providers_config_path(home)],
            saved: Some((store.clone(), slot)),
        },
    )
}

/// Where a key is read from now: where the saved key is kept when it's the
/// one, else the source's description (a command isn't run, so one that
/// would be is the answer).
fn used_source(home: &Path, store: &Store, target: &str) -> Option<String> {
    let r = key_ref(home, target, false, "providers api-key show").ok()?;
    match ref_lookup(home, store, &r, false).source()? {
        Source::Saved(place) => Some(place.clone()),
        other => Some(other.describe()),
    }
}

/// What a key operation on `target` (`provider#key`) sees: the key's sources
/// (the saved key in its place), where a saved key goes (and the kind of
/// store), and what looking through them found (commands run only with
/// `run_commands`).
pub(crate) struct KeyPlan {
    pub(crate) provider: String,
    pub(crate) key: String,
    pub(crate) sources: Vec<Source>,
    pub(crate) save_to: String,
    pub(crate) store: &'static str,
    pub(crate) lookup: Lookup,
}

pub(crate) fn key_plan(target: &str, run_commands: bool) -> Result<KeyPlan, HostError> {
    let home = lexi_home();
    let store = Store::open(&home)?;
    let r = key_ref(&home, target, false, "providers api-key show")?;
    let lookup = ref_lookup(&home, &store, &r, run_commands);
    Ok(KeyPlan {
        save_to: store.location(&r.slot()),
        store: store.kind(),
        sources: keystore::with_saved(&r.sources, Some(&store.location(&r.slot()))),
        provider: r.provider,
        key: r.key,
        lookup,
    })
}

/// Save the API key `key` as `target`'s (`provider#key`; `provider#` the
/// unnamed key) saved key: in the OS key store, else an owner-only file
/// (`providers/<name>/keys/<key>`, `providers/<name>/api-key` for the
/// unnamed key) — one copy, in one place (`key_store` in config.json). A
/// named key that isn't there yet is created. The sources `api_key` lists
/// are never written; providers-config.yaml changes only to add the key or
/// to take out a key written in the config itself. `target` must name the
/// key (§12.5).
pub fn set_api_key(target: &str, key: &str) -> Result<SavedKey, HostError> {
    set_api_key_in(&lexi_home(), target, key)
}

fn set_api_key_in(home: &Path, target: &str, key: &str) -> Result<SavedKey, HostError> {
    let key = key.trim();
    let r = key_ref(home, target, true, "providers api-key set")?;
    if key.is_empty() {
        return Err(HostError::Config("API key cannot be empty".into()));
    }
    let store = Store::open(home)?;
    let slot = r.slot();
    let label = r.target();
    store.write(&slot, key)?;
    move_key_check(home, &r.provider, &key_label(&r.key), None);
    keystore::forget_commands();
    let has_literal = r.sources.iter().any(|s| matches!(s, Source::Literal(_)));
    let added = has_literal || r.new;
    if added {
        let list: Vec<Source> = r
            .sources
            .iter()
            .filter(|s| !matches!(s, Source::Literal(_)))
            .cloned()
            .collect();
        r.write_sources(home, &list)?;
    }
    Ok(SavedKey {
        used: used_source(home, &store, &label),
        location: store.location(&slot),
        store: store.kind().to_string(),
        added,
    })
}

/// Delete `target`'s (`provider#key`) saved key (from the OS store and any
/// key file). The key itself stays, with its sources (yours) — not ready if
/// they have none. Returns whether there was one. `target` must name the key.
pub fn remove_api_key(target: &str) -> Result<bool, HostError> {
    remove_api_key_in(&lexi_home(), target)
}

fn remove_api_key_in(home: &Path, target: &str) -> Result<bool, HostError> {
    let r = key_ref(home, target, false, "providers api-key remove")?;
    move_key_check(home, &r.provider, &key_label(&r.key), None);
    keystore::forget_commands();
    Store::open(home)?.delete(&r.slot())
}

/// The sources rows of one key: `[{kind, from, value, implicit?}]` — `value`
/// as written in providers-config.yaml; the saved key is `implicit` when the
/// list doesn't mark its place.
fn source_rows(store: &Store, r: &KeyRef) -> Vec<Value> {
    let marked = r.sources.iter().any(|s| matches!(s, Source::Saved(_)));
    let saved_at = store.location(&r.slot());
    keystore::with_saved(&r.sources, Some(&saved_at))
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
        .collect()
}

/// Where keys are looked for, in order. For `provider#key`: `{provider,
/// key_name, sources: [...]}` ([`source_rows`]). For a provider alone (§12.5:
/// a command that looks names no key): every key, `{provider, variable:
/// {name, set}, keys: [{key_name, default, sources}]}`.
pub fn key_source_list(target: &str) -> Result<Value, HostError> {
    key_source_list_in(&lexi_home(), target)
}

fn key_source_list_in(home: &Path, target: &str) -> Result<Value, HostError> {
    let (provider, key) = split_key_arg(target, None)?;
    let store = Store::open(home)?;
    if key.is_some() {
        let r = key_ref(home, target, false, "providers api-key source list")?;
        return Ok(serde_json::json!({
            "provider": r.provider,
            "key_name": r.key,
            "sources": source_rows(&store, &r),
        }));
    }
    check_provider_name(&provider)?;
    let mut doc = read_providers_doc(home)?;
    let resolved = resolve_template(&Value::Object(raw_entry(&mut doc, &provider)?))?;
    let names = key_list(&store, &provider, &resolved);
    let default = default_of(&resolved, &names).ok();
    let mut keys = Vec::new();
    for name in &names {
        let r = key_ref(home, &format!("{provider}#{name}"), false, "")?;
        keys.push(serde_json::json!({
            "key_name": name,
            "default": default.as_deref() == Some(name.as_str()),
            "sources": source_rows(&store, &r),
        }));
    }
    Ok(serde_json::json!({
        "provider": provider,
        "variable": variable_value(&resolved),
        "keys": keys,
    }))
}

/// The last test of each of a provider's keys (`providers status`, the
/// app's Test): `providers/<p>/key-checks.json`, `{label: {status, at, fp}}`
/// — `label` `#work`, `#`, or `$OPENAI_API_KEY` for the variable; `status`
/// `ok` (the provider accepted it), `rejected` (401/403) or `failed` (no
/// answer); `at` seconds since 1970; `fp` a short fingerprint of the key
/// tested (not the key), so a key that has changed since isn't reported as
/// tested.
const KEY_CHECKS: &str = "key-checks.json";

/// A fingerprint of `key`: enough to tell whether it changed, not to
/// recover it.
fn key_fingerprint(key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    format!("{:08x}", h.finish() >> 32)
}

fn read_key_checks(home: &Path, provider: &str) -> Map {
    match read_provider_file(home, provider, KEY_CHECKS) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

fn write_key_checks(home: &Path, provider: &str, checks: &Map) {
    let path = provider_file(home, provider, KEY_CHECKS);
    if checks.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = write_json(&path, &Value::Object(checks.clone()));
    }
}

/// Remember a test of `provider`'s key `label` (`#work`, `#`,
/// `$OPENAI_API_KEY`): `status` `ok`, `rejected` or `failed`. Best effort.
pub(crate) fn record_key_check(provider: &str, label: &str, status: &str, key: &str) {
    record_key_check_in(&lexi_home(), provider, label, status, key);
}

fn record_key_check_in(home: &Path, provider: &str, label: &str, status: &str, key: &str) {
    let mut checks = read_key_checks(home, provider);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    checks.insert(
        label.to_string(),
        serde_json::json!({ "status": status, "at": at, "fp": key_fingerprint(key) }),
    );
    write_key_checks(home, provider, &checks);
}

/// Forget the test of `provider`'s key `label`, or move it to `to` (a
/// renamed key). Best effort.
fn move_key_check(home: &Path, provider: &str, label: &str, to: Option<&str>) {
    let mut checks = read_key_checks(home, provider);
    if let Some(v) = checks.shift_remove(label) {
        if let Some(t) = to {
            checks.insert(t.to_string(), v);
        }
        write_key_checks(home, provider, &checks);
    }
}

/// The last test of `provider`'s key `label`, as `{status, at}` — none when
/// it was never tested, or `value` (the key now, when it's known without
/// running a command) isn't the key that was tested.
fn key_check(checks: &Map, label: &str, value: Option<&str>) -> Value {
    let Some(c) = checks.get(label) else {
        return Value::Null;
    };
    if let Some(v) = value {
        if c.get("fp").and_then(Value::as_str) != Some(key_fingerprint(v).as_str()) {
            return Value::Null;
        }
    }
    serde_json::json!({ "status": c.get("status"), "at": c.get("at") })
}

/// `{name, set}` for the provider's variable, or null without one.
fn variable_value(cfg: &Value) -> Value {
    match provider_variable(cfg) {
        Some(v) => serde_json::json!({ "name": v, "set": variable_set(&v) }),
        None => Value::Null,
    }
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

/// Add a place to look for `target`'s (`provider#key`) key: at the end of
/// its `api_key` list, or first with `first` (one already listed moves
/// there). Returns its sources afterwards ([`key_source_list`]). `target`
/// must name the key.
pub fn add_key_source(target: &str, source: &Value, first: bool) -> Result<Value, HostError> {
    add_key_source_in(&lexi_home(), target, source, first)
}

fn add_key_source_in(
    home: &Path,
    target: &str,
    source: &Value,
    first: bool,
) -> Result<Value, HostError> {
    let new = source_arg(source)?;
    let r = key_ref(home, target, false, "providers api-key source add")?;
    let label = r.target();
    move_key_check(home, &r.provider, &key_label(&r.key), None);
    let mut list = r.sources.clone();
    let had = list.iter().any(|s| s.same_place(&new));
    if !had || first {
        list.retain(|s| !s.same_place(&new));
        if first {
            list.insert(0, new);
        } else {
            list.push(new);
        }
        r.write_sources(home, &list)?;
    }
    keystore::forget_commands();
    key_source_list_in(home, &label)
}

/// Stop looking in a place for `target`'s (`provider#key`) key — taking it
/// out of its `api_key` list (`saved` takes out only the marker: the saved
/// key is looked in last again). Returns whether it was listed. `target`
/// must name the key.
pub fn remove_key_source(target: &str, source: &Value) -> Result<bool, HostError> {
    remove_key_source_in(&lexi_home(), target, source)
}

fn remove_key_source_in(home: &Path, target: &str, source: &Value) -> Result<bool, HostError> {
    let gone = source_arg(source)?;
    let r = key_ref(home, target, false, "providers api-key source rm")?;
    if !r.sources.iter().any(|s| s.same_place(&gone)) {
        return Ok(false);
    }
    move_key_check(home, &r.provider, &key_label(&r.key), None);
    let list: Vec<Source> = r
        .sources
        .iter()
        .filter(|s| !s.same_place(&gone))
        .cloned()
        .collect();
    r.write_sources(home, &list)?;
    keystore::forget_commands();
    Ok(true)
}

/// A provider's keys (§12): `{provider, default_key, store: {kind}, keys:
/// [{name, label, default, source, from, error?}], variable: {name, set},
/// ready, uses, reason}` — `store` where saved keys are kept (`keychain`,
/// `credential-manager`, `secret-service`, `file`); the unnamed key (`name: ""`, label `#`) first when there is
/// one, then the named ones, each with where its key comes from (`source` /
/// `from`, null when none has one; commands aren't run); `default_key` the
/// ★ key's name (null: none); `uses` what a request that names no key uses
/// (`{variable: NAME}` or `{key: NAME}`, null when nothing), and `reason`
/// why not (`no key`, `no default key`, `no key found`). Never the keys
/// themselves.
pub fn key_names(provider: &str) -> Result<Value, HostError> {
    key_names_in(&lexi_home(), provider)
}

fn key_names_in(home: &Path, provider: &str) -> Result<Value, HostError> {
    check_provider_name(provider)?;
    let mut doc = read_providers_doc(home)?;
    let entry = raw_entry(&mut doc, provider)?;
    let resolved = resolve_template(&Value::Object(entry))?;
    let store = Store::open(home)?;
    let names = key_list(&store, provider, &resolved);
    let default = default_of(&resolved, &names);
    let checks = read_key_checks(home, provider);
    let mut rows = Vec::new();
    for name in &names {
        let r = key_ref(home, &format!("{provider}#{name}"), false, "")?;
        let found = ref_lookup(home, &store, &r, false);
        let check = key_check(&checks, &key_label(name), found.key.as_deref());
        let (source, from, error) = match found.failure() {
            Some((s, e)) => (None, Some(s.describe()), Some(e.to_string())),
            None => (
                found.source().map(Source::kind),
                found.source().map(Source::describe),
                None,
            ),
        };
        let mut row = serde_json::json!({
            "name": name,
            "label": key_label(name),
            "default": default.as_ref().ok() == Some(name),
            "source": source,
            "from": from,
            "check": check,
        });
        if let Some(e) = error {
            row["error"] = Value::from(e);
        }
        rows.push(row);
    }
    let var = provider_variable(&resolved);
    let mut variable = variable_value(&resolved);
    if let Some(v) = var.as_deref().filter(|v| variable_set(v)) {
        variable["check"] = key_check(&checks, &format!("${v}"), std::env::var(v).ok().as_deref());
    }
    let (uses, reason) = match (&var, &default) {
        (Some(v), _) if variable_set(v) => (serde_json::json!({ "variable": v }), Value::Null),
        (_, Ok(d)) => {
            let has = rows
                .iter()
                .find(|r| r["name"] == d.as_str())
                .is_some_and(|r| !r["source"].is_null());
            if has {
                (serde_json::json!({ "key": d }), Value::Null)
            } else {
                (Value::Null, Value::from("no key found"))
            }
        }
        (_, Err(why)) => (Value::Null, Value::from(why.reason())),
    };
    Ok(serde_json::json!({
        "provider": provider,
        "default_key": default.as_ref().ok(),
        "store": { "kind": store.kind() },
        "keys": rows,
        "variable": variable,
        "ready": reason.is_null(),
        "uses": uses,
        "reason": reason,
    }))
}

/// Add the key `name` to `provider` (`keys: {name: {}}` — its saved key
/// alone until sources are added). Whether it's new.
pub fn add_named_key(provider: &str, name: &str) -> Result<bool, HostError> {
    add_named_key_in(&lexi_home(), provider, name)
}

fn add_named_key_in(home: &Path, provider: &str, name: &str) -> Result<bool, HostError> {
    check_new_key_name(name)?;
    let r = key_ref(home, &format!("{provider}#{name}"), true, "")?;
    let new = r.new;
    if new {
        let sources = r.sources.clone();
        r.write_sources(home, &sources)?;
    }
    Ok(new)
}

/// Rename `provider`'s key `old` to `new` (`old` `""`: the unnamed key,
/// which becomes a named key). Its sources and saved key go along, and it
/// stays the default if it was.
pub fn rename_named_key(provider: &str, old: &str, new: &str) -> Result<(), HostError> {
    rename_named_key_in(&lexi_home(), provider, old, new)
}

fn rename_named_key_in(home: &Path, provider: &str, old: &str, new: &str) -> Result<(), HostError> {
    check_new_key_name(new)?;
    let r = key_ref(home, &format!("{provider}#{old}"), false, "")?;
    let store = Store::open(home)?;
    let resolved = resolve_template(&Value::Object(r.entry.clone()))?;
    let names = key_list(&store, provider, &resolved);
    if names.iter().any(|k| k == new) {
        return Err(HostError::Invalid(format!(
            "{provider} already has a key named {new:?}"
        )));
    }
    let was_default = default_of(&resolved, &names).ok().as_deref() == Some(old);
    let KeyRef {
        mut doc,
        mut entry,
        sources,
        ..
    } = r;
    if old.is_empty() {
        // The unnamed key becomes a named one: its list moves into `keys`.
        entry.shift_remove("api_key");
        let mut named = Map::new();
        if !sources.is_empty() {
            named.insert("api_key".into(), keystore::sources_value(&sources));
        }
        let keys = entry
            .entry("keys")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(k) = keys.as_object_mut() {
            k.insert(new.to_string(), Value::Object(named));
        }
    } else if let Some(keys) = entry.get_mut("keys").and_then(Value::as_object_mut) {
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
    if was_default && names.len() > 1 {
        entry.insert("default_key".into(), Value::from(new));
    }
    let (from, to) = (key_slot(provider, old), key_slot(provider, new));
    store.rename(&from, &to)?;
    move_key_check(home, provider, &key_label(old), Some(&key_label(new)));
    let (from_file, to_file) = (
        keystore::file_path(home, &from.file()),
        keystore::file_path(home, &to.file()),
    );
    if from_file.exists() {
        if let Some(dir) = to_file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::rename(from_file, to_file)?;
    }
    providers_mut(&mut doc).insert(provider.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    keystore::forget_commands();
    Ok(())
}

/// Remove `provider`'s key `name` (`""`: the unnamed key): its config and
/// its saved key. Any key can be removed, the default and the only one
/// included (§12.2 rule 4): removing the default leaves no default while
/// several keys remain, and removing the only key leaves the provider with
/// none.
pub fn remove_named_key(provider: &str, name: &str) -> Result<(), HostError> {
    remove_named_key_in(&lexi_home(), provider, name)
}

fn remove_named_key_in(home: &Path, provider: &str, name: &str) -> Result<(), HostError> {
    let r = key_ref(home, &format!("{provider}#{name}"), false, "")?;
    let store = Store::open(home)?;
    let resolved = resolve_template(&Value::Object(r.entry.clone()))?;
    let names = key_list(&store, provider, &resolved);
    let was_default = default_of(&resolved, &names).ok().as_deref() == Some(name);
    let slot = r.slot();
    let KeyRef {
        mut doc, mut entry, ..
    } = r;
    if name.is_empty() {
        entry.shift_remove("api_key");
    } else {
        if let Some(keys) = entry.get_mut("keys").and_then(Value::as_object_mut) {
            keys.shift_remove(name);
        }
        if entry
            .get("keys")
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty)
        {
            entry.shift_remove("keys");
        }
    }
    let left = names.len() - 1;
    if was_default && left > 1 {
        // No default until you choose one (null, so the unnamed key doesn't
        // become it by the older rule).
        entry.insert("default_key".into(), Value::Null);
    } else if was_default || left <= 1 {
        entry.shift_remove("default_key");
    }
    store.delete(&slot)?;
    move_key_check(home, provider, &key_label(name), None);
    let file = keystore::file_path(home, &slot.file());
    if file.exists() {
        std::fs::remove_file(file)?;
    }
    providers_mut(&mut doc).insert(provider.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    keystore::forget_commands();
    Ok(())
}

/// Make `name` the key `provider`'s requests use when they name none (`""`:
/// the unnamed key).
pub fn set_default_key(provider: &str, name: &str) -> Result<(), HostError> {
    set_default_key_in(&lexi_home(), provider, name)
}

fn set_default_key_in(home: &Path, provider: &str, name: &str) -> Result<(), HostError> {
    let r = key_ref(home, &format!("{provider}#{name}"), false, "")?;
    let KeyRef {
        mut doc, mut entry, ..
    } = r;
    entry.insert("default_key".into(), Value::from(name));
    providers_mut(&mut doc).insert(provider.to_string(), Value::Object(entry));
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    keystore::forget_commands();
    Ok(())
}

/// The key file Lexi saves (or once saved) `provider`'s key in (or, before
/// §12, its `api`'s own key).
fn key_file(provider: &str, api: Option<&str>) -> String {
    Slot::new(provider, api).file()
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
        changed |= unlist_provider_variable(entry);
        changed |= drop_default_key_name(entry);
    }
    if changed {
        let _ = write_yaml(&providers_config_path(home), &Value::Object(doc));
    }
}

/// In a raw entry built on a template, the provider's variable comes out of
/// its unnamed key's `api_key` list (0.3.0 listed it there): it isn't one of
/// the provider's keys, and it's looked at on its own, first
/// (`docs/key-storage-design.md` §12.7). A list left empty is taken out, so
/// the key is its saved key alone (or there's no unnamed key, when nothing
/// is saved). Whether anything changed.
fn unlist_provider_variable(entry: &mut Value) -> bool {
    let Some(var) = resolve_template(entry)
        .ok()
        .and_then(|r| provider_variable(&r))
    else {
        return false;
    };
    let Ok(list) = keystore::parse_sources(entry.get("api_key")) else {
        return false;
    };
    if !list
        .iter()
        .any(|s| matches!(s, Source::Env(v) if *v == var))
    {
        return false;
    }
    let out: Vec<Source> = list
        .into_iter()
        .filter(|s| !matches!(s, Source::Env(v) if *v == var))
        .collect();
    if let Some(o) = entry.as_object_mut() {
        if out.is_empty() {
            o.shift_remove("api_key");
        } else {
            o.insert("api_key".into(), keystore::sources_value(&out));
        }
    }
    true
}

/// `default_key: default` (before §12, the unnamed key) is the unnamed key
/// when it's the default anyway: it comes out. Whether anything changed.
fn drop_default_key_name(entry: &mut Value) -> bool {
    let Some(o) = entry.as_object_mut() else {
        return false;
    };
    if o.get("default_key").and_then(Value::as_str) == Some("default") {
        o.shift_remove("default_key");
        return true;
    }
    false
}

/// A name for a key made from `base` (an API or a model): what fits the
/// name rules (other characters become `-`, at most 32), with `-2`, `-3`…
/// when `taken`.
fn key_name_from(base: &str, taken: &[String]) -> String {
    let mut name: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    name = name.trim_matches('-').to_string();
    if name.is_empty() || name == "default" {
        name = format!("key-{name}").trim_end_matches('-').to_string();
    }
    name.truncate(32);
    let mut candidate = name.clone();
    let mut n = 2;
    while taken.contains(&candidate) {
        let suffix = format!("-{n}");
        let mut stem = name.clone();
        stem.truncate(32 - suffix.len());
        candidate = format!("{stem}{suffix}");
        n += 1;
    }
    candidate
}

/// An API binding's or a configured model's own key becomes one of the
/// provider's named keys, once (`docs/key-storage-design.md` §12.7): an
/// API's is `#<api>`, its list and saved key moved there; a model's is named
/// after the model (sources only; a model never had a saved key). Requests
/// to that API or model then use the provider's key like any other, so each
/// move is a notice. Best effort: what can't move stays, and is reported
/// when it would be used.
pub(crate) fn adopt_own_keys(home: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    let Ok(mut doc) = read_providers_doc(home) else {
        return notes;
    };
    let Ok(store) = Store::open(home) else {
        return notes;
    };
    let mut changed = false;
    let names: Vec<String> = providers_mut(&mut doc).keys().cloned().collect();
    for provider in names {
        let Some(entry) = providers_mut(&mut doc)
            .get_mut(&provider)
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let mut taken = named_key_names(&Value::Object(entry.clone()));
        let mut adopted: Vec<(String, Value)> = Vec::new();
        if let Some(apis) = entry.get_mut("apis").and_then(Value::as_array_mut) {
            for b in apis.iter_mut() {
                let Some(api) = b.get("api").and_then(Value::as_str).map(String::from) else {
                    continue;
                };
                let Some(list) = b.get("api_key").cloned() else {
                    continue;
                };
                let name = key_name_from(&api, &taken);
                let (from, to) = (
                    Slot::new(&provider, Some(&api)),
                    Slot::named(&provider, &name),
                );
                if store.rename(&from, &to).is_err() {
                    continue;
                }
                let (from_file, to_file) = (
                    keystore::file_path(home, &from.file()),
                    keystore::file_path(home, &to.file()),
                );
                if from_file.exists() {
                    if let Some(dir) = to_file.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    if std::fs::rename(&from_file, &to_file).is_err() {
                        continue;
                    }
                }
                if let Some(o) = b.as_object_mut() {
                    o.shift_remove("api_key");
                }
                notes.push(format!(
                    "{provider}'s {api} API had its own key. It's now the key #{name}, used only when a request names it ({provider}#{name}). Rename it with `lexi providers keys rename {provider} {name} NEW`."
                ));
                taken.push(name.clone());
                adopted.push((name, list));
            }
        }
        // A configured model's own key.
        let models_path = provider_file(home, &provider, "configured-models.yaml");
        if let Ok(mut models) = read_provider_file(home, &provider, "configured-models.yaml") {
            let mut models_changed = false;
            if let Some(entries) = models.get_mut("models").and_then(Value::as_object_mut) {
                for (id, meta) in entries.iter_mut() {
                    let Some(list) = meta.get("api_key").cloned() else {
                        continue;
                    };
                    let name = key_name_from(id, &taken);
                    if let Some(o) = meta.as_object_mut() {
                        o.shift_remove("api_key");
                    }
                    models_changed = true;
                    notes.push(format!(
                        "{provider}'s model {id} had its own key. It's now the key #{name}, used only when a request names it ({provider}#{name}). Rename it with `lexi providers keys rename {provider} {name} NEW`."
                    ));
                    taken.push(name.clone());
                    adopted.push((name, list));
                }
            }
            if models_changed && write_yaml(&models_path, &models).is_err() {
                continue;
            }
        }
        if adopted.is_empty() {
            continue;
        }
        let keys = entry
            .entry("keys")
            .or_insert_with(|| Value::Object(Map::new()));
        if !keys.is_object() {
            *keys = Value::Object(Map::new());
        }
        if let Some(k) = keys.as_object_mut() {
            for (name, list) in adopted {
                // The saved marker alone is the saved key alone.
                let sources = keystore::parse_sources(Some(&list)).unwrap_or_default();
                let mut named = Map::new();
                if !sources.iter().all(|s| matches!(s, Source::Saved(_))) {
                    named.insert("api_key".into(), list);
                }
                k.insert(name, Value::Object(named));
            }
        }
        changed = true;
    }
    if changed {
        let _ = write_yaml(&providers_config_path(home), &Value::Object(doc));
    }
    notes
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

        set_api_key_in(home, "mid#main", "mid-key").unwrap();
        rename_provider_in(home, "mid", "middle").unwrap();
        // Key files Lexi saved follow the provider to its new name, and the
        // config doesn't list them.
        assert!(home.join("providers/middle/keys/main").exists());
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
            lookup_api_key_in(home, provider, &entry, true).map(|(l, _)| l.key)
        })
    }

    fn found(home: &Path, provider: &str, cfg: &Value, run: bool) -> Lookup {
        lookup_api_key_in(home, provider, cfg, run).unwrap().0
    }

    #[test]
    fn the_provider_key_shorthand_is_split_and_checked() {
        let split = |p: &str, k: Option<&str>| split_key_arg(p, k);
        assert_eq!(split("openai", None).unwrap(), ("openai".into(), None));
        assert_eq!(
            split("openai#work", None).unwrap(),
            ("openai".into(), Some("work".into()))
        );
        // `openai#` is the unnamed key.
        assert_eq!(
            split("openai#", None).unwrap(),
            ("openai".into(), Some(String::new()))
        );
        assert_eq!(
            split("openai", Some("work")).unwrap(),
            ("openai".into(), Some("work".into()))
        );
        assert!(split("openai#work", Some("work")).is_ok());
        let err = split("openai#work", Some("home")).unwrap_err().to_string();
        assert!(err.contains("name different keys"), "{err}");
        assert!(split("openai#no good", None).is_err());
        // `default` isn't a key name any more.
        let err = split("openai#default", None).unwrap_err().to_string();
        assert!(err.contains("`openai#`"), "{err}");
        assert!(check_provider_name("a#b").is_err());
        assert_eq!(key_label(""), "#");
        assert_eq!(key_label("work"), "#work");
    }

    #[test]
    fn a_key_is_chosen_by_name_or_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::env::set_var("LEXI_NK_UNNAMED", "k-unnamed");
        std::env::set_var("LEXI_NK_WORK", "k-work");
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api: chat-completions\n    api_key: [{env: LEXI_NK_UNNAMED}]\n    keys:\n      work: {api_key: [{env: LEXI_NK_WORK}]}\n      personal: {}\n",
        );
        // The unnamed key (saved before names) is the default while
        // default_key is absent, and `p#` names it.
        assert_eq!(
            key_for(home, "p", None).unwrap().as_deref(),
            Some("k-unnamed")
        );
        assert_eq!(
            key_for(home, "p", Some("")).unwrap().as_deref(),
            Some("k-unnamed")
        );
        assert_eq!(
            key_for(home, "p", Some("work")).unwrap().as_deref(),
            Some("k-work")
        );
        // A named key with no sources is its saved key alone.
        assert_eq!(key_for(home, "p", Some("personal")).unwrap(), None);
        let saved = set_api_key_in(home, "p#personal", "k-personal").unwrap();
        assert_eq!(saved.location, "providers/p/keys/personal");
        assert!(!saved.added);
        assert_eq!(
            key_for(home, "p", Some("personal")).unwrap().as_deref(),
            Some("k-personal")
        );
        assert!(!home.join("providers/p/api-key").exists());
        let err = key_for(home, "p", Some("nope")).unwrap_err().to_string();
        assert!(
            err.contains("p has no key named \"nope\" (its keys: #, #work, #personal)"),
            "{err}"
        );

        set_default_key_in(home, "p", "work").unwrap();
        assert_eq!(key_for(home, "p", None).unwrap().as_deref(), Some("k-work"));
        assert!(set_default_key_in(home, "p", "nope").is_err());
        let names = key_names_in(home, "p").unwrap();
        assert_eq!(names["default_key"], "work");
        assert_eq!(names["uses"], json!({"key": "work"}));
        assert_eq!(names["ready"], true);
        let rows: Vec<(&str, &str, bool, &str)> = names["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["name"].as_str().unwrap(),
                    r["label"].as_str().unwrap(),
                    r["default"] == true,
                    r["source"].as_str().unwrap_or("-"),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("", "#", false, "env"),
                ("work", "#work", true, "env"),
                ("personal", "#personal", false, "saved")
            ]
        );
        // The unnamed key can be the default by name too.
        set_default_key_in(home, "p", "").unwrap();
        assert_eq!(
            key_for(home, "p", None).unwrap().as_deref(),
            Some("k-unnamed")
        );
        std::env::remove_var("LEXI_NK_UNNAMED");
        std::env::remove_var("LEXI_NK_WORK");
    }

    #[test]
    fn the_providers_variable_wins_when_set_and_a_named_key_wins_over_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  openai:\n    template: openai\n    keys:\n      work: {}\n",
        );
        std::env::remove_var("OPENAI_API_KEY");
        set_api_key_in(home, "openai#work", "k-work").unwrap();
        // The template's variable isn't a key: the only key is #work.
        let names = key_names_in(home, "openai").unwrap();
        assert_eq!(names["keys"].as_array().unwrap().len(), 1);
        assert_eq!(
            names["variable"],
            json!({"name": "OPENAI_API_KEY", "set": false})
        );
        assert_eq!(
            key_for(home, "openai", None).unwrap().as_deref(),
            Some("k-work")
        );
        // Set, it wins over the default; a named key still wins over it.
        std::env::set_var("OPENAI_API_KEY", "k-env");
        assert_eq!(
            key_for(home, "openai", None).unwrap().as_deref(),
            Some("k-env")
        );
        assert_eq!(
            key_for(home, "openai", Some("work")).unwrap().as_deref(),
            Some("k-work")
        );
        assert_eq!(
            key_for(home, "openai", Some(VARIABLE_KEY))
                .unwrap()
                .as_deref(),
            Some("k-env")
        );
        let names = key_names_in(home, "openai").unwrap();
        assert_eq!(names["uses"], json!({"variable": "OPENAI_API_KEY"}));
        std::env::remove_var("OPENAI_API_KEY");

        // No key and no variable: not ready, with the fix.
        write_providers(home, "providers:\n  anthropic:\n    template: anthropic\n");
        std::env::remove_var("ANTHROPIC_API_KEY");
        let err = key_for(home, "anthropic", None).unwrap_err().to_string();
        assert!(
            err.contains("anthropic has no key: add one with `lexi providers api-key set anthropic#NAME`, or set $ANTHROPIC_API_KEY"),
            "{err}"
        );
        let names = key_names_in(home, "anthropic").unwrap();
        assert_eq!(
            (names["ready"].clone(), names["reason"].clone()),
            (json!(false), json!("no key"))
        );
    }

    #[test]
    fn keys_are_added_renamed_and_removed_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api: chat-completions\n",
        );
        // A command that changes a key must name it.
        let err = set_api_key_in(home, "p", "k").unwrap_err().to_string();
        assert!(
            err.contains("p has no key: add one with `lexi providers api-key set p#NAME`"),
            "{err}"
        );
        // `p#` never creates an unnamed key.
        assert!(set_api_key_in(home, "p#", "k").is_err());
        // Saving under a new name creates it.
        let saved = set_api_key_in(home, "p#client-a", "k-a").unwrap();
        assert!(saved.added);
        assert_eq!(
            read_providers_doc(home).unwrap()["providers"]["p"]["keys"]["client-a"],
            json!({})
        );
        // The only key is the default.
        assert_eq!(key_for(home, "p", None).unwrap().as_deref(), Some("k-a"));
        assert!(add_named_key_in(home, "p", "client-b").unwrap());
        assert!(!add_named_key_in(home, "p", "client-b").unwrap());
        assert!(add_named_key_in(home, "p", "default").is_err());
        assert!(add_named_key_in(home, "p", "").is_err());
        let err = set_api_key_in(home, "p", "k").unwrap_err().to_string();
        assert!(
            err.contains("name the key: `lexi providers api-key set p#NAME` (p's keys: #client-a, #client-b)"),
            "{err}"
        );
        // Two keys and no default: a request that names none is refused.
        let err = key_for(home, "p", None).unwrap_err().to_string();
        assert!(err.contains("p has several keys and no default"), "{err}");
        add_key_source_in(home, "p#client-b", &json!({"env": "B"}), false).unwrap();
        assert_eq!(
            read_providers_doc(home).unwrap()["providers"]["p"]["keys"]["client-b"]["api_key"],
            json!([{"env": "B"}])
        );
        assert!(add_key_source_in(home, "p", &json!({"env": "B"}), false).is_err());
        set_default_key_in(home, "p", "client-a").unwrap();

        // Renaming moves its saved key, and the default along.
        rename_named_key_in(home, "p", "client-a", "acme").unwrap();
        let entry = &read_providers_doc(home).unwrap()["providers"]["p"];
        assert_eq!(entry["default_key"], "acme");
        assert!(entry["keys"].get("client-a").is_none());
        assert!(!home.join("providers/p/keys/client-a").exists());
        assert_eq!(key_for(home, "p", None).unwrap().as_deref(), Some("k-a"));
        assert!(rename_named_key_in(home, "p", "acme", "client-b").is_err());

        // Removing the default leaves no default while several remain...
        add_named_key_in(home, "p", "client-c").unwrap();
        remove_named_key_in(home, "p", "acme").unwrap();
        assert!(!home.join("providers/p/keys/acme").exists());
        let entry = &read_providers_doc(home).unwrap()["providers"]["p"];
        assert!(entry["default_key"].is_null() && entry.get("default_key").is_some());
        assert_eq!(key_names_in(home, "p").unwrap()["reason"], "no default key");
        // ...and with one left, it's the default.
        remove_named_key_in(home, "p", "client-c").unwrap();
        assert!(read_providers_doc(home).unwrap()["providers"]["p"]
            .get("default_key")
            .is_none());
        assert_eq!(key_names_in(home, "p").unwrap()["default_key"], "client-b");
        // The only key can go too: the provider has none.
        remove_named_key_in(home, "p", "client-b").unwrap();
        assert!(read_providers_doc(home).unwrap()["providers"]["p"]
            .get("keys")
            .is_none());
        assert!(remove_named_key_in(home, "p", "client-b").is_err());
        assert_eq!(key_names_in(home, "p").unwrap()["reason"], "no key");
    }

    #[test]
    fn the_unnamed_key_is_renamed_into_a_named_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api: chat-completions\n    api_key: [saved, {file: mine.key}]\n    keys:\n      work: {}\n",
        );
        std::fs::create_dir_all(home.join("providers/p")).unwrap();
        std::fs::write(home.join("providers/p/api-key"), "old-key\n").unwrap();
        // The unnamed key is the default (no default_key, as before §12).
        assert_eq!(
            key_for(home, "p", None).unwrap().as_deref(),
            Some("old-key")
        );
        rename_named_key_in(home, "p", "", "personal").unwrap();
        let entry = &read_providers_doc(home).unwrap()["providers"]["p"];
        assert!(entry.get("api_key").is_none());
        assert_eq!(
            entry["keys"]["personal"]["api_key"],
            json!(["saved", {"file": "mine.key"}])
        );
        // It stays the default, now by name.
        assert_eq!(entry["default_key"], "personal");
        assert!(!home.join("providers/p/api-key").exists());
        assert!(home.join("providers/p/keys/personal").exists());
        assert_eq!(
            key_for(home, "p", None).unwrap().as_deref(),
            Some("old-key")
        );
        // `p#` is gone for good.
        assert!(key_for(home, "p", Some("")).is_err());
    }

    #[test]
    fn an_apis_or_models_own_key_is_never_used() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api_key: [{env: PATH}]\n    apis:\n      - api: chat-completions\n      - {api: embeddings, api_key: [{env: PATH}]}\n",
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
        let err = lookup_api_key_in(home, "p", &emb, false)
            .err()
            .expect("an error")
            .to_string();
        assert!(
            err.contains("the embeddings API has its own api_key"),
            "{err}"
        );
        let chat = binding_config(&entry, &bindings[0]).unwrap();
        assert!(lookup_api_key_in(home, "p", &chat, false).is_ok());
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
        let found = |cfg: Value| found(home, "p", &cfg, true);

        // Last by default: a listed source with a key wins.
        let last = found(json!({"api_key": [{"file": "mine.key"}]}));
        assert_eq!(last.key.as_deref(), Some("mine"));
        assert!(last.checked[1].0 == Source::Saved("providers/p/api-key".into()));
        assert_eq!(last.checked[1].1.state(), "not checked");
        // Marked first, it wins.
        let first = found(json!({"api_key": ["saved", {"file": "mine.key"}]}));
        assert_eq!(first.key.as_deref(), Some("saved-key"));
        assert_eq!(first.used().map(Source::kind), Some("saved"));
        // No list, but a saved key: the unnamed key is its saved key alone.
        assert_eq!(found(json!({})).key.as_deref(), Some("saved-key"));
        // Nothing listed and nothing saved: the provider has no key.
        let err = lookup_api_key_in(home, "q", &json!({}), true)
            .err()
            .expect("an error")
            .to_string();
        assert!(err.contains("q has no key"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_command_prints_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(home, "providers: {}\n");
        let sh = |script: &str| json!({"command": ["sh", "-c", script]});
        let found = |cfg: Value, run: bool| found(home, "p", &cfg, run);

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

        // The unnamed key's file; the config doesn't change.
        let saved = set_api_key_in(home, "p#", " k1 ").unwrap();
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
        assert!(!set_api_key_in(home, "p#", "k2").unwrap().added);

        // A listed file is yours: never written, and it still comes first.
        std::fs::create_dir_all(home.join("my-keys")).unwrap();
        std::fs::write(home.join("my-keys/m.key"), "mine\n").unwrap();
        let mine = set_api_key_in(home, "m#", "k4").unwrap();
        assert_eq!(mine.location, "providers/m/api-key");
        assert_eq!(mine.used.as_deref(), Some("my-keys/m.key"));
        assert_eq!(
            std::fs::read_to_string(home.join("my-keys/m.key")).unwrap(),
            "mine\n"
        );
        let after_p_m = std::fs::read_to_string(providers_config_path(home)).unwrap();
        assert_eq!(after_p_m, before);

        // A key written in the config is taken out.
        let q = set_api_key_in(home, "q#", "k3").unwrap();
        assert!(q.added);
        assert_eq!(entry_key(home, "q"), json!([]));
        assert!(set_api_key_in(home, "q#", "  ").is_err());
        assert!(set_api_key_in(home, "nope#x", "k").is_err());

        // Remove deletes the saved key only, and needs the key's name.
        assert!(remove_api_key_in(home, "m").is_err());
        assert!(remove_api_key_in(home, "m#").unwrap());
        assert!(!remove_api_key_in(home, "m#").unwrap());
        assert!(home.join("my-keys/m.key").exists());
    }

    #[test]
    fn sources_can_be_added_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  openai:\n    template: openai\n    keys:\n      work: {}\n",
        );
        let kinds = |v: &Value| -> Vec<String> {
            v["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["from"].as_str().unwrap().to_string())
                .collect()
        };
        let listed = key_source_list_in(home, "openai#work").unwrap();
        assert_eq!(kinds(&listed), ["providers/openai/keys/work"]);
        assert_eq!(listed["sources"][0]["implicit"], true);
        // Named no key: every key, and the variable.
        let all = key_source_list_in(home, "openai").unwrap();
        assert_eq!(all["keys"][0]["key_name"], "work");
        assert_eq!(all["variable"]["name"], "OPENAI_API_KEY");

        let after = add_key_source_in(home, "openai#work", &json!({"file": "~/k"}), false).unwrap();
        assert_eq!(kinds(&after), ["~/k", "providers/openai/keys/work"]);
        let after = add_key_source_in(home, "openai#work", &json!("saved"), true).unwrap();
        assert_eq!(kinds(&after)[0], "providers/openai/keys/work");
        assert!(after["sources"][0].get("implicit").is_none());
        let doc = read_providers_doc(home).unwrap();
        assert_eq!(
            doc["providers"]["openai"]["keys"]["work"]["api_key"],
            json!(["saved", {"file": "~/k"}])
        );
        // Adding one that's listed changes nothing; with first it moves.
        add_key_source_in(home, "openai#work", &json!({"file": "~/k"}), false).unwrap();
        add_key_source_in(home, "openai#work", &json!({"file": "~/k"}), true).unwrap();
        let doc = read_providers_doc(home).unwrap();
        assert_eq!(
            doc["providers"]["openai"]["keys"]["work"]["api_key"][0],
            json!({"file": "~/k"})
        );
        // A key itself isn't a source.
        assert!(add_key_source_in(home, "openai#work", &json!("sk-123"), false).is_err());

        assert!(remove_key_source_in(home, "openai#work", &json!("saved")).unwrap());
        assert!(!remove_key_source_in(home, "openai#work", &json!("saved")).unwrap());
        assert!(remove_key_source_in(home, "openai#work", &json!({"file": "~/k"})).unwrap());
        assert!(
            read_providers_doc(home).unwrap()["providers"]["openai"]["keys"]["work"]
                .get("api_key")
                .is_none()
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
        // The provider's variable isn't listed (it's looked at on its own).
        assert_eq!(entry_key(home, "openai"), json!(["saved"]));
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
            "providers:\n  openai:\n    template: openai\n    api_key: [{keychain: openai}, {env: OPENAI_API_KEY}]\n  both:\n    url: u\n    api_key: [{file: providers/both/api-key}, {keychain: both}, {env: B}]\n",
        );
        upgrade_key_sources(home);
        assert_eq!(entry_key(home, "openai"), json!(["saved"]));
        // Listed once, where the file already was.
        assert_eq!(entry_key(home, "both"), json!(["saved", {"env": "B"}]));
        let doc = read_providers_doc(home).unwrap();
        let resolved = resolve_template(&doc["providers"]["openai"]).unwrap();
        assert!(key_sources(&resolved).is_ok());
    }

    #[test]
    fn lexis_own_key_files_and_the_variable_come_out_of_the_lists() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // What 0.3.0 wrote when it saved keys.
        write_providers(
            home,
            "providers:\n  openai:\n    template: openai\n    api_key: [{env: OPENAI_API_KEY}, {file: providers/openai/api-key}]\n  anthropic:\n    template: anthropic\n    api_key: [{file: providers/anthropic/api-key}, {env: ANTHROPIC_API_KEY}]\n    default_key: default\n  mine:\n    url: u\n    api_key: [{env: X}, {file: providers/mine/api-key}, {file: ~/elsewhere.key}]\n",
        );
        upgrade_key_sources(home);
        let doc = read_providers_doc(home).unwrap();
        // Nothing left but the saved key: the list goes.
        assert!(doc["providers"]["openai"].get("api_key").is_none());
        // First, or followed by another source: it becomes the marker.
        assert_eq!(entry_key(home, "anthropic"), json!(["saved"]));
        assert!(doc["providers"]["anthropic"].get("default_key").is_none());
        // A provider without a template has no variable: its list stays.
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
    fn an_apis_and_a_models_own_keys_become_named_keys() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  openai:\n    template: openai\n    keys:\n      image-generations: {}\n    apis:\n      - api: image-generations\n        api_key: [saved]\n      - api: embeddings\n        api_key: [{env: EMB}]\n",
        );
        std::fs::create_dir_all(home.join("providers/openai")).unwrap();
        std::fs::write(
            home.join("providers/openai/api-key-image-generations"),
            "img\n",
        )
        .unwrap();
        std::fs::write(
            home.join("providers/openai/configured-models.yaml"),
            "models:\n  gpt-4o.mini:\n    api_key: [{env: M}]\n    description: d\n",
        )
        .unwrap();
        let notes = adopt_own_keys(home);
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(
            notes[0].contains("It's now the key #image-generations-2"),
            "{}",
            notes[0]
        );
        let doc = read_providers_doc(home).unwrap();
        let entry = &doc["providers"]["openai"];
        assert_eq!(entry["keys"]["image-generations-2"], json!({}));
        assert_eq!(
            entry["keys"]["embeddings"],
            json!({"api_key": [{"env": "EMB"}]})
        );
        assert_eq!(
            entry["keys"]["gpt-4o-mini"],
            json!({"api_key": [{"env": "M"}]})
        );
        assert!(entry["apis"][0].get("api_key").is_none());
        // The saved key moved with it.
        assert!(home
            .join("providers/openai/keys/image-generations-2")
            .exists());
        assert!(!home
            .join("providers/openai/api-key-image-generations")
            .exists());
        let models = read_provider_file(home, "openai", "configured-models.yaml").unwrap();
        assert_eq!(models["models"]["gpt-4o.mini"], json!({"description": "d"}));
        // Once only.
        assert!(adopt_own_keys(home).is_empty());
    }

    #[test]
    fn a_keys_last_test_is_listed_until_the_key_changes() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api: chat-completions\n",
        );
        set_api_key_in(home, "p#work", "k-1").unwrap();
        record_key_check_in(home, "p", "#work", "ok", "k-1");
        let check = |home: &Path| key_names_in(home, "p").unwrap()["keys"][0]["check"].clone();
        assert_eq!(check(home)["status"], "ok");
        assert!(check(home)["at"].as_u64().is_some());
        // The fingerprint isn't the key.
        let file = std::fs::read_to_string(home.join("providers/p/key-checks.json")).unwrap();
        assert!(!file.contains("k-1"), "{file}");
        // A rename carries it along; a new value drops it.
        rename_named_key_in(home, "p", "work", "acme").unwrap();
        assert_eq!(check(home)["status"], "ok");
        set_api_key_in(home, "p#acme", "k-2").unwrap();
        assert!(check(home).is_null());
        // A key that changed behind Lexi's back (a different value) isn't tested.
        record_key_check_in(home, "p", "#acme", "rejected", "k-old");
        assert!(check(home).is_null());
        record_key_check_in(home, "p", "#acme", "rejected", "k-2");
        assert_eq!(check(home)["status"], "rejected");
        remove_named_key_in(home, "p", "acme").unwrap();
        assert!(!home.join("providers/p/key-checks.json").exists());
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
