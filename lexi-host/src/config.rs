//! Config, API-definition, and API-key loading.
//!
//! Reads `~/.lexi-cli/` (overridable via `LEXI_HOME`), shared with the Python
//! CLI. The API definitions are embedded in the binary, with a per-user
//! override under `~/.lexi-cli/apis/`.

use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value};

use crate::keystore::{self, current_secrets, KeyStore, Lookup, Secrets, Source};
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
    // An API's own api_key replaces the provider's (never merged).
    if let Some(k) = binding.get("api_key") {
        eff["api_key"] = k.clone();
    }
    Ok(eff)
}

/// The `api_key` sources in `cfg` — a provider entry, or a binding's or
/// model's effective config (whose own `api_key` replaces the provider's) — in
/// the order they're tried.
fn key_sources(cfg: &Value) -> Result<Vec<Source>, HostError> {
    keystore::parse_sources(cfg.get("api_key")).map_err(HostError::Config)
}

/// Look for the key `cfg`'s `api_key` sources lead to.
pub(crate) fn lookup_api_key(cfg: &Value) -> Result<Lookup, HostError> {
    Ok(keystore::lookup(
        &lexi_home(),
        &key_sources(cfg)?,
        current_secrets(),
    ))
}

/// Resolve an API key from `cfg`'s `api_key` sources (a provider entry, or a
/// binding's/model's effective config — pass the effective config when there
/// is one, so an API's or model's own `api_key` applies). The first source
/// with a key wins; a read error (a locked keychain, an unreadable file)
/// stops the lookup and is the error.
pub fn resolve_api_key(provider_name: &str, provider_cfg: &Value) -> Result<String, HostError> {
    let lookup = lookup_api_key(provider_cfg)?;
    if let Some(key) = lookup.key.clone() {
        return Ok(key);
    }
    if let Some((source, err)) = lookup.failure() {
        return Err(read_error(provider_name, source, err));
    }
    Err(HostError::MissingApiKey(if lookup.checked.is_empty() {
        format!("no API key for provider {provider_name:?}: its api_key in providers-config.yaml names no source")
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

/// Which kind of source the key comes from — `"env"`, `"keychain"`,
/// `"file"`, `"config"` (a literal) — without revealing it; `None` when no
/// source has one. Same order as [`resolve_api_key`] (a read error is an
/// error), but a keychain item is only checked for, not read, so this never
/// makes macOS ask for Keychain access.
pub fn api_key_source(
    provider_name: &str,
    provider_cfg: &Value,
) -> Result<Option<&'static str>, HostError> {
    let lookup = keystore::probe(&lexi_home(), &key_sources(provider_cfg)?, current_secrets());
    if let Some((source, err)) = lookup.failure() {
        return Err(read_error(provider_name, source, err));
    }
    Ok(lookup.used().map(Source::kind))
}

/// Where a saved key went (`set_api_key`, `move_api_key`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedKey {
    /// `"keychain"` or `"file"`.
    pub store: &'static str,
    /// The keychain item or file, as messages name it.
    pub location: String,
    /// Whether that source was added to providers-config.yaml.
    pub added: bool,
    /// The source the key is now read from — another one when an earlier
    /// source (e.g. a set environment variable) has a key.
    pub used: Option<String>,
}

impl SavedKey {
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "store": self.store,
            "location": self.location,
            "added": self.added,
            "used": self.used,
        })
    }
}

/// The `api_key` list a key operation works on: the provider's, or (with
/// `api`) that API's own. `own` is false when the API has none and uses the
/// provider's.
struct KeyScope {
    doc: Map,
    entry: Map,
    api: Option<String>,
    sources: Vec<Source>,
    own: bool,
}

fn key_scope(home: &Path, provider: &str, api: Option<&str>) -> Result<KeyScope, HostError> {
    check_provider_name(provider)?;
    let mut doc = read_providers_doc(home)?;
    let entry = raw_entry(&mut doc, provider)?;
    let resolved = resolve_template(&Value::Object(entry.clone()))?;
    let (sources, own) = match api {
        None => (key_sources(&resolved)?, true),
        Some(a) => {
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
        api: api.map(String::from),
        sources,
        own,
    })
}

/// Keychain accounts any provider's `api_key` (or an API's) names.
fn keychain_accounts(doc: &Map, except: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let Some(providers) = doc.get("providers").and_then(Value::as_object) else {
        return out;
    };
    for (name, entry) in providers {
        if Some(name.as_str()) == except {
            continue;
        }
        for sources in entry_source_lists(entry) {
            for s in sources {
                if let Source::Keychain(a) = s {
                    out.push(a);
                }
            }
        }
    }
    out
}

