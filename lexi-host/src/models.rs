//! Model tags and resolution: which binding (API) serves a model, and the
//! model's effective tags. The decisions are `lexi_core::classify_model`; this
//! module supplies the provider entry, API definitions, configured models and
//! the user's tags (`providers/<p>/model-tags.yaml`).

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::{config, HostError};

/// The tag vocabulary (`lexi tags`).
pub fn tag_vocabulary() -> Value {
    lexi_core::tag_vocabulary()
}

fn tags_path(provider: &str) -> PathBuf {
    config::lexi_home()
        .join("providers")
        .join(provider)
        .join("model-tags.yaml")
}

/// The user's tags for a provider: `{model-id-or-glob: [ops]}`.
pub fn user_model_tags(provider: &str) -> Result<Map<String, Value>, HostError> {
    let path = tags_path(provider);
    if !path.exists() {
        return Ok(Map::new());
    }
    let doc: Value = serde_yaml::from_str(&std::fs::read_to_string(&path)?)?;
    Ok(doc
        .get("tags")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

fn write_user_tags(provider: &str, tags: &Map<String, Value>) -> Result<(), HostError> {
    let path = tags_path(provider);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let doc = json!({ "tags": tags });
    std::fs::write(&path, serde_yaml::to_string(&doc)?)?;
    Ok(())
}

fn require_provider(provider: &str) -> Result<Value, HostError> {
    config::load_providers()?
        .get("providers")
        .and_then(|p| p.get(provider))
        .cloned()
        .ok_or_else(|| HostError::UnknownProvider(provider.to_string()))
}

/// Add or remove tags on a model id or glob (`ops`: `+tag` / `tag` / `-tag`).
/// A new op on a tag replaces any earlier op on the same tag. Returns the key's
/// ops after the change.
pub fn tag_model(provider: &str, key: &str, ops: &[String]) -> Result<Vec<String>, HostError> {
    require_provider(provider)?;
    if key.trim().is_empty() {
        return Err(HostError::Invalid(
            "a model id or pattern is required".into(),
        ));
    }
    let mut parsed = Vec::new();
    for op in ops {
        parsed.push(lexi_core::parse_tag_op(op).map_err(HostError::Invalid)?);
    }
    let mut all = user_model_tags(provider)?;
    let mut current: Vec<String> = all
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    for (add, tag) in parsed {
        current.retain(|op| op.trim_start_matches(['+', '-']) != tag);
        current.push(format!("{}{tag}", if add { "+" } else { "-" }));
    }
    all.insert(key.to_string(), json!(current));
    write_user_tags(provider, &all)?;
    Ok(current)
}

/// Remove all of the user's tags on a model id or glob; false if it had none.
pub fn clear_model_tags(provider: &str, key: &str) -> Result<bool, HostError> {
    require_provider(provider)?;
    let mut all = user_model_tags(provider)?;
    let had = all.shift_remove(key).is_some();
    if had {
        write_user_tags(provider, &all)?;
    }
    Ok(had)
}

/// The API definitions a provider's bindings (and `extra`) name.
fn binding_defs(
    provider_cfg: &Value,
    extra: Option<&str>,
) -> Result<Map<String, Value>, HostError> {
    let mut defs = Map::new();
    let names = lexi_core::provider_bindings(provider_cfg)
        .into_iter()
        .filter_map(|b| b["api"].as_str().map(String::from))
        .chain(extra.map(String::from));
    for name in names {
        if !defs.contains_key(&name) {
            defs.insert(name.clone(), config::load_api_def(&name)?);
        }
    }
    Ok(defs)
}

/// A configured model's explicit `api:` (it wins over tags).
fn pinned_api(provider: &str, model: &str) -> Result<Option<String>, HostError> {
    Ok(config::load_models(provider)?
        .into_iter()
        .find(|m| m.id == model)
        .and_then(|m| {
            m.metadata
                .get("api")
                .and_then(Value::as_str)
                .map(String::from)
        }))
}

/// Classify one model: `{provider, model, kind, api, binding, tags: [{tag,
/// source}], error?}` (see `lexi_core::classify_model`). No network I/O.
pub fn classify(provider: &str, model: &str) -> Result<Value, HostError> {
    let pcfg = require_provider(provider)?;
    let entry = crate::hosted::cached_entry(provider, &pcfg, model);
    classify_with(provider, &pcfg, model, entry)
}

/// [`classify`] with the listing entry already in hand.
fn classify_with(
    provider: &str,
    pcfg: &Value,
    model: &str,
    entry: Option<Value>,
) -> Result<Value, HostError> {
    let pcfg = pcfg.clone();
    let pinned = pinned_api(provider, model)?;
    let defs = binding_defs(&pcfg, pinned.as_deref())?;
    let user = Value::Object(user_model_tags(provider)?);
    let catalog = crate::hosted::catalog_cfg(&pcfg);
    let catalog_ops = entry
        .as_ref()
        .map(|e| json!(lexi_core::catalog_tags(e, &catalog)))
        .unwrap_or(Value::Null);
    let mut out =
        lexi_core::classify_model(&pcfg, &defs, model, &user, &catalog_ops, pinned.as_deref());
    out["provider"] = Value::from(provider);
    out["in_catalog"] = Value::Bool(entry.is_some());
    if let Some(e) = &entry {
        let facts = lexi_core::catalog_facts(e, &catalog);
        out["limits"] = facts["limits"].clone();
        if let Some(n) = facts.get("name") {
            out["name"] = n.clone();
        }
    }
    Ok(out)
}

/// A model resolved to the binding that serves it.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub provider: String,
    /// The model id (aliases resolved).
    pub model: String,
    /// The API definition serving it.
    pub api: String,
    pub kind: Option<String>,
    /// `[{tag, source}]`.
    pub tags: Vec<Value>,
    /// The binding's effective config, with the model's own overrides merged.
    pub effective: Value,
    /// Limits from the provider's catalog (`context_window`,
    /// `max_output_tokens`, …), when the cached listing has the model.
    pub limits: Map<String, Value>,
}

impl ResolvedModel {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t["tag"] == tag)
    }
}

