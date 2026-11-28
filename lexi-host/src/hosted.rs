//! Hosted-model discovery: the provider's model catalog (`GET /models`).
//!
//! The provider's `catalog:` config (or the older `hosted_models` /
//! `hosted_model_fields`) says where the listing is, how it pages, and how its
//! fields map to tags and limits (`lexi_core::catalog_tags` / `catalog_facts`).
//! `models hosted` fetches it live and caches it in
//! `providers/<p>/catalog.json`; classification reads only the cache, so
//! resolving a model never makes a hidden network call.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{auth, config, HostError};

/// How long to wait for a provider's models endpoint (matches the CLI).
const HOSTED_TIMEOUT: Duration = Duration::from_secs(15);

/// Normalize a hosted-models response to a flat list (mirrors the Node handler):
/// an object's `data` or `models` array, else its values; a top-level array
/// as-is; anything else an empty list.
fn normalize_hosted_items(data: &Value) -> Vec<Value> {
    if let Some(obj) = data.as_object() {
        if let Some(arr) = obj.get("data").and_then(Value::as_array) {
            return arr.clone();
        }
        if let Some(arr) = obj.get("models").and_then(Value::as_array) {
            return arr.clone();
        }
        return obj.values().cloned().collect();
    }
    if let Some(arr) = data.as_array() {
        return arr.clone();
    }
    Vec::new()
}

/// A provider's catalog config: `catalog:`, else one built from the older
/// `hosted_models` / `hosted_model_fields`.
pub(crate) fn catalog_cfg(cfg: &Value) -> Value {
    if let Some(c) = cfg.get("catalog").filter(|c| c.is_object()) {
        return c.clone();
    }
    let mut c = serde_json::Map::new();
    if let Some(p) = cfg.get("hosted_models") {
        c.insert("path".into(), p.clone());
    }
    if let Some(f) = cfg.get("hosted_model_fields") {
        c.insert("fields".into(), f.clone());
    }
    Value::Object(c)
}

/// Compute the models endpoint from an effective config: the base URL —
/// normalized to a `/v1` root if it embeds `/v1/...` — plus the catalog path
/// (default `/models`).
fn hosted_endpoint(effective_cfg: &Value) -> String {
    let mut base = effective_cfg
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    if let Some(idx) = base.find("/v1/") {
        base = format!("{}/v1", &base[..idx]);
    } else if let Some(stripped) = base.strip_suffix("/v1/messages") {
        base = format!("{stripped}/v1");
    }
    let catalog = catalog_cfg(effective_cfg);
    let path = catalog
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("/models")
        .to_string();
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// At most this many pages per listing.
const MAX_PAGES: usize = 50;

/// Fetch and normalize a provider's hosted models over an already-resolved
/// effective config. No file/config I/O, so it is testable against a mock server.
pub async fn fetch_hosted_models_with(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
) -> Result<Vec<Value>, HostError> {
    let endpoint = hosted_endpoint(effective_cfg);
    let catalog = catalog_cfg(effective_cfg);
    let paginate = catalog.get("paginate").filter(|p| p.is_object());
    let items_path = catalog.get("items_path").and_then(Value::as_str);
    let mut all = Vec::new();
    let mut after: Option<Value> = None;
    for _ in 0..MAX_PAGES {
        let mut query: Vec<(String, String)> = Vec::new();
        if let Some(p) = paginate {
            if let (Some(param), Some(limit)) =
                (p.get("limit_param").and_then(Value::as_str), p.get("limit"))
            {
                query.push((param.into(), crate::config_scalar(limit)));
            }
            if let (Some(param), Some(a)) = (p.get("after_param").and_then(Value::as_str), &after) {
                query.push((param.into(), crate::config_scalar(a)));
            }
        }
        let resp = client
            .get(&endpoint)
            .query(&query)
            .headers(auth::build_headers(effective_cfg, api_key))
            .timeout(HOSTED_TIMEOUT)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(HostError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }
        let data: Value = resp.json().await?;
        let page = match items_path.and_then(|p| lexi_core::get_path(&data, p)) {
            Some(Value::Array(a)) => a.clone(),
            _ => normalize_hosted_items(&data),
        };
        all.extend(page);
        let Some(p) = paginate else { break };
        let more = p
            .get("has_more")
            .and_then(Value::as_str)
            .and_then(|k| data.get(k))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let last = p
            .get("last_id")
            .and_then(Value::as_str)
            .and_then(|k| data.get(k))
            .filter(|v| !v.is_null())
            .cloned();
        match (more, last) {
            (true, Some(l)) => after = Some(l),
            _ => break,
        }
    }
    Ok(all)
}

fn cache_path(provider: &str) -> std::path::PathBuf {
    config::lexi_home()
        .join("providers")
        .join(provider)
        .join("catalog.json")
}

/// The provider's cached catalog, written by [`fetch_hosted_models`]:
/// `{fetched_at, models}` (`fetched_at` in Unix seconds), or `None` if it has
/// never been fetched.
pub fn cached_catalog(provider: &str) -> Result<Option<Value>, HostError> {
    let path = cache_path(provider);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&std::fs::read_to_string(path)?)?))
}