/// The `api_key` lists written in a raw provider entry: its own and each of
/// its APIs'. Unparseable ones are skipped.
fn entry_source_lists(entry: &Value) -> Vec<Vec<Source>> {
    let mut lists = Vec::new();
    if let Ok(s) = keystore::parse_sources(entry.get("api_key")) {
        lists.push(s);
    }
    for b in entry
        .get("apis")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Ok(s) = keystore::parse_sources(b.get("api_key")) {
            lists.push(s);
        }
    }
    lists
}

/// A new place to save a key for `provider` (or its `api`): a keychain item
/// named after it (not one another provider uses), or its key file.
fn new_key_source(
    home: &Path,
    doc: &Map,
    provider: &str,
    api: Option<&str>,
    keychain: bool,
) -> Source {
    let label = match api {
        Some(a) => format!("{provider}/{a}"),
        None => provider.to_string(),
    };
    if keychain {
        let taken = keychain_accounts(doc, None);
        let base = keystore::account(home, &label);
        let account = std::iter::once(base.clone())
            .chain((2..).map(|n| format!("{base} {n}")))
            .find(|a| !taken.contains(a))
            .expect("an unused name");
        Source::Keychain(account)
    } else {
        Source::File(match api {
            Some(a) => format!("providers/{provider}/api-key-{a}"),
            None => format!("providers/{provider}/api-key"),
        })
    }
}

/// Write `scope`'s list back into providers-config.yaml.
fn write_scope_sources(
    home: &Path,
    provider: &str,
    scope: KeyScope,
    sources: &[Source],
) -> Result<(), HostError> {
    match scope.api {
        None => {
            let (mut doc, mut entry) = (scope.doc, scope.entry);
            entry.insert("api_key".into(), keystore::sources_value(sources));
            providers_mut(&mut doc).insert(provider.to_string(), Value::Object(entry));
            write_yaml(&providers_config_path(home), &Value::Object(doc))
        }
        Some(api) => {
            let mut b = Map::new();
            b.insert("api".into(), Value::from(api));
            b.insert("api_key".into(), keystore::sources_value(sources));
            add_provider_api_in(home, provider, &b).map(|_| ())
        }
    }
}

fn no_keychain() -> HostError {
    HostError::Config(if keystore::NATIVE {
        "the keychain is turned off (LEXI_KEY_STORE=file)".into()
    } else {
        "this platform has no keychain Lexi can use; save the key in a file instead".into()
    })
}