/// Resolve which model a request uses and which binding serves it.
///
/// - `model`: an alias or id; `None` picks the default for the kind of model
///   the first binding whose API produces `output` (default `text`) serves:
///   your default for that kind (`models default`), else the binding's
///   `default_model`, else (the catch-all binding) the provider's.
/// - `output` (`text`, `image`, `vector`, …): what the request asks for. With
///   a named model and no `output`, the model's API decides (an image model
///   makes images); with both, a model whose API doesn't produce `output` is
///   refused.
/// - The model must be configured, an alias, in the cached catalog, or
///   recognized by a `kind:` tag from a rule or the user's tags (typo
///   protection).
pub fn resolve_model(
    provider: &str,
    model: Option<&str>,
    output: Option<&str>,
) -> Result<ResolvedModel, HostError> {
    let want = output.unwrap_or("text");
    let pcfg = require_provider(provider)?;
    let configured = config::load_models(provider)?;
    let bindings = lexi_core::provider_bindings(&pcfg);
    if bindings.is_empty() {
        return resolve_inline(provider, &pcfg, model);
    }

    let (model, explicit) = match model {
        Some(m) => {
            let aliases = config::model_aliases(provider)?;
            match aliases.get(m).and_then(Value::as_str) {
                Some(target) => (target.to_string(), false),
                None => (m.to_string(), true),
            }
        }
        None => {
            let defs = binding_defs(&pcfg, None)?;
            let idx = lexi_core::binding_for_output(&pcfg, &defs, want).ok_or_else(|| {
                HostError::Unsupported(format!(
                    "provider {provider:?} has no API configured that produces {want}"
                ))
            })?;
            let kind = binding_kind(&defs, &bindings[idx]);
            let mine = config::default_models(provider)?;
            let (m, _) = kind_default(&pcfg, &bindings, idx, &kind, &mine).ok_or_else(|| {
                HostError::Config(format!(
                    "no default {kind} model for {provider} (pass --model, or set one with `lexi models default {provider} MODEL`)"
                ))
            })?;
            (m, false)
        }
    };

    let cls = classify(provider, &model)?;
    if let Some(err) = cls.get("error").and_then(Value::as_str) {
        return Err(HostError::Unsupported(err.to_string()));
    }
    let tags: Vec<Value> = cls["tags"].as_array().cloned().unwrap_or_default();
    let recognized = tags.iter().any(|t| {
        t["tag"].as_str().is_some_and(|x| x.starts_with("kind:"))
            && t["source"].as_str().is_some_and(|s| !s.starts_with("api:"))
    });
    let in_catalog = cls["in_catalog"].as_bool().unwrap_or(false);
    if explicit
        && !recognized
        && !in_catalog
        && !configured.is_empty()
        && !configured.iter().any(|x| x.id == model)
    {
        let aliases = config::model_aliases(provider)?;
        let mut msg =
            format!("model {model:?} is not a configured model or alias for provider {provider:?}");
        if !aliases.is_empty() {
            let mut names: Vec<&str> = aliases.keys().map(String::as_str).collect();
            names.sort();
            msg.push_str(&format!("\navailable aliases: {}", names.join(", ")));
        }
        let mut ids: Vec<&str> = configured.iter().map(|x| x.id.as_str()).collect();
        ids.sort();
        msg.push_str(&format!("\nconfigured models: {}", ids.join(", ")));
        return Err(HostError::Config(msg));
    }

    let api = cls["api"].as_str().unwrap_or("").to_string();
    let binding = cls["binding"]
        .as_u64()
        .and_then(|i| bindings.get(i as usize).cloned())
        .unwrap_or_else(|| json!({ "api": api }));
    let mut effective = config::binding_config(&pcfg, &binding)?;
    // APIs that put the model in the URL (`/genai/{model}`).
    if let Some(p) = effective.pointer_mut("/request/path") {
        if let Some(s) = p.as_str().filter(|s| s.contains("{model}")) {
            *p = Value::from(s.replace("{model}", &model));
        }
    }

    let kind = cls["kind"].as_str().map(String::from);
    let produces = effective
        .pointer("/capabilities/output")
        .and_then(Value::as_array)
        .is_some_and(|o| o.iter().any(|x| x.as_str() == Some(want)));
    // Checked when the output was asked for, or when the model is the default
    // for it; a named model with no output asked for produces what it produces.
    if !produces && (output.is_some() || !explicit) {
        let output = want;
        return Err(HostError::Unsupported(format!(
            "{model} is a kind:{} model (served by the {api} API), which doesn't produce {output}",
            kind.as_deref().unwrap_or("unknown")
        )));
    }

    // Per-model overrides of the API's auth/request/response blocks, and the
    // model's own api_key (which replaces the API's / provider's).
    if let Some(meta) = configured
        .iter()
        .find(|x| x.id == model)
        .map(|x| &x.metadata)
    {
        if let Some(cfg) = effective.as_object_mut() {
            for key in ["auth", "request", "response"] {
                if let Some(over) = meta.get(key) {
                    let merged = match cfg.get(key) {
                        Some(existing) if existing.is_object() && over.is_object() => {
                            config::deep_merge(existing, over)
                        }
                        _ => over.clone(),
                    };
                    cfg.insert(key.to_string(), merged);
                }
            }
            if let Some(k) = meta.get("api_key") {
                cfg.insert("api_key".into(), k.clone());
                cfg.insert(
                    config::KEY_SCOPE.into(),
                    serde_json::json!({ "model": format!("providers/{provider}/configured-models.yaml") }),
                );
            }
        }
    }

    Ok(ResolvedModel {
        provider: provider.to_string(),
        model,
        api,
        kind,
        tags,
        effective,
        limits: cls["limits"].as_object().cloned().unwrap_or_default(),
    })
}