/// The cached listing entry for `model` (matched on the catalog's `id_path`,
/// default `id`).
pub(crate) fn cached_entry(provider: &str, cfg: &Value, model: &str) -> Option<Value> {
    let cache = cached_catalog(provider).ok().flatten()?;
    let id_path = catalog_cfg(cfg)
        .get("id_path")
        .and_then(Value::as_str)
        .unwrap_or("id")
        .to_string();
    cache
        .get("models")?
        .as_array()?
        .iter()
        .find(|e| lexi_core::get_path(e, &id_path).and_then(Value::as_str) == Some(model))
        .cloned()
}

/// How [`fetch_hosted_models`] gets `provider`'s listing, without asking it:
/// `{provider, url, catalog, source, template}`.
/// - `url`: the models endpoint (the provider's base URL plus the catalog's
///   `path`, default `/models`), fetched with `GET` and the provider's auth.
/// - `catalog`: the catalog settings that apply — `path`, `paginate`,
///   `items_path`, `id_path`, `fields`, `tags`, `limits`, `name`, … (the
///   older `hosted_models` / `hosted_model_fields` shown as `path` /
///   `fields`); `{}` when there are none.
/// - `source`: where they come from — `config` (providers-config.yaml, over
///   the template's if it has one), `template` (the shipped template named by
///   `template`) or `default` (none: the listing is read from the response's
///   `data` or `models` list).
pub fn hosted_schema(provider: &str) -> Result<Value, HostError> {
    let providers = config::load_providers()?;
    let provider_cfg = providers
        .get("providers")
        .and_then(|p| p.get(provider))
        .cloned()
        .ok_or_else(|| HostError::UnknownProvider(provider.to_string()))?;
    let eff = config::effective_config(provider, &provider_cfg)?;
    let raw = Value::Object(config::raw_provider_entry(provider)?);
    let template = raw.get("template").and_then(Value::as_str);
    let has_catalog = |v: &Value| catalog_cfg(v).as_object().is_some_and(|c| !c.is_empty());
    let catalog = catalog_cfg(&eff);
    let source = if has_catalog(&raw) {
        "config"
    } else if template.is_some() && has_catalog(&provider_cfg) {
        "template"
    } else {
        "default"
    };
    Ok(serde_json::json!({
        "provider": provider,
        "url": hosted_endpoint(&eff),
        "catalog": catalog,
        "source": source,
        "template": template,
    }))
}

/// Load config, resolve the provider, and fetch its hosted models.
pub async fn fetch_hosted_models(provider: &str) -> Result<Vec<Value>, HostError> {
    let providers = config::load_providers()?;
    let provider_cfg = providers
        .get("providers")
        .and_then(|p| p.get(provider))
        .cloned()
        .ok_or_else(|| HostError::UnknownProvider(provider.to_string()))?;
    let eff = config::effective_config(provider, &provider_cfg)?;
    let api_key = config::resolve_api_key(provider, &eff)?;
    let client = reqwest::Client::new();
    let models = fetch_hosted_models_with(&client, &eff, &api_key).await?;
    // Cache it: classification reads models' catalog data from here.
    let fetched_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = cache_path(provider);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        serde_json::to_string(&serde_json::json!({ "fetched_at": fetched_at, "models": models }))?,
    )?;
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_handles_data_models_values_and_array() {
        assert_eq!(
            normalize_hosted_items(&json!({ "data": [{ "id": "a" }] })).len(),
            1
        );
        assert_eq!(
            normalize_hosted_items(&json!({ "models": [{ "id": "a" }, { "id": "b" }] })).len(),
            2
        );
        assert_eq!(normalize_hosted_items(&json!([{ "id": "a" }])).len(), 1);
        assert!(normalize_hosted_items(&json!("nope")).is_empty());
    }

    #[test]
    fn endpoint_normalizes_v1_base() {
        let cfg = json!({ "url": "https://x.test/v1/messages", "hosted_models": "/models" });
        assert_eq!(hosted_endpoint(&cfg), "https://x.test/v1/models");

        let cfg = json!({ "url": "https://x.test/v1/chat/completions" });
        assert_eq!(hosted_endpoint(&cfg), "https://x.test/v1/models");

        let cfg = json!({ "url": "https://x.test", "hosted_models": "models" });
        assert_eq!(hosted_endpoint(&cfg), "https://x.test/models");
    }
}