fn write_secret(
    home: &Path,
    source: &Source,
    key: &str,
    secrets: Option<&dyn Secrets>,
) -> Result<(), HostError> {
    match source {
        Source::Keychain(account) => {
            secrets
                .ok_or_else(no_keychain)?
                .set(account, key)
                .map_err(|e| {
                    HostError::Config(format!(
                        "couldn't save the API key in the {}: {e}",
                        KeyStore::Keychain.label()
                    ))
                })
        }
        Source::File(raw) => {
            let path = keystore::file_path(home, raw);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, format!("{key}\n"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        }
        other => Err(HostError::Invalid(format!(
            "Lexi can't save a key in {}",
            other.describe()
        ))),
    }
}

fn read_secret(
    home: &Path,
    source: &Source,
    secrets: Option<&dyn Secrets>,
) -> Result<Option<String>, HostError> {
    let lookup = keystore::lookup(home, std::slice::from_ref(source), secrets);
    match lookup.failure() {
        Some((s, e)) => Err(HostError::Config(format!(
            "couldn't read {}: {e}",
            s.describe()
        ))),
        None => Ok(lookup.key),
    }
}

/// Delete a saved key; whether there was one. A file outside `$LEXI_HOME` is
/// yours, not Lexi's — it's never deleted.
fn delete_secret(
    home: &Path,
    source: &Source,
    secrets: Option<&dyn Secrets>,
) -> Result<bool, HostError> {
    match source {
        Source::Keychain(account) => {
            secrets
                .ok_or_else(no_keychain)?
                .delete(account)
                .map_err(|e| {
                    HostError::Config(format!(
                        "couldn't remove the API key from the {}: {e}",
                        KeyStore::Keychain.label()
                    ))
                })
        }
        Source::File(raw) => {
            let path = keystore::file_path(home, raw);
            if !path.starts_with(home) {
                return Err(HostError::Invalid(format!(
                    "{} isn't in Lexi's folder, so Lexi won't delete it; remove it yourself or change providers-config.yaml",
                    path.display()
                )));
            }
            if path.exists() {
                std::fs::remove_file(path)?;
                return Ok(true);
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// The source `provider`'s (or its `api`'s) key is read from now.
fn used_source(
    home: &Path,
    provider: &str,
    api: Option<&str>,
    secrets: Option<&dyn Secrets>,
) -> Option<String> {
    let scope = key_scope(home, provider, api).ok()?;
    let lookup = keystore::probe(home, &scope.sources, secrets);
    lookup.used().map(Source::describe)
}

/// What a key operation on `provider` (or its `api`) sees: the sources that
/// apply (the API's own, else the provider's), whether they're the API's
/// own, and where a saved key would go (`adds`: that place isn't in the
/// config yet).
pub(crate) struct KeyPlan {
    pub(crate) sources: Vec<Source>,
    pub(crate) own: bool,
    pub(crate) save_to: Source,
    pub(crate) adds: bool,
    pub(crate) lookup: Lookup,
}

/// `read`: read the key that's used (a keychain item's secret too — macOS may
/// ask for access); otherwise keychain items are only checked for.
pub(crate) fn key_plan(
    provider: &str,
    api: Option<&str>,
    read: bool,
) -> Result<KeyPlan, HostError> {
    let home = lexi_home();
    let secrets = current_secrets();
    let scope = key_scope(&home, provider, api)?;
    let sources = if scope.own {
        scope.sources.clone()
    } else {
        key_scope(&home, provider, None)?.sources
    };
    let (save_to, adds) = match scope.sources.iter().find(|s| s.writable()) {
        Some(s) => (s.clone(), false),
        None => (
            new_key_source(&home, &scope.doc, provider, api, secrets.is_some()),
            true,
        ),
    };
    let lookup = if read {
        keystore::lookup(&home, &sources, secrets)
    } else {
        keystore::probe(&home, &sources, secrets)
    };
    Ok(KeyPlan {
        sources,
        own: scope.own,
        save_to,
        adds,
        lookup,
    })
}

/// Save an API key for `provider` — or, with `api`, a key for that API only.
/// It goes in the first keychain item or file its `api_key` lists; when the
/// list has neither, one is added to providers-config.yaml (a keychain item on
/// macOS and Windows, else the owner-only `providers/<name>/api-key`) after
/// the sources already there (a literal key in the config is replaced).
pub fn set_api_key(
    provider_name: &str,
    api: Option<&str>,
    key: &str,
) -> Result<SavedKey, HostError> {
    set_api_key_in(&lexi_home(), provider_name, api, key, current_secrets())
}

fn set_api_key_in(
    home: &Path,
    provider_name: &str,
    api: Option<&str>,
    key: &str,
    secrets: Option<&dyn Secrets>,
) -> Result<SavedKey, HostError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(HostError::Config("API key cannot be empty".into()));
    }
    let scope = key_scope(home, provider_name, api)?;
    let (target, added) = match scope.sources.iter().find(|s| s.writable()) {
        Some(s) => (s.clone(), false),
        None => (
            new_key_source(home, &scope.doc, provider_name, api, secrets.is_some()),
            true,
        ),
    };
    write_secret(home, &target, key, secrets)?;
    if added {
        let mut list: Vec<Source> = scope
            .sources
            .iter()
            .filter(|s| !matches!(s, Source::Literal(_)))
            .cloned()
            .collect();
        list.push(target.clone());
        write_scope_sources(home, provider_name, scope, &list)?;
    }
    Ok(SavedKey {
        store: target.kind(),
        location: target.describe(),
        added,
        used: used_source(home, provider_name, api, secrets),
    })
}

fn uses_provider_key(provider: &str, api: &str) -> HostError {
    HostError::Invalid(format!(
        "the {api} API has no api_key of its own; it uses {provider}'s key (leave out the API)"
    ))
}

/// Delete `provider`'s (or its `api`'s) saved key — the one in the first
/// keychain item or file its `api_key` lists; providers-config.yaml is left as
/// it is. Returns whether there was one.
pub fn remove_api_key(provider_name: &str, api: Option<&str>) -> Result<bool, HostError> {
    remove_api_key_in(&lexi_home(), provider_name, api, current_secrets())
}

fn remove_api_key_in(
    home: &Path,
    provider_name: &str,
    api: Option<&str>,
    secrets: Option<&dyn Secrets>,
) -> Result<bool, HostError> {
    let scope = key_scope(home, provider_name, api)?;
    if let (Some(a), false) = (api, scope.own) {
        return Err(uses_provider_key(provider_name, a));
    }
    match scope.sources.iter().find(|s| s.writable()) {
        Some(s) => delete_secret(home, s, secrets),
        None => Ok(false),
    }
}

/// Move `provider`'s (or its `api`'s) saved key to the keychain (`to =
/// "keychain"`) or a key file (`"file"`): it's written there and read back,
/// providers-config.yaml then names the new place instead of the old, and the
/// old copy is deleted (a file outside `$LEXI_HOME` is left alone).
pub fn move_api_key(
    provider_name: &str,
    api: Option<&str>,
    to: &str,
) -> Result<SavedKey, HostError> {
    move_api_key_in(&lexi_home(), provider_name, api, to, current_secrets())
}

fn move_api_key_in(
    home: &Path,
    provider_name: &str,
    api: Option<&str>,
    to: &str,
    secrets: Option<&dyn Secrets>,
) -> Result<SavedKey, HostError> {
    let to_keychain = match to {
        "keychain" => true,
        "file" => false,
        other => {
            return Err(HostError::Invalid(format!(
                "move a key to \"keychain\" or \"file\", not {other:?}"
            )))
        }
    };
    if to_keychain && secrets.is_none() {
        return Err(no_keychain());
    }
    let scope = key_scope(home, provider_name, api)?;
    if let (Some(a), false) = (api, scope.own) {
        return Err(uses_provider_key(provider_name, a));
    }
    let Some(from) = scope.sources.iter().find(|s| s.writable()).cloned() else {
        return Err(HostError::Invalid(format!(
            "{provider_name} has no saved key to move: its api_key lists no keychain item or file"
        )));
    };
    if from.kind() == to {
        return Ok(SavedKey {
            store: from.kind(),
            location: from.describe(),
            added: false,
            used: used_source(home, provider_name, api, secrets),
        });
    }
    let key = read_secret(home, &from, secrets)?.ok_or_else(|| {
        HostError::Invalid(format!("there's no key in {} to move", from.describe()))
    })?;
    let target = new_key_source(home, &scope.doc, provider_name, api, to_keychain);
    write_secret(home, &target, &key, secrets)?;
    if read_secret(home, &target, secrets)?.as_deref() != Some(key.as_str()) {
        return Err(HostError::Config(format!(
            "{} didn't return the key it was given; nothing was changed",
            target.describe()
        )));
    }
    let list: Vec<Source> = scope
        .sources
        .iter()
        .map(|s| {
            if *s == from {
                target.clone()
            } else {
                s.clone()
            }
        })
        .collect();
    write_scope_sources(home, provider_name, scope, &list)?;
    match &from {
        Source::File(raw) if !keystore::file_path(home, raw).starts_with(home) => {}
        _ => {
            delete_secret(home, &from, secrets)?;
        }
    }
    Ok(SavedKey {
        store: target.kind(),
        location: target.describe(),
        added: false,
        used: used_source(home, provider_name, api, secrets),
    })
}

/// One-time upgrade of an older config: a provider whose `api_key` is still
/// the old single form and that has a key file (`providers/<name>/api-key`,
/// which older versions read first) gets `api_key: [{file: …}, …]`, so the
/// same key keeps being used and the config says where it is. Best effort.
pub(crate) fn upgrade_key_sources(home: &Path) {
    let Ok(mut doc) = read_providers_doc(home) else {
        return;
    };
    let mut changed = false;
    for (name, entry) in providers_mut(&mut doc).iter_mut() {
        let legacy = matches!(entry.get("api_key"), None | Some(Value::String(_)));
        let rel = format!("providers/{name}/api-key");
        if !legacy || !home.join(&rel).is_file() {
            continue;
        }
        let Ok(resolved) = resolve_template(entry) else {
            continue;
        };
        let mut list = vec![Source::File(rel)];
        list.extend(key_sources(&resolved).unwrap_or_default());
        if let Some(o) = entry.as_object_mut() {
            o.insert("api_key".into(), keystore::sources_value(&list));
            changed = true;
        }
    }
    if changed {
        let _ = write_yaml(&providers_config_path(home), &Value::Object(doc));
    }
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
    pub default: bool,
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
    let default = read_provider_file(home, provider, "default-model.yaml")?
        .get("default_model")
        .and_then(Value::as_str)
        .map(str::to_string);
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
                default: default.as_deref() == Some(id.as_str()),
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

/// The provider's current default model id, if set.
pub fn default_model(provider: &str) -> Result<Option<String>, HostError> {
    default_model_in(&lexi_home(), provider)
}

fn default_model_in(home: &Path, provider: &str) -> Result<Option<String>, HostError> {
    Ok(read_provider_file(home, provider, "default-model.yaml")?
        .get("default_model")
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// Set (or clear, with `None`) the provider's default model.
pub fn set_default_model(provider: &str, model: Option<&str>) -> Result<(), HostError> {
    set_default_model_in(&lexi_home(), provider, model)
}

fn set_default_model_in(home: &Path, provider: &str, model: Option<&str>) -> Result<(), HostError> {
    let data = match model {
        Some(m) => serde_json::json!({ "default_model": m }),
        None => Value::Object(Map::new()),
    };
    write_yaml(&provider_file(home, provider, "default-model.yaml"), &data)
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
/// making it the default. Returns whether the model already existed. Mirrors the
/// CLI's `models add`/`set` (both upsert-merge).
pub fn upsert_model(
    provider: &str,
    model: &str,
    meta: &Map,
    set_default: bool,
) -> Result<bool, HostError> {
    upsert_model_in(&lexi_home(), provider, model, meta, set_default)
}

fn upsert_model_in(
    home: &Path,
    provider: &str,
    model: &str,
    meta: &Map,
    set_default: bool,
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
    if set_default {
        set_default_model_in(home, provider, Some(model))?;
    }
    Ok(existed)
}

/// Remove a model. Errors if absent. If it was the default, the default falls to
/// the first remaining model (or is cleared when none remain).
pub fn delete_model(provider: &str, model: &str) -> Result<(), HostError> {
    delete_model_in(&lexi_home(), provider, model)
}

fn delete_model_in(home: &Path, provider: &str, model: &str) -> Result<(), HostError> {
    let mut data = match read_provider_file(home, provider, "configured-models.yaml")? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let new_default;
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
        new_default = models.keys().next().cloned();
    }
    let was_default = default_model_in(home, provider)?.as_deref() == Some(model);
    write_yaml(
        &provider_file(home, provider, "configured-models.yaml"),
        &Value::Object(data),
    )?;
    if was_default {
        set_default_model_in(home, provider, new_default.as_deref())?;
    }
    Ok(())
}

/// Load `config.json` (the CLI's app settings), or an empty object if absent.
/// Caller-supplied defaults are layered on top by the frontend.
pub fn load_config() -> Result<Value, HostError> {
    load_config_in(&lexi_home())
}

fn load_config_in(home: &Path) -> Result<Value, HostError> {
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

fn set_config_in(home: &Path, updates: &Map) -> Result<Value, HostError> {
    let mut cfg = match load_config_in(home)? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    for (k, v) in updates {
        cfg.insert(k.clone(), v.clone());
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
    if name.trim().is_empty() || name.contains(['/', '\\']) || name == ".." || name == "." {
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
/// `providers/<name>/` directory.
pub fn rename_provider(old: &str, new: &str) -> Result<(), HostError> {
    rename_provider_in(&lexi_home(), old, new, current_secrets())
}

fn rename_provider_in(
    home: &Path,
    old: &str,
    new: &str,
    secrets: Option<&dyn Secrets>,
) -> Result<(), HostError> {
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
    // Keys Lexi saved under the old name follow it: keychain items Lexi named
    // after the provider (copied first — nothing changes if that fails — and
    // the old ones dropped last), and key files in its folder.
    let keychain_err = |e: String| {
        HostError::Config(format!(
            "couldn't move the API key in the {}: {e}",
            KeyStore::Keychain.label()
        ))
    };
    let mut moves: Vec<(String, String)> = Vec::new();
    if let Some(entry) = providers.get_mut(new) {
        let old_dir = format!("providers/{old}/");
        let rename_source = |s: Source, moves: &mut Vec<(String, String)>| match s {
            Source::Keychain(a) => {
                let renamed = std::iter::once(old.to_string())
                    .chain(api_in_account(&a, old).map(|api| format!("{old}/{api}")))
                    .find(|label| keystore::account(home, label) == a)
                    .map(|label| keystore::account(home, &format!("{new}{}", &label[old.len()..])));
                match renamed {
                    Some(n) => {
                        moves.push((a, n.clone()));
                        Source::Keychain(n)
                    }
                    None => Source::Keychain(a),
                }
            }
            Source::File(p) => match p.strip_prefix(&old_dir) {
                Some(rest) => Source::File(format!("providers/{new}/{rest}")),
                None => Source::File(p),
            },
            other => other,
        };
        rewrite_entry_sources(entry, |list| {
            list.into_iter()
                .map(|s| rename_source(s, &mut moves))
                .collect()
        });
    }
    if let Some(store) = secrets {
        for (from, to) in &moves {
            if let Some(key) = store.get(from).map_err(keychain_err)? {
                store.set(to, &key).map_err(keychain_err)?;
            }
        }
    } else if !moves.is_empty() {
        return Err(no_keychain());
    }
    write_yaml(&providers_config_path(home), &Value::Object(doc))?;
    let old_dir = home.join("providers").join(old);
    if old_dir.exists() {
        std::fs::rename(&old_dir, home.join("providers").join(new))?;
    }
    if let Some(store) = secrets {
        for (from, _) in &moves {
            store.delete(from).map_err(keychain_err)?;
        }
    }
    Ok(())
}

/// The API named in a keychain account Lexi made for `provider/api`
/// (`openai/image-generations` → `image-generations`), if it is one.
fn api_in_account(account: &str, provider: &str) -> Option<String> {
    let rest = account.strip_prefix(provider)?.strip_prefix('/')?;
    // A non-default home's accounts end in ` (<home>)`.
    Some(match rest.find(" (") {
        Some(i) => rest[..i].to_string(),
        None => rest.to_string(),
    })
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

/// Remove a provider: its config entry, its `providers/<name>/` directory
/// (models, aliases, default model, key files) and the keychain items its
/// `api_key` lists name (unless another provider uses them too).
pub fn remove_provider(name: &str) -> Result<(), HostError> {
    remove_provider_in(&lexi_home(), name, current_secrets())
}

fn remove_provider_in(
    home: &Path,
    name: &str,
    secrets: Option<&dyn Secrets>,
) -> Result<(), HostError> {
    check_provider_name(name)?;
    let mut doc = read_providers_doc(home)?;
    let others = keychain_accounts(&doc, Some(name));
    let Some(entry) = providers_mut(&mut doc).shift_remove(name) else {
        return Err(HostError::UnknownProvider(name.to_string()));
    };
    // Its keychain items go too — unless another provider names the same one.
    if let Some(store) = secrets {
        for sources in entry_source_lists(&entry) {
            for s in sources {
                if let Source::Keychain(a) = s {
                    if !others.contains(&a) {
                        store.delete(&a).map_err(|e| {
                            HostError::Config(format!(
                                "couldn't remove the API key from the {}: {e}",
                                KeyStore::Keychain.label()
                            ))
                        })?;
                    }
                }
            }
        }
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
        let existed = upsert_model_in(dir.path(), "openai", "gpt-4o", &meta, true).unwrap();
        assert!(!existed);
        assert_eq!(
            default_model_in(dir.path(), "openai").unwrap().as_deref(),
            Some("gpt-4o")
        );

        // Second upsert merges new fields over the existing entry.
        let mut meta2 = Map::new();
        meta2.insert("context_window".into(), json!(128000));
        let existed = upsert_model_in(dir.path(), "openai", "gpt-4o", &meta2, false).unwrap();
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
        upsert_model_in(dir.path(), "openai", "a", &empty, true).unwrap();
        upsert_model_in(dir.path(), "openai", "b", &empty, false).unwrap();

        // Deleting the default promotes the first remaining model.
        delete_model_in(dir.path(), "openai", "a").unwrap();
        assert_eq!(
            default_model_in(dir.path(), "openai").unwrap().as_deref(),
            Some("b")
        );

        // Deleting the last model clears the default.
        delete_model_in(dir.path(), "openai", "b").unwrap();
        assert_eq!(default_model_in(dir.path(), "openai").unwrap(), None);

        assert!(delete_model_in(dir.path(), "openai", "gone").is_err());
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

        let kc = crate::keystore::test_support::MemSecrets::default();
        set_api_key_in(home, "first", None, "first-key", Some(&kc)).unwrap();
        set_api_key_in(home, "mid", None, "mid-key", None).unwrap();
        rename_provider_in(home, "first", "alpha", Some(&kc)).unwrap();
        rename_provider_in(home, "mid", "middle", None).unwrap();
        // Keys Lexi saved follow the provider to its new name.
        assert!(home.join("providers/middle/api-key").exists());
        assert_eq!(
            entry_key(home, "middle"),
            json!([{"file": "providers/middle/api-key"}])
        );
        let alpha = keystore::account(home, "alpha");
        assert_eq!(entry_key(home, "alpha"), json!([{"keychain": alpha}]));
        assert_eq!(kc.items.borrow()[&alpha], "first-key");
        assert!(!kc
            .items
            .borrow()
            .contains_key(&keystore::account(home, "first")));
        assert!(rename_provider_in(home, "nope", "x", None).is_err());
        assert!(rename_provider_in(home, "alpha", "last", None).is_err());

        let doc = read_providers_doc(home).unwrap();
        let names: Vec<&String> = doc["providers"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["alpha", "last", "middle"]);
        assert_eq!(doc["defaults"]["max_tokens"], 1); // other sections kept

        remove_provider_in(home, "middle", None).unwrap();
        assert!(!home.join("providers/middle").exists());
        assert!(remove_provider_in(home, "middle", None).is_err());
        // Removing a provider deletes its keychain key too.
        remove_provider_in(home, "alpha", Some(&kc)).unwrap();
        assert!(kc.items.borrow().is_empty());
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

    fn lookup_in(home: &Path, cfg: &Value, kc: Option<&dyn Secrets>) -> Lookup {
        keystore::lookup(home, &key_sources(cfg).unwrap(), kc)
    }

    #[test]
    fn key_sources_are_tried_in_order_and_only_a_missing_key_moves_on() {
        use crate::keystore::test_support::MemSecrets;
        use crate::keystore::Outcome;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let kc = MemSecrets::default();
        kc.items
            .borrow_mut()
            .insert("p".into(), "from-keychain".into());
        std::fs::write(home.join("k.txt"), "from-file\n").unwrap();
        let cfg = json!({"api_key": [
            "${LEXI_TEST_SURELY_UNSET_VAR}",
            {"keychain": "nothing-here"},
            {"keychain": "p"},
            {"file": "k.txt"},
        ]});
        let found = lookup_in(home, &cfg, Some(&kc));
        assert_eq!(found.key.as_deref(), Some("from-keychain"));
        let states: Vec<&str> = found.checked.iter().map(|(_, o)| o.state()).collect();
        assert_eq!(states, ["missing", "missing", "used", "not checked"]);
        assert!(found
            .summary()
            .starts_with("$LEXI_TEST_SURELY_UNSET_VAR (not set), "));

        // No keychain (LEXI_KEY_STORE=file): its items count as missing.
        let found = lookup_in(home, &cfg, None);
        assert_eq!(found.key.as_deref(), Some("from-file"));
        assert!(matches!(found.checked[1].1, Outcome::Unavailable(_)));

        // A set variable comes first when it's listed first.
        let env_first = json!({"api_key": [{"env": "PATH"}, {"keychain": "p"}]});
        assert_eq!(
            lookup_in(home, &env_first, Some(&kc))
                .used()
                .map(Source::kind),
            Some("env")
        );

        // A locked keychain stops the lookup — the file after it isn't used.
        let locked = MemSecrets {
            fail: true,
            ..Default::default()
        };
        let found = lookup_in(home, &cfg, Some(&locked));
        assert_eq!(found.key, None);
        assert_eq!(found.failure().map(|(_, e)| e), Some("keychain locked"));
        assert_eq!(found.checked[3].1, Outcome::NotChecked);

        // The older forms still read; bad entries are config errors.
        assert_eq!(
            lookup_in(home, &json!({"api_key": "literal"}), None)
                .key
                .as_deref(),
            Some("literal")
        );
        assert!(key_sources(&json!({"api_key": {"vault": "x"}})).is_err());
        assert!(key_sources(&json!({"api_key": [{"env": ""}]})).is_err());
        assert!(key_sources(&json!({})).unwrap().is_empty());
    }

    #[test]
    fn a_status_check_never_reads_a_keychain_secret() {
        use crate::keystore::test_support::MemSecrets;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let kc = MemSecrets::default();
        kc.items.borrow_mut().insert("p".into(), "secret".into());
        let sources = key_sources(&json!({"api_key": [
            "${LEXI_TEST_SURELY_UNSET_VAR}", {"keychain": "p"}, {"file": "never-read"},
        ]}))
        .unwrap();
        let probed = keystore::probe(home, &sources, Some(&kc));
        assert_eq!(probed.used().map(Source::kind), Some("keychain"));
        assert_eq!(probed.key, None);
        assert_eq!(kc.reads.get(), 0); // macOS wouldn't have asked
        let read = keystore::lookup(home, &sources, Some(&kc));
        assert_eq!(read.key.as_deref(), Some("secret"));
        assert_eq!(kc.reads.get(), 1);
        // Missing and locked items probe the same as they read.
        assert!(keystore::probe(home, &sources[..1], Some(&kc))
            .used()
            .is_none());
        let locked = MemSecrets {
            fail: true,
            ..Default::default()
        };
        assert!(keystore::probe(home, &sources, Some(&locked))
            .failure()
            .is_some());
    }

    #[test]
    fn saving_uses_the_listed_place_or_adds_one() {
        use crate::keystore::test_support::MemSecrets;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api_key: ${LEXI_TEST_SURELY_UNSET_VAR}\n  q:\n    url: u\n    api_key: sk-literal\n",
        );
        let kc = MemSecrets::default();
        let account = keystore::account(home, "p");
        assert!(account.starts_with("p (") && account.contains(&home.display().to_string()));

        // No keychain item or file listed: one is added after the variable.
        let saved = set_api_key_in(home, "p", None, " k1 ", Some(&kc)).unwrap();
        assert_eq!((saved.store, saved.added), ("keychain", true));
        assert_eq!(saved.used.as_deref(), Some(saved.location.as_str()));
        assert_eq!(
            entry_key(home, "p"),
            json!([{"env": "LEXI_TEST_SURELY_UNSET_VAR"}, {"keychain": account}])
        );
        assert_eq!(kc.items.borrow()[&account], "k1");
        // Saving again writes the same item; the config doesn't change.
        let again = set_api_key_in(home, "p", None, "k2", Some(&kc)).unwrap();
        assert!(!again.added);
        assert_eq!(kc.items.borrow()[&account], "k2");

        // Without a keychain it's an owner-only file; a literal key is replaced.
        let saved = set_api_key_in(home, "q", None, "k3", None).unwrap();
        assert_eq!(
            (saved.store, saved.location.as_str()),
            ("file", "providers/q/api-key")
        );
        assert_eq!(
            entry_key(home, "q"),
            json!([{"file": "providers/q/api-key"}])
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.join("providers/q/api-key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(set_api_key_in(home, "q", None, "  ", None).is_err());
        assert!(set_api_key_in(home, "nope", None, "k", None).is_err());

        // Remove deletes the saved key and leaves the config as it is.
        assert!(remove_api_key_in(home, "p", None, Some(&kc)).unwrap());
        assert!(!remove_api_key_in(home, "p", None, Some(&kc)).unwrap());
        assert_eq!(entry_key(home, "p").as_array().unwrap().len(), 2);
        assert!(remove_api_key_in(home, "q", None, None).unwrap());
        assert!(!home.join("providers/q/api-key").exists());

        // A file outside Lexi's folder is never deleted.
        let mine = tempfile::NamedTempFile::new().unwrap();
        write_providers(
            home,
            &format!(
                "providers:\n  r:\n    url: u\n    api_key: {{file: {}}}\n",
                mine.path().display()
            ),
        );
        assert!(remove_api_key_in(home, "r", None, None).is_err());
        assert!(mine.path().exists());
    }

    #[test]
    fn a_locked_keychain_is_an_error_not_a_silent_fallback() {
        use crate::keystore::test_support::MemSecrets;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(home, "providers:\n  s:\n    url: u\n");
        let locked = MemSecrets {
            fail: true,
            ..Default::default()
        };
        let e = set_api_key_in(home, "s", None, "k", Some(&locked))
            .unwrap_err()
            .to_string();
        assert!(e.contains("couldn't save the API key"), "{e}");
        assert!(!home.join("providers/s/api-key").exists());
        assert!(entry_key(home, "s").is_null());
    }

    #[test]
    fn an_api_can_have_its_own_key() {
        use crate::keystore::test_support::MemSecrets;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api_key: {keychain: main}\n    apis:\n      - api: chat-completions\n      - api: embeddings\n        for: [kind:embedding]\n",
        );
        let kc = MemSecrets::default();
        kc.items
            .borrow_mut()
            .insert("main".into(), "main-key".into());

        // Without a list of its own an API uses the provider's key.
        assert!(!key_scope(home, "p", Some("embeddings")).unwrap().own);
        assert!(remove_api_key_in(home, "p", Some("embeddings"), Some(&kc)).is_err());
        assert!(key_scope(home, "p", Some("nope")).is_err());

        let saved = set_api_key_in(home, "p", Some("embeddings"), "emb-key", Some(&kc)).unwrap();
        assert!(saved.added);
        let account = keystore::account(home, "p/embeddings");
        assert_eq!(saved.location, keystore::keychain_place(&account));
        let doc = read_providers_doc(home).unwrap();
        let entry = &doc["providers"]["p"];
        assert_eq!(entry["apis"][1]["api_key"], json!([{"keychain": account}]));
        assert_eq!(entry["apis"][1]["for"], json!(["kind:embedding"]));
        assert_eq!(entry["api_key"], json!({"keychain": "main"}));

        // The binding's effective config carries its own list, not the provider's.
        let eff = binding_config(entry, &entry["apis"][1]).unwrap();
        assert_eq!(
            lookup_in(home, &eff, Some(&kc)).key.as_deref(),
            Some("emb-key")
        );
        let chat = binding_config(entry, &entry["apis"][0]).unwrap();
        assert_eq!(
            lookup_in(home, &chat, Some(&kc)).key.as_deref(),
            Some("main-key")
        );
    }

    #[test]
    fn keys_move_between_the_keychain_and_a_file() {
        use crate::keystore::test_support::MemSecrets;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write_providers(
            home,
            "providers:\n  p:\n    url: u\n    api_key: [{env: LEXI_TEST_SURELY_UNSET_VAR}, {file: providers/p/api-key}]\n",
        );
        let kc = MemSecrets::default();
        assert!(move_api_key_in(home, "p", None, "keychain", Some(&kc)).is_err()); // nothing saved
        set_api_key_in(home, "p", None, "the-key", None).unwrap();
        assert!(move_api_key_in(home, "p", None, "keychain", None).is_err()); // no keychain
        assert!(move_api_key_in(home, "p", None, "vault", Some(&kc)).is_err());

        let moved = move_api_key_in(home, "p", None, "keychain", Some(&kc)).unwrap();
        let account = keystore::account(home, "p");
        assert_eq!(moved.store, "keychain");
        assert_eq!(
            entry_key(home, "p"),
            json!([{"env": "LEXI_TEST_SURELY_UNSET_VAR"}, {"keychain": account}])
        );
        assert!(!home.join("providers/p/api-key").exists());
        assert_eq!(kc.items.borrow()[&account], "the-key");

        let back = move_api_key_in(home, "p", None, "file", Some(&kc)).unwrap();
        assert_eq!(back.location, "providers/p/api-key");
        assert!(kc.items.borrow().is_empty());
        assert_eq!(
            std::fs::read_to_string(home.join("providers/p/api-key")).unwrap(),
            "the-key\n"
        );
        // Already there: nothing to do.
        assert!(
            !move_api_key_in(home, "p", None, "file", Some(&kc))
                .unwrap()
                .added
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
            json!([{"file": "providers/openai/api-key"}, {"env": "OPENAI_API_KEY"}])
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