/// Request-body fields that cap a reply's output tokens, across APIs.
pub const OUTPUT_TOKEN_KEYS: &[&str] =
    &["max_tokens", "max_output_tokens", "max_completion_tokens"];

impl ResolvedModel {
    /// Check a composed request against what the model's tags and limits say
    /// it accepts. Returns errors (refuse) and warnings (send anyway).
    pub fn check_request(&self, body: &Value, has_images: bool) -> (Vec<String>, Vec<String>) {
        let (mut errors, mut warnings) = (Vec::new(), Vec::new());
        let m = &self.model;
        if has_images && !self.has_tag("in:image") {
            errors.push(format!(
                "{m} doesn't accept image input (no in:image tag); if it does, tag it: \
                 `lexi models tag {} {m} +in:image` (or pass --no-validate)",
                self.provider
            ));
        }
        if let Some(limit) = self.limits.get("max_output_tokens").and_then(Value::as_u64) {
            for key in OUTPUT_TOKEN_KEYS {
                if let Some(n) = body.get(*key).and_then(Value::as_u64) {
                    if n > limit {
                        errors.push(format!(
                            "{key} {n} is more than {m} allows ({limit}); lower it (or pass --no-validate)"
                        ));
                    }
                }
            }
        }
        if self.has_tag("status:deprecated") {
            warnings.push(format!(
                "{m} is deprecated by {} and may stop working; see `lexi models hosted {}`",
                self.provider, self.provider
            ));
        }
        (errors, warnings)
    }
}

/// The kind of model a binding's API serves (`chat` when it can't be told).
fn binding_kind(defs: &Map<String, Value>, binding: &Value) -> String {
    binding["api"]
        .as_str()
        .and_then(|a| defs.get(a))
        .map(lexi_core::api_kind)
        .unwrap_or_else(|| "chat".into())
}