/// A provider's API-key readiness, as `providers status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeyStatus {
    /// `"ready"`, `"no api-key"`, or `"invalid api-key"`.
    pub status: &'static str,
    /// The key checked: `#work`, `#` (the unnamed key) or `$OPENAI_API_KEY`
    /// (the provider's variable); none when there was nothing to check.
    pub key: Option<String>,
    /// Why there's no key to check (`no api-key`): the provider has none,
    /// several and no default, or the key's sources found nothing.
    pub reason: Option<String>,
    /// Where the key comes from (see [`config::api_key_source`]).
    pub source: Option<&'static str>,
    /// The file or variable it was read from.
    pub from: Option<String>,
    /// Whether the provider answered and accepted the key; `false` when no key
    /// is set, the key was refused, or the check couldn't reach a verdict
    /// (timeout, 5xx, no models endpoint) — then `status` is still `"ready"`.
    pub verified: bool,
}

/// How long `check_api_key` waits for the models endpoint.
const KEY_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Check a provider's API key — the one a request would use (§12.4: the
/// chosen key, else the variable when set, else the default) — with a live
/// authenticated GET to its models endpoint: 401/403 means `invalid
/// api-key`; success — or any other outcome that can't prove the key bad
/// (timeouts, 5xx, no endpoint) — means `ready`; nothing to use is `no
/// api-key`, with the reason.
pub async fn check_api_key(provider: &str) -> Result<KeyStatus, HostError> {
    let providers = config::load_providers()?;
    let provider_cfg = providers
        .get("providers")
        .and_then(|p| p.get(provider))
        .cloned()
        .ok_or_else(|| HostError::UnknownProvider(provider.to_string()))?;
    let eff = config::effective_config(provider, &provider_cfg)?;
    // Check afresh: a command runs again rather than answering from memory.
    crate::keystore::forget_commands();
    let (lookup, key) = match config::lookup_chosen_key(provider, &eff, true) {
        Ok(found) => found,
        Err(HostError::MissingApiKey(why)) => {
            return Ok(KeyStatus {
                status: "no api-key",
                key: None,
                reason: Some(why),
                source: None,
                from: None,
                verified: false,
            })
        }
        Err(e) => return Err(e),
    };
    if let Some((s, e)) = lookup.failure() {
        return Err(HostError::Config(format!(
            "couldn't read {provider}'s API key from {}: {e}",
            s.describe()
        )));
    }
    let (Some(api_key), Some(used)) = (lookup.key.clone(), lookup.used()) else {
        return Ok(KeyStatus {
            status: "no api-key",
            reason: Some(format!("no API key found (looked in {})", lookup.summary())),
            key: Some(key),
            source: None,
            from: None,
            verified: false,
        });
    };
    let (source, from) = (used.kind(), used.describe());
    let client = reqwest::Client::builder()
        .timeout(KEY_CHECK_TIMEOUT)
        .build()?;
    let (status, verified) = match fetch_hosted_models_with(&client, &eff, &api_key).await {
        Ok(_) => ("ready", true),
        Err(HostError::HttpStatus {
            status: 401 | 403, ..
        }) => ("invalid api-key", false),
        Err(_) => ("ready", false),
    };
    Ok(KeyStatus {
        status,
        key: Some(key),
        reason: None,
        source: Some(source),
        from: Some(from),
        verified,
    })
}

impl KeyStatus {
    /// `{key, status, source, from, verified, reason}`, the key written as
    /// `label` (`openai#work`, `$OPENAI_API_KEY`).
    pub fn to_value(&self, label: &str) -> Value {
        json!({
            "key": label,
            "status": self.status,
            "source": self.source,
            "from": self.from,
            "verified": self.verified,
            "reason": self.reason,
        })
    }
}

/// Check every key `provider` has, and its variable when it's set
/// (`docs/key-storage-design.md` §12.5: `providers status` with no key
/// named): `{provider, keys: [{key, status, source, from, verified,
/// reason}], variable: {name, set, status?, verified?}, default_key, uses,
/// ready, reason}` — each key as [`check_api_key`] reports it (`key` written
/// `openai#work`; one that couldn't be read has `error`), and what a request
/// that names no key would use ([`config::key_names`]).
pub async fn check_provider_keys(provider: &str) -> Result<Value, HostError> {
    let names = config::key_names(provider)?;
    let mut rows = Vec::new();
    for k in names["keys"].as_array().cloned().unwrap_or_default() {
        let name = k["name"].as_str().unwrap_or_default().to_string();
        let label = format!("{provider}{}", config::key_label(&name));
        let row = match config::with_key(Some(name), check_api_key(provider)).await {
            Ok(st) => st.to_value(&label),
            Err(e) => json!({ "key": label, "error": e.to_string() }),
        };
        rows.push(row);
    }
    let mut variable = names["variable"].clone();
    if names["variable"]["set"] == true {
        let check = check_api_key(provider);
        match config::with_key(Some(config::VARIABLE_KEY.to_string()), check).await {
            Ok(st) => {
                variable["status"] = Value::from(st.status);
                variable["verified"] = Value::from(st.verified);
            }
            Err(e) => variable["error"] = Value::from(e.to_string()),
        }
    }
    Ok(json!({
        "provider": provider,
        "keys": rows,
        "variable": variable,
        "default_key": names["default_key"],
        "uses": names["uses"],
        "ready": names["ready"],
        "reason": names["reason"],
    }))
}