/// The default model of binding `idx`, which serves `kind`: yours for the
/// kind (`mine`, from `config::default_models`), else the binding's
/// `default_model`, else — the catch-all binding — the provider's. With where
/// it came from: `user`, `binding` or `provider`.
fn kind_default(
    pcfg: &Value,
    bindings: &[Value],
    idx: usize,
    kind: &str,
    mine: &Map<String, Value>,
) -> Option<(String, &'static str)> {
    let s = |v: Option<&Value>| v.and_then(Value::as_str).map(String::from);
    if let Some(m) = s(mine.get(kind)) {
        return Some((m, "user"));
    }
    if let Some(m) = s(bindings[idx].get("default_model")) {
        return Some((m, "binding"));
    }
    let catch_all = bindings.iter().position(|b| b.get("for").is_none());
    if Some(idx) == catch_all {
        return s(pcfg.get("default_model")).map(|m| (m, "provider"));
    }
    None
}

fn check_kind(kind: &str) -> Result<(), HostError> {
    lexi_core::validate_tag(&format!("kind:{kind}")).map_err(HostError::Invalid)
}

/// The provider's default model for each kind:
/// `{provider, default_model, defaults: [{kind, model, source}]}` — a row per
/// kind its APIs serve (in binding order), then any other kind you set a
/// default for; `default_model` is the chat default. `source` is `user`
/// (yours, set with `models default`), `binding` (the API binding's
/// `default_model`, usually from the provider's template) or `provider` (the
/// provider's `default_model`); `model` and `source` are null when there's
/// none. With `kind`, just that one: `{provider, kind, model, source}`.
pub fn defaults(provider: &str, kind: Option<&str>) -> Result<Value, HostError> {
    let rows = default_rows(provider)?;
    if let Some(kind) = kind {
        check_kind(kind)?;
        let row = rows.into_iter().find(|r| r["kind"] == kind);
        let field = |k: &str| row.as_ref().map(|r| r[k].clone()).unwrap_or(Value::Null);
        return Ok(json!({
            "provider": provider,
            "kind": kind,
            "model": field("model"),
            "source": field("source"),
        }));
    }
    let chat = rows
        .iter()
        .find(|r| r["kind"] == "chat")
        .map(|r| r["model"].clone())
        .unwrap_or(Value::Null);
    Ok(json!({ "provider": provider, "default_model": chat, "defaults": rows }))
}

fn default_rows(provider: &str) -> Result<Vec<Value>, HostError> {
    let pcfg = require_provider(provider)?;
    let mine = config::default_models(provider)?;
    let bindings = lexi_core::provider_bindings(&pcfg);
    let row = |kind: &str, found: Option<(String, &'static str)>| match found {
        Some((m, source)) => json!({ "kind": kind, "model": m, "source": source }),
        None => json!({ "kind": kind, "model": null, "source": null }),
    };
    let mut rows: Vec<Value> = Vec::new();
    if bindings.is_empty() {
        let found = mine
            .get("chat")
            .and_then(Value::as_str)
            .map(|m| (m.to_string(), "user"))
            .or_else(|| {
                pcfg.get("default_model")
                    .and_then(Value::as_str)
                    .map(|m| (m.to_string(), "provider"))
            });
        rows.push(row("chat", found));
    } else {
        let defs = binding_defs(&pcfg, None)?;
        for (i, b) in bindings.iter().enumerate() {
            let kind = binding_kind(&defs, b);
            if rows.iter().all(|r| r["kind"] != kind.as_str()) {
                rows.push(row(&kind, kind_default(&pcfg, &bindings, i, &kind, &mine)));
            }
        }
    }
    for (kind, m) in &mine {
        if rows.iter().all(|r| r["kind"] != kind.as_str()) {
            rows.push(row(kind, m.as_str().map(|m| (m.to_string(), "user"))));
        }
    }
    Ok(rows)
}

/// Make `model` (an id or alias) your default for its kind — the kind its tags
/// give it, as `models tags` shows. With `kind`, the model must be of that
/// kind. The model must be one Lexi can use (configured, an alias, in the
/// cached catalog, or recognized by a tag). `{provider, kind, model}`.
pub fn set_default(provider: &str, model: &str, kind: Option<&str>) -> Result<Value, HostError> {
    if let Some(k) = kind {
        check_kind(k)?;
    }
    let r = resolve_model(provider, Some(model), None)?;
    let model_kind = r.kind.clone().unwrap_or_else(|| "chat".into());
    if let Some(k) = kind.filter(|k| *k != model_kind) {
        return Err(HostError::Invalid(format!(
            "{} is a {model_kind} model, not {k} (its kind comes from its tags: `lexi models tags {provider} {}`)",
            r.model, r.model
        )));
    }
    config::set_kind_default(provider, &model_kind, Some(&r.model))?;
    Ok(json!({ "provider": provider, "kind": model_kind, "model": r.model }))
}

/// Clear your default for `kind`, so the provider's applies again.
/// `{provider, kind, cleared, model, source}`: `cleared` is whether you had
/// one; `model` / `source` the default now in effect (as `defaults`).
pub fn clear_default(provider: &str, kind: &str) -> Result<Value, HostError> {
    require_provider(provider)?;
    check_kind(kind)?;
    let had = config::default_models(provider)?.contains_key(kind);
    config::set_kind_default(provider, kind, None)?;
    let now = defaults(provider, Some(kind))?;
    Ok(json!({
        "provider": provider,
        "kind": kind,
        "cleared": had,
        "model": now["model"],
        "source": now["source"],
    }))
}

/// A provider that defines its API inline (`request:` / `response:`, no `api`):
/// one API, so no classification — the model is the given one or the default.
fn resolve_inline(
    provider: &str,
    pcfg: &Value,
    model: Option<&str>,
) -> Result<ResolvedModel, HostError> {
    let effective = config::effective_config(provider, pcfg)?;
    let model = match model {
        Some(m) => config::model_aliases(provider)?
            .get(m)
            .and_then(Value::as_str)
            .unwrap_or(m)
            .to_string(),
        None => config::default_model(provider)?
            .or_else(|| {
                pcfg.get("default_model")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .ok_or_else(|| {
                HostError::Config("no model (pass --model or set a default model)".into())
            })?,
    };
    Ok(ResolvedModel {
        provider: provider.to_string(),
        model,
        api: String::new(),
        kind: None,
        tags: Vec::new(),
        effective,
        limits: Map::new(),
    })
}

/// The provider's configured models with each one's classification:
/// `[{id, name, description, default, aliases, metadata, kind, api, tags}]`.
pub fn model_summaries(provider: &str) -> Result<Vec<Value>, HostError> {
    require_provider(provider)?;
    config::load_models(provider)?
        .into_iter()
        .map(|m| {
            let cls = classify(provider, &m.id)?;
            let mut v = serde_json::to_value(&m)?;
            v["kind"] = cls["kind"].clone();
            v["api"] = cls["api"].clone();
            v["tags"] = Value::Array(
                cls["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.get("tag").cloned())
                    .collect(),
            );
            Ok(v)
        })
        .collect()
}

/// The provider's hosted models (the catalog), each with Lexi's
/// classification added: the provider's entry plus `kind`, `api` and `tags`
/// (tag names). `cached` reads the last fetch instead of the network (an error
/// if there is none); otherwise fetches and refreshes the cache. With `tags`,
/// only models having all of them.
pub async fn hosted_summaries(
    provider: &str,
    cached: bool,
    tags: &[String],
) -> Result<Vec<Value>, HostError> {
    let pcfg = require_provider(provider)?;
    for t in tags {
        lexi_core::validate_tag(t).map_err(HostError::Invalid)?;
    }
    let entries = if cached {
        crate::hosted::cached_catalog(provider)?
            .and_then(|c| c.get("models").and_then(Value::as_array).cloned())
            .ok_or_else(|| {
                HostError::Invalid(format!(
                    "no cached catalog for {provider}; run `lexi models hosted {provider}` first"
                ))
            })?
    } else {
        crate::hosted::fetch_hosted_models(provider).await?
    };
    let id_path = crate::hosted::catalog_cfg(&pcfg)
        .get("id_path")
        .and_then(Value::as_str)
        .unwrap_or("id")
        .to_string();
    let mut out = Vec::new();
    for entry in entries {
        let Some(id) = lexi_core::get_path(&entry, &id_path)
            .and_then(Value::as_str)
            .map(String::from)
        else {
            continue;
        };
        let cls = classify_with(provider, &pcfg, &id, Some(entry.clone()))?;
        let names: Vec<Value> = cls["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.get("tag").cloned())
            .collect();
        if !tags.iter().all(|t| names.iter().any(|n| n == t.as_str())) {
            continue;
        }
        let mut v = entry;
        if let Some(o) = v.as_object_mut() {
            o.insert("kind".into(), cls["kind"].clone());
            o.insert("api".into(), cls["api"].clone());
            o.insert("tags".into(), Value::Array(names));
        }
        out.push(v);
    }
    Ok(out)
}
