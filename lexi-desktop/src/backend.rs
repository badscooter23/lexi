//! `/api/*` request handling for the reused web frontend.
//!
//! The frontend still speaks the Node server's HTTP contract; a `fetch`-shim in
//! the webview routes every `/api/*` call to the `api` Tauri command, which
//! dispatches here. Shapes match the original Node handlers. Every endpoint is
//! served in-process by `lexi-host`: chat (buffered + streaming, fanned out over
//! providers concurrently) plus the full admin surface (providers, models,
//! prompts, config, API definitions) and image generation (the Images tab).
//! Only chat, image generation and hosted-model discovery call a provider; the
//! rest read/write `~/.lexi-cli/`.

use serde_json::{json, Map, Value};

use lexi_host::config;

/// Dispatch a shimmed `/api/*` request to a response value.
pub async fn handle(method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let base = path.split('?').next().unwrap_or(path);
    match (method, base) {
        ("GET", "/api/providers") | ("GET", "/api/providers/") => providers(),
        ("GET", "/api/providers/config-source") => providers_config_source(),
        ("GET", "/api/config") | ("GET", "/api/config/") => Ok(config_get()),
        ("GET", "/api/themes") => Ok(themes()),
        ("GET", "/api/startup") => startup(path),
        ("POST", "/api/startup/seen") => startup_seen(),
        ("POST", "/api/providers/api-key") => provider_api_key(body.unwrap_or(Value::Null)),
        ("POST", "/api/providers/check") => provider_check(body.unwrap_or(Value::Null)).await,
        ("POST", "/api/open-url") => open_url(body.unwrap_or(Value::Null)),
        ("GET", "/api/prompts") => prompts_list(),
        ("GET", p) if p.starts_with("/api/models/default") => models_default_get(path),
        ("GET", p) if p.starts_with("/api/models/aliases") => models_aliases_get(path),
        ("GET", p) if p.starts_with("/api/models/hosted") => models_hosted(path).await,
        ("GET", p) if p.starts_with("/api/models") => models(path),
        ("GET", p) if p.starts_with("/api/definitions/raw/") => api_raw(base),
        ("GET", "/api/definitions") | ("GET", "/api/definitions/") => api_list(),
        ("GET", p) if p.starts_with("/api/definitions/validate/") => api_validate(base),
        ("GET", p) if p.starts_with("/api/definitions/") => api_show(base),
        ("GET", p) if p.starts_with("/api/respond/options") => Ok(json!({ "presets": [] })),
        ("POST", "/api/config") | ("POST", "/api/config/") => {
            config_set(body.unwrap_or(Value::Null))
        }
        ("POST", "/api/prompts") => prompt_save(body.unwrap_or(Value::Null)),
        ("POST", "/api/prompts/active") => prompt_active(body.unwrap_or(Value::Null)),
        ("POST", "/api/prompts/delete") => prompt_delete(body.unwrap_or(Value::Null)),
        ("POST", "/api/models/default") => models_set_default(body.unwrap_or(Value::Null)),
        ("POST", "/api/models/aliases") => models_set_alias(body.unwrap_or(Value::Null)),
        ("POST", "/api/models/aliases/delete") => models_delete_alias(body.unwrap_or(Value::Null)),
        ("POST", "/api/models/add") => models_upsert(body.unwrap_or(Value::Null), false),
        ("POST", "/api/models/set") => models_upsert(body.unwrap_or(Value::Null), true),
        ("POST", "/api/models/delete") => models_delete(body.unwrap_or(Value::Null)),
        ("POST", "/api/definitions/create") => api_create(body.unwrap_or(Value::Null)),
        ("POST", "/api/definitions/save-raw") => api_save_raw(body.unwrap_or(Value::Null)),
        ("POST", "/api/definitions/delete") => api_delete(body.unwrap_or(Value::Null)),
        ("POST", "/api/definitions/preview") => api_preview(body.unwrap_or(Value::Null)),
        ("POST", "/api/respond") => respond(body.unwrap_or(Value::Null)).await,
        ("GET", "/api/images/providers") => images_providers(),
        ("GET", "/api/images/models") => images_models(path).await,
        ("GET", "/api/images/options") => images_options(path),
        ("POST", "/api/images/generate") => images_generate(body.unwrap_or(Value::Null)).await,
        ("POST", "/api/images/reveal") => images_reveal(body.unwrap_or(Value::Null)),
        _ => Err(format!(
            "endpoint not available in the desktop app: {method} {base}"
        )),
    }
}

/// `GET /api/providers` — the normalized provider list the chat tab renders.
fn providers() -> Result<Value, String> {
    let cfg = config::load_providers().map_err(|e| e.to_string())?;
    let empty = Map::new();
    let providers = cfg
        .get("providers")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    // The catch-all API and default model, read through templates and bindings.
    let summaries = lexi_host::admin::provider_summaries().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (name, meta) in providers {
        let summary = summaries.iter().find(|s| s["name"] == name.as_str());
        let field = |k: &str| {
            summary
                .and_then(|s| s.get(k))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let url = meta.get("url").and_then(Value::as_str).unwrap_or("");
        let default_model = field("default_model");
        let api = field("api");
        // A `${VAR}` key counts only when the variable is set.
        let has_key = config::api_key_source(name, meta).ok().flatten().is_some();
        let configured = !url.is_empty() && has_key && !default_model.is_empty();
        // Whether it can chat: a binding that produces text, serving a chat
        // model by default. The Chat tab lists only these providers.
        let chat = lexi_host::resolve_model(name, None, Some("text"))
            .is_ok_and(|r| r.kind.as_deref().is_none_or(|k| k == "chat"));

        out.push(json!({
            "name": name,
            "url": url,
            "hosted_models": summary.and_then(|s| s.get("hosted_models")).filter(|v| !v.is_null()).cloned().unwrap_or(json!("")),
            "hosted_model_fields": summary.and_then(|s| s.get("hosted_model_fields")).filter(|v| !v.is_null()).cloned().unwrap_or(json!([])),
            "default_model": default_model,
            "api": api,
            "api_key": if has_key { "[set]" } else { "[missing]" },
            "configured": configured,
            "chat": chat,
            "source": "providers-config.yaml",
        }));
    }
    Ok(json!({ "providers": out, "source": "providers-config.yaml" }))
}

/// `GET /api/providers/config-source` — the text of `providers-config.yaml`
/// and where it is (the Providers tab's Show button; `providers list --raw`).
fn providers_config_source() -> Result<Value, String> {
    let path = config::lexi_home().join("providers/providers-config.yaml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    Ok(
        json!({ "source": "providers-config.yaml", "path": path.display().to_string(), "text": text }),
    )
}

/// `GET /api/models?provider=X` — a provider's configured models, enriched with
/// its default flag and aliases. Mirrors the Node `{ provider, models: [...] }`
/// shape; the chat tab reads `id`/`name`, the Models tab the rest.
fn models(path: &str) -> Result<Value, String> {
    let provider = query_param(path, "provider")
        .ok_or_else(|| "provider query param is required".to_string())?;

    // Configured models with their classification (kind, serving API, tags).
    let models = lexi_host::models::model_summaries(&provider).map_err(|e| e.to_string())?;
    let models: Vec<Value> = models
        .into_iter()
        .map(|m| {
            json!({
                "id": m["id"],
                "name": m["name"],
                "description": m["description"],
                "default": m["default"],
                "default_for": m["default_for"],
                "aliases": m["aliases"],
                "max_tokens": m["metadata"].get("max_tokens"),
                "context_window": m["metadata"].get("context_window"),
                "kind": m["kind"],
                "api": m["api"],
                "tags": m["tags"],
            })
        })
        .collect();
    Ok(json!({ "provider": provider, "models": models }))
}

/// `GET /api/models/default?provider=X[&kind=K]` — the provider's default
/// model for each kind (`models::defaults`), or for kind `K`.
fn models_default_get(path: &str) -> Result<Value, String> {
    let provider = require_provider(path)?;
    let kind = query_param(path, "kind");
    lexi_host::models::defaults(&provider, kind.as_deref()).map_err(|e| e.to_string())
}

/// `GET /api/models/aliases?provider=X` — the provider's `alias -> model` map.
fn models_aliases_get(path: &str) -> Result<Value, String> {
    let provider = require_provider(path)?;
    let aliases = config::model_aliases(&provider).map_err(|e| e.to_string())?;
    Ok(json!({ "provider": provider, "aliases": aliases }))
}

/// `GET /api/models/hosted?provider=X` — models the provider reports via its API
/// (a live call to the provider's models endpoint). With `cached=1`, the list
/// from the last fetch instead (no network), plus its `fetched_at` (Unix secs).
async fn models_hosted(path: &str) -> Result<Value, String> {
    let provider = require_provider(path)?;
    let cached = query_param(path, "cached").is_some_and(|v| v == "1" || v == "true");
    // The catalog, each entry with kind / api / tags (a live fetch refreshes the cache).
    let models = lexi_host::models::hosted_summaries(&provider, cached, &[])
        .await
        .map_err(|e| e.to_string())?;
    let mut out = json!({ "provider": provider, "models": models });
    if cached {
        let catalog = lexi_host::hosted::cached_catalog(&provider).map_err(|e| e.to_string())?;
        out["fetched_at"] = catalog
            .and_then(|c| c.get("fetched_at").cloned())
            .unwrap_or(Value::Null);
    }
    Ok(out)
}

/// `POST /api/models/default` — `{provider, model}` makes the model your
/// default for its kind; `{provider, kind, clear: true}` clears your default
/// for that kind.
fn models_set_default(body: Value) -> Result<Value, String> {
    let provider = require_field(&body, "provider")?;
    let kind = body.get("kind").and_then(Value::as_str);
    if body.get("clear").and_then(Value::as_bool) == Some(true) {
        let kind = kind.ok_or("kind is required to clear a default")?;
        return lexi_host::models::clear_default(&provider, kind).map_err(|e| e.to_string());
    }
    let model = require_field(&body, "model")?;
    lexi_host::models::set_default(&provider, &model, kind).map_err(|e| e.to_string())
}

/// `POST /api/models/aliases` — point an alias at a model.
fn models_set_alias(body: Value) -> Result<Value, String> {
    let provider = require_field(&body, "provider")?;
    let alias = require_field(&body, "alias")?;
    let model = require_field(&body, "model")?;
    config::set_model_alias(&provider, &alias, &model).map_err(|e| e.to_string())?;
    Ok(json!({ "provider": provider, "alias": alias, "model": model, "output": "" }))
}

/// `POST /api/models/aliases/delete` — remove an alias.
fn models_delete_alias(body: Value) -> Result<Value, String> {
    let provider = require_field(&body, "provider")?;
    let alias = require_field(&body, "alias")?;
    config::delete_model_alias(&provider, &alias).map_err(|e| e.to_string())?;
    Ok(json!({ "provider": provider, "alias": alias, "output": "" }))
}

/// `POST /api/models/add` and `/api/models/set` — add or update a model (both
/// upsert-merge). `is_set` only affects the reported action word.
fn models_upsert(body: Value, is_set: bool) -> Result<Value, String> {
    let provider = require_field(&body, "provider")?;
    let model = require_field(&body, "model")?;

    let mut meta = Map::new();
    for key in ["name", "description"] {
        if let Some(v) = body
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            meta.insert(key.into(), json!(v));
        }
    }
    for key in ["max_tokens", "context_window"] {
        if let Some(v) = int_field(&body, key) {
            meta.insert(key.into(), v);
        }
    }
    let set_default = body
        .get("set_default")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let existed =
        config::upsert_model(&provider, &model, &meta, set_default).map_err(|e| e.to_string())?;
    let action = if is_set && existed {
        "Updated"
    } else {
        "Added"
    };
    Ok(json!({
        "provider": provider,
        "model": model,
        "output": format!("{action} model {model} for {provider}."),
    }))
}

/// `POST /api/models/delete` — remove a model (fixing the default if needed).
fn models_delete(body: Value) -> Result<Value, String> {
    let provider = require_field(&body, "provider")?;
    let model = require_field(&body, "model")?;
    config::delete_model(&provider, &model).map_err(|e| e.to_string())?;
    Ok(json!({ "provider": provider, "model": model, "output": "" }))
}

/// Require a `provider` query param from a request path.
fn require_provider(path: &str) -> Result<String, String> {
    query_param(path, "provider").ok_or_else(|| "provider query param is required".to_string())
}

/// Require a non-empty string field from a request body.
fn require_field(body: &Value, key: &str) -> Result<String, String> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

/// Coerce an integer body field (number or numeric string), else `None`.
fn int_field(body: &Value, key: &str) -> Option<Value> {
    match body.get(key) {
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => Some(Value::Number(n.clone())),
        Some(Value::Number(n)) => n.as_f64().map(|f| json!(f as i64)),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok().map(|i| json!(i)),
        _ => None,
    }
}

/// `GET /api/config` — the CLI app settings, the saved `config.json` layered
/// over built-in defaults. Never errors (defaults stand in on any read failure).
fn config_get() -> Value {
    let mut cfg = json!({
        "cli_name": "lexi",
        "prompt": "lexi",
        "prompt_delimiter": ">",
        "edit_mode": "emacs",
        "request_timeout": config::DEFAULT_REQUEST_TIMEOUT,
    });
    if let Ok(Value::Object(saved)) = config::load_config() {
        let base = cfg.as_object_mut().unwrap();
        for (k, v) in saved {
            base.insert(k, v);
        }
    }
    cfg
}

/// The shipped GUI themes; copied to `themes.json` in the Lexi home on first
/// use, where the user can edit them.
const SHIPPED_THEMES: &str = include_str!("../themes.json");

/// `GET /api/themes` — the GUI themes from `themes.json` in the Lexi home, the
/// default theme, and the selected one (`theme` in `config.json`).
fn themes() -> Value {
    let selected = config_get()
        .get("theme")
        .and_then(Value::as_str)
        .map(str::to_string);
    themes_in(&config::lexi_home(), selected.as_deref())
}

/// The themes shipped before `themes.json` recorded `offered`.
const THEMES_BEFORE_OFFERED: &[&str] = &["dark-cyan", "dark-purple", "nvidia-dark", "nvidia-light"];

/// The ids of the themes in a themes file.
fn theme_ids(file: &Value) -> Vec<String> {
    file["themes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Adds to the user's file the shipped themes it has never offered (themes
/// new in this version), recording every shipped id in `offered` so a shipped
/// theme the user deleted stays deleted. A file without `offered` counts the
/// themes it has as offered. Returns the names of the added themes, and
/// whether the file changed.
///
/// A file without `offered` was written before it existed, by the version
/// that shipped [`THEMES_BEFORE_OFFERED`]; those count as offered.
fn add_new_shipped_themes(file: &mut Value, shipped: &Value) -> (Vec<String>, bool) {
    let had_offered = file.get("offered").and_then(Value::as_array).is_some();
    let mut offered: Vec<String> = match file.get("offered").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        None => THEMES_BEFORE_OFFERED
            .iter()
            .map(|s| s.to_string())
            .chain(theme_ids(file))
            .collect(),
    };
    let mut seen = std::collections::HashSet::new();
    offered.retain(|id| seen.insert(id.clone()));
    let present = theme_ids(file);
    let mut added = Vec::new();
    for t in shipped["themes"].as_array().into_iter().flatten() {
        let Some(id) = t["id"].as_str() else { continue };
        if offered.iter().any(|o| o == id) {
            continue;
        }
        if !present.iter().any(|p| p == id) {
            if let Some(list) = file["themes"].as_array_mut() {
                list.push(t.clone());
                added.push(t["name"].as_str().unwrap_or(id).to_string());
            }
        }
        offered.push(id.to_string());
    }
    let changed = !added.is_empty() || !had_offered;
    if changed {
        file["offered"] = json!(offered);
    }
    (added, changed)
}

/// The themes in `<home>/themes.json`, writing the shipped file there first
/// when there is none, and adding shipped themes it has never offered. An
/// unreadable or empty file falls back to the shipped themes, with a
/// `warning`. `selected` is used when it names a theme, else the file's
/// `default`, else its first theme.
fn themes_in(home: &std::path::Path, selected: Option<&str>) -> Value {
    let path = home.join("themes.json");
    let shipped: Value =
        serde_json::from_str(SHIPPED_THEMES).expect("the shipped themes.json is valid JSON");
    let mut warning: Option<String> = None;
    let mut added: Vec<String> = Vec::new();
    let write = |v: &Value| {
        std::fs::create_dir_all(home).and_then(|_| {
            std::fs::write(
                &path,
                serde_json::to_string_pretty(v).unwrap_or_default() + "\n",
            )
        })
    };
    let file = if path.exists() {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()));
        match parsed {
            Ok(mut v)
                if v.get("themes")
                    .and_then(Value::as_array)
                    .is_some_and(|t| !t.is_empty()) =>
            {
                let (new, changed) = add_new_shipped_themes(&mut v, &shipped);
                if changed {
                    if let Err(e) = write(&v) {
                        warning = Some(format!("Couldn't update {} ({e}).", path.display()));
                    }
                }
                added = new;
                v
            }
            Ok(_) => {
                warning = Some(format!(
                    "{} lists no themes; using the built-in ones.",
                    path.display()
                ));
                shipped.clone()
            }
            Err(e) => {
                warning = Some(format!(
                    "Couldn't read {} ({e}); using the built-in themes.",
                    path.display()
                ));
                shipped.clone()
            }
        }
    } else {
        let mut seeded = shipped.clone();
        seeded["offered"] = json!(theme_ids(&shipped));
        if let Err(e) = write(&seeded) {
            warning = Some(format!("Couldn't create {} ({e}).", path.display()));
        }
        shipped.clone()
    };
    let themes: Vec<Value> = file["themes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.get("id").and_then(Value::as_str).is_some() && t["vars"].is_object())
        .collect();
    let has = |id: &str| themes.iter().any(|t| t["id"] == id);
    let first = themes.first().and_then(|t| t["id"].as_str()).unwrap_or("");
    let default = file
        .get("default")
        .and_then(Value::as_str)
        .filter(|d| has(d))
        .unwrap_or(first)
        .to_string();
    let selected = selected
        .filter(|s| has(s))
        .map(str::to_string)
        .unwrap_or_else(|| default.clone());
    json!({
        "path": path.display().to_string(),
        "default": default,
        "selected": selected,
        "themes": themes,
        "elements": file.get("elements").cloned().unwrap_or(Value::Null),
        "added": added,
        "warning": warning,
    })
}

/// `GET /api/prompts` — the saved prompt library. Flattens each prompt's body
/// into `{ name, ...body, isActive }`; the `$$` prompt is the active one.
fn prompts_list() -> Result<Value, String> {
    let prompts = config::load_prompts().map_err(|e| e.to_string())?;
    let prompts: Vec<Value> = prompts
        .into_iter()
        .map(|(name, body)| {
            let mut obj = Map::new();
            obj.insert("name".into(), json!(name));
            if let Value::Object(b) = body {
                for (k, v) in b {
                    obj.insert(k, v);
                }
            }
            obj.insert("isActive".into(), json!(name == "$$"));
            Value::Object(obj)
        })
        .collect();
    Ok(json!({ "prompts": prompts }))
}

/// `POST /api/config` — merge the posted keys into `config.json`, then return the
/// updated settings (defaults layered on, as the GET does).
fn config_set(body: Value) -> Result<Value, String> {
    let updates = body
        .as_object()
        .ok_or_else(|| "config body must be an object".to_string())?;
    config::set_config(updates).map_err(|e| e.to_string())?;
    Ok(config_get())
}

/// The release notes (`CHANGELOG.md`): one section per release, newest first.
const RELEASE_NOTES: &str = include_str!("../../CHANGELOG.md");

/// This app's version.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `(version, heading, body)` for each `## ` section of the release notes;
/// `version` is the heading's first word (`0.2.0`, or `Unreleased`).
fn release_sections(text: &str) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.trim().to_string();
            let version = heading.split_whitespace().next().unwrap_or("").to_string();
            out.push((version, heading, String::new()));
        } else if let Some((_, _, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    for (_, _, body) in &mut out {
        *body = body.trim().to_string();
    }
    out
}

/// `1.2.3` → `[1, 2, 3]`; `None` for anything else.
fn version_key(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|p| p.parse().ok()).collect()
}

/// The release notes someone moving from `seen` (the last version they opened,
/// if any) to `current` hasn't read: unreleased changes, then each release
/// after `seen` up to `current` — or just `current`'s on a first run.
fn unseen_notes(text: &str, seen: Option<&str>, current: &str) -> Vec<Value> {
    let cur = version_key(current);
    let seen = seen.and_then(version_key);
    release_sections(text)
        .into_iter()
        .filter(|(version, _, body)| {
            if body.is_empty() {
                return false;
            }
            if version.eq_ignore_ascii_case("unreleased") {
                return true;
            }
            let Some(v) = version_key(version) else {
                return false;
            };
            match (&cur, &seen) {
                (Some(c), Some(s)) => v <= *c && v > *s,
                (Some(c), None) => v == *c,
                (None, _) => false,
            }
        })
        .map(|(version, heading, body)| json!({ "version": version, "title": heading, "notes": body }))
        .collect()
}

/// One provider's credential state for the startup screen, the API key panel
/// and the setup wizard: its `api_key` sources in order and what each had
/// (`sources`, the saved key among them), the one the key came from
/// (`source` kind, `from` place), the first environment variable it reads,
/// where a saved key goes (`save_to`, `store`), and the template's `setup`
/// help. Commands (`command:` sources) aren't run here.
fn provider_credentials(name: &str) -> Value {
    let cfg = lexi_host::admin::provider_config(name).unwrap_or(Value::Null);
    let setup = cfg.get("setup").cloned().unwrap_or(Value::Null);
    let mut out = json!({
        "name": name,
        "display": setup.get("name").and_then(Value::as_str).unwrap_or(name),
        "setup": setup,
        "url": cfg.get("url"),
    });
    match lexi_host::admin::api_key_info(name, None, false) {
        Ok(info) => {
            let env = info.get("env").and_then(Value::as_str);
            for k in [
                "source", "from", "key", "error", "sources", "save_to", "store",
            ] {
                out[k] = info[k].clone();
            }
            out["env"] = json!(env);
            out["env_set"] =
                json!(env.is_some_and(|v| std::env::var(v).is_ok_and(|s| !s.is_empty())));
        }
        Err(e) => {
            out["source"] = Value::Null;
            out["error"] = json!(e.to_string());
        }
    }
    out
}

/// `GET /api/startup` — what the startup screen needs: this version and the
/// last one opened (`seen_version` in `config.json`), the release notes not yet
/// seen, and each provider's credentials. `?about=1` (the header logo) gives
/// this version's notes whether or not they've been seen.
fn startup(path: &str) -> Result<Value, String> {
    let about = query_param(path, "about").is_some_and(|v| v == "1");
    let cfg = config_get();
    let seen = cfg.get("seen_version").and_then(Value::as_str);
    let names = config::load_providers()
        .map(|p| config::provider_names(&p))
        .map_err(|e| e.to_string())?;
    let providers: Vec<Value> = names.iter().map(|n| provider_credentials(n)).collect();
    let has_credentials = providers.iter().any(|p| !p["source"].is_null());
    let new_version = seen != Some(APP_VERSION);
    let notes = if about {
        unseen_notes(RELEASE_NOTES, None, APP_VERSION)
    } else if new_version {
        unseen_notes(RELEASE_NOTES, seen, APP_VERSION)
    } else {
        Vec::new()
    };
    Ok(json!({
        "version": APP_VERSION,
        "seen_version": seen,
        "new_version": new_version,
        "notes": notes,
        "providers": providers,
        "has_credentials": has_credentials,
        "platform": std::env::consts::OS,
    }))
}

/// `POST /api/startup/seen` — remember that this version has been opened.
fn startup_seen() -> Result<Value, String> {
    let mut updates = Map::new();
    updates.insert("seen_version".into(), json!(APP_VERSION));
    config::set_config(&updates).map_err(|e| e.to_string())?;
    Ok(json!({ "seen_version": APP_VERSION }))
}

/// `POST /api/providers/api-key` `{action: show|set|remove, provider,
/// reveal?, value?}` — a provider's API key: `set` saves it as the saved key
/// (the provider's owner-only key file; listed sources are never written),
/// `remove` deletes the saved one. Every action answers with the
/// provider's credentials afterwards (plus `location`/`added`/`used`, or
/// `removed`).
fn provider_api_key(body: Value) -> Result<Value, String> {
    let provider = body
        .get("provider")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "provider is required".to_string())?;
    lexi_host::admin::provider_config(provider).map_err(|e| e.to_string())?;
    let action = body.get("action").and_then(Value::as_str).unwrap_or("show");
    let mut result = json!({});
    match action {
        "show" => {
            let mut creds = provider_credentials(provider);
            if body.get("reveal").and_then(Value::as_bool) == Some(true) {
                let info = lexi_host::admin::api_key_info(provider, None, true)
                    .map_err(|e| e.to_string())?;
                creds["key"] = info["key"].clone();
            }
            return Ok(creds);
        }
        "set" => {
            let value = body.get("value").and_then(Value::as_str).unwrap_or("");
            let saved = config::set_api_key(provider, None, value).map_err(|e| e.to_string())?;
            result = saved_value(&saved);
        }
        "remove" => {
            let removed = config::remove_api_key(provider, None).map_err(|e| e.to_string())?;
            result["removed"] = json!(removed);
        }
        other => return Err(format!("unknown api-key action {other:?}")),
    }
    let mut creds = provider_credentials(provider);
    if let (Some(c), Some(r)) = (creds.as_object_mut(), result.as_object()) {
        c.extend(r.clone());
    }
    Ok(creds)
}

fn saved_value(saved: &config::SavedKey) -> Value {
    json!({
        "location": saved.location,
        "added": saved.added,
        "used": saved.used,
    })
}

/// `POST /api/providers/check` `{provider}` — test the provider's key against
/// its models endpoint (a listing; nothing is generated or billed).
async fn provider_check(body: Value) -> Result<Value, String> {
    let provider = body
        .get("provider")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "provider is required".to_string())?;
    let st = lexi_host::check_api_key(provider)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "provider": provider,
        "status": st.status,
        "source": st.source,
        "from": st.from,
        "verified": st.verified,
    }))
}

/// `POST /api/open-url` `{url}` — open a web page in the default browser
/// (http and https only).
fn open_url(body: Value) -> Result<Value, String> {
    use std::process::Command;
    let url = body
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
        .filter(|u| !u.chars().any(|c| c.is_whitespace() || c.is_control()))
        .ok_or_else(|| "only http(s) links can be opened".to_string())?;
    let status = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).status()
    } else if cfg!(target_os = "windows") {
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .status()
    } else {
        Command::new("xdg-open").arg(url).status()
    };
    status.map_err(|e| e.to_string())?;
    Ok(json!({}))
}

/// `POST /api/prompts` — save a prompt (named, or the active `$$` slot when
/// unnamed), then return the refreshed prompt list.
fn prompt_save(body: Value) -> Result<Value, String> {
    let prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Prompt text is required".to_string())?;

    let mut obj = Map::new();
    obj.insert("prompt".into(), json!(prompt));
    if let Some(v) = num_field(&body, "max_tokens") {
        obj.insert("max_tokens".into(), v);
    }
    if let Some(v) = num_field(&body, "temperature") {
        obj.insert("temperature".into(), v);
    }
    if let Some(role) = body
        .get("role")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        obj.insert("role".into(), json!(role));
    }

    let name = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("$$");
    config::save_prompt(name, &Value::Object(obj)).map_err(|e| e.to_string())?;
    prompts_list()
}

/// `POST /api/prompts/active` — make the named prompt active, then refresh the list.
fn prompt_active(body: Value) -> Result<Value, String> {
    let name = require_name(&body)?;
    config::set_active_prompt(&name).map_err(|e| e.to_string())?;
    prompts_list()
}

/// `POST /api/prompts/delete` — delete the named prompt, then refresh the list.
fn prompt_delete(body: Value) -> Result<Value, String> {
    let name = require_name(&body)?;
    config::delete_prompt(&name).map_err(|e| e.to_string())?;
    prompts_list()
}

/// Pull a required `name` string from a request body.
fn require_name(body: &Value) -> Result<String, String> {
    body.get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Prompt name is required".to_string())
}

/// Coerce a numeric prompt field: keep numbers, parse numeric strings, else drop.
fn num_field(body: &Value, key: &str) -> Option<Value> {
    match body.get(key) {
        Some(Value::Number(n)) => Some(Value::Number(n.clone())),
        Some(Value::String(s)) => serde_json::from_str::<serde_json::Number>(s.trim())
            .ok()
            .map(Value::Number),
        _ => None,
    }
}

/// `GET /api/definitions` — the available API definitions as `{ name, description }`.
fn api_list() -> Result<Value, String> {
    let defs = config::list_api_defs().map_err(|e| e.to_string())?;
    let apis: Vec<Value> = defs
        .into_iter()
        .map(|(name, description)| json!({ "name": name, "description": description }))
        .collect();
    Ok(json!({ "apis": apis }))
}

/// `GET /api/definitions/{name}` — a single definition, parsed.
fn api_show(base: &str) -> Result<Value, String> {
    let name = def_name(base, "/api/definitions/");
    let definition = config::load_api_def(&name).map_err(|e| e.to_string())?;
    Ok(json!({ "name": name, "definition": definition }))
}

/// `GET /api/definitions/raw/{name}` — a definition's raw YAML text (for the editor).
fn api_raw(base: &str) -> Result<Value, String> {
    let name = def_name(base, "/api/definitions/raw/");
    let yaml = config::load_api_raw(&name).map_err(|e| e.to_string())?;
    Ok(json!({ "name": name, "yaml": yaml }))
}

/// `GET /api/definitions/validate/{name}` — validate a stored definition.
fn api_validate(base: &str) -> Result<Value, String> {
    let name = def_name(base, "/api/definitions/validate/");
    let (valid, errors) = config::validate_api_def(&name).map_err(|e| e.to_string())?;
    Ok(json!({ "name": name, "valid": valid, "output": errors.join("\n") }))
}

/// `POST /api/definitions/create` — scaffold or clone a new definition.
fn api_create(body: Value) -> Result<Value, String> {
    let name = require_field(&body, "name")?;
    let from = body
        .get("from")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let path = body
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let text_path = body
        .get("text_path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    config::create_api_def(&name, from, path, text_path).map_err(|e| e.to_string())?;
    Ok(json!({ "name": name, "output": format!("Created {name:?}.") }))
}

/// `POST /api/definitions/save-raw` — save edited YAML, then validate.
fn api_save_raw(body: Value) -> Result<Value, String> {
    let name = require_field(&body, "name")?;
    let yaml = body
        .get("yaml")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "yaml content is required".to_string())?;
    let errors = config::save_api_raw(&name, yaml).map_err(|e| e.to_string())?;
    Ok(json!({
        "name": name,
        "valid": errors.is_empty(),
        "validation": errors.join("\n"),
    }))
}

/// `POST /api/definitions/delete` — remove a custom definition.
fn api_delete(body: Value) -> Result<Value, String> {
    let name = require_field(&body, "name")?;
    config::delete_api_def(&name).map_err(|e| e.to_string())?;
    Ok(json!({ "name": name, "output": format!("Removed {name:?}.") }))
}

/// `POST /api/definitions/preview` — live-preview a definition (from edited `yaml`
/// or a stored `name`) with sample inputs: validation, flags, presets, and a
/// sample composed body.
fn api_preview(body: Value) -> Result<Value, String> {
    let yaml = body
        .get("yaml")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("hello world");
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("sample-model");
    let role = body.get("role").and_then(Value::as_str).unwrap_or("user");
    config::preview_api_def(name, yaml, prompt, model, role).map_err(|e| e.to_string())
}

/// Extract (and percent-decode) a definition name from a `/api/definitions/...` path.
fn def_name(base: &str, prefix: &str) -> String {
    let rest = base
        .strip_prefix(prefix)
        .unwrap_or("")
        .trim_end_matches('/');
    percent_decode(rest)
}

/// Extract a single query-string parameter from a shimmed request path.
fn query_param(path: &str, key: &str) -> Option<String> {
    let query = path.split('?').nth(1)?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

/// Minimal percent-decoding for query values (`%XX` and `+` → space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Sentinel the webview shim watches for to close the ReadableStream.
pub const STREAM_END: &str = "__LEXI_STREAM_END__";

fn sse(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

/// `POST /api/respond/stream` — streaming chat. Emits the frontend's SSE protocol
/// (`provider_start` / `delta` / `provider_done`, keyed by provider) through the
/// Tauri channel. Providers stream **concurrently** — their events are tagged by
/// provider, so interleaving is fine and the slowest provider no longer blocks
/// the others. A close sentinel is sent once all providers finish.
pub async fn stream(body: Value, on_event: tauri::ipc::Channel<String>) -> Result<(), String> {
    let prompt = body.get("prompt").and_then(Value::as_str).unwrap_or("");
    let providers = body
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let empty = Map::new();
    let models = body
        .get("models")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let jobs = providers.iter().filter_map(Value::as_str).map(|provider| {
        let provider = provider.to_string();
        let model = models
            .get(&provider)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let ch = on_event.clone();
        async move {
            let _ = ch.send(sse("provider_start", json!({ "provider": provider })));

            let cb_ch = ch.clone();
            let p = provider.clone();
            let result = lexi_host::respond_stream_text(
                &provider,
                prompt,
                "user",
                model.as_deref(),
                &[],
                move |frag| {
                    let _ = cb_ch.send(sse("delta", json!({ "provider": p, "text": frag })));
                },
            )
            .await;

            let done = match result {
                Ok(_) => json!({ "provider": provider }),
                Err(e) => json!({ "provider": provider, "error": e.to_string() }),
            };
            let _ = ch.send(sse("provider_done", done));
        }
    });

    futures_util::future::join_all(jobs).await;

    let _ = on_event.send(STREAM_END.to_string());
    Ok(())
}

/// `POST /api/respond` — buffered chat, fanned out over the selected providers.
async fn respond(body: Value) -> Result<Value, String> {
    let prompt = body.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    let providers = body
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if providers.is_empty() {
        return Err("at least one provider must be selected".into());
    }
    let empty = Map::new();
    let models = body
        .get("models")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    // Fan out over the selected providers concurrently; `join_all` preserves the
    // input order, so the rendered responses stay stable.
    let jobs = providers.iter().filter_map(Value::as_str).map(|provider| {
        let model = models
            .get(provider)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        async move {
            match lexi_host::respond_text(provider, prompt, "user", model, &[]).await {
                Ok(r) => json!({ "provider": provider, "output": r.text, "stderr": "" }),
                Err(e) => json!({ "provider": provider, "error": e.to_string() }),
            }
        }
    });
    let responses = futures_util::future::join_all(jobs).await;
    Ok(json!({ "responses": responses }))
}

// --- Images tab ---------------------------------------------------------------
// The same pipeline as `lexi respond --output image` / `lexi generate`: the
// provider's image binding picks the API and default model, the settings are
// that API's schema flags (`x-cli`), and images are saved by
// `lexi_host::generate::save_artifacts` (never overwriting a file).

/// Where generated images go: `images_dir` in `config.json` (`~` expanded),
/// else `~/Pictures/Lexi`.
fn images_dir() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let configured = config::load_config()
        .ok()
        .and_then(|c| {
            c.get("images_dir")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|s| !s.trim().is_empty());
    match configured {
        Some(dir) => match dir.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None if dir == "~" => home,
            None => std::path::PathBuf::from(dir),
        },
        None => home.join("Pictures").join("Lexi"),
    }
}

/// `GET /api/images/providers` — every provider, whether it can make images
/// (an API that produces them), and its default image model; plus the folder
/// images are saved to.
fn images_providers() -> Result<Value, String> {
    let cfg = config::load_providers().map_err(|e| e.to_string())?;
    let names: Vec<String> = cfg
        .get("providers")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let providers: Vec<Value> = names
        .iter()
        .map(
            |name| match lexi_host::resolve_model(name, None, Some("image")) {
                Ok(r) => {
                    json!({ "name": name, "images": true, "default_model": r.model, "api": r.api })
                }
                Err(e) => json!({ "name": name, "images": false, "reason": e.to_string() }),
            },
        )
        .collect();
    Ok(json!({
        "providers": providers,
        "images_dir": images_dir().display().to_string(),
    }))
}

/// `GET /api/images/models?provider=X` — the models that produce images
/// (`out:image`): configured ones, plus those in the cached catalog (usable
/// unconfigured). No network call.
async fn images_models(path: &str) -> Result<Value, String> {
    let provider = require_provider(path)?;
    let has_out_image = |m: &Value| {
        m.get("tags")
            .and_then(Value::as_array)
            .is_some_and(|t| t.iter().any(|t| t == "out:image"))
    };
    let mut models: Vec<Value> = Vec::new();
    for m in lexi_host::models::model_summaries(&provider).map_err(|e| e.to_string())? {
        if has_out_image(&m) {
            models.push(json!({ "id": m["id"], "name": m["name"], "configured": true }));
        }
    }
    // The catalog is optional: no cached listing just means no extra models.
    if let Ok(hosted) =
        lexi_host::models::hosted_summaries(&provider, true, &["out:image".to_string()]).await
    {
        for m in hosted {
            let Some(id) = m.get("id").and_then(Value::as_str) else {
                continue;
            };
            if models.iter().all(|c| c["id"] != id) {
                let name = m.get("display_name").or_else(|| m.get("name")).cloned();
                models.push(json!({ "id": id, "name": name, "configured": false }));
            }
        }
    }
    let default = lexi_host::resolve_model(&provider, None, Some("image"))
        .ok()
        .map(|r| r.model);
    Ok(json!({ "provider": provider, "default_model": default, "models": models }))
}

/// `GET /api/images/options?provider=X[&model=M]` — the settings the image API
/// serving that model accepts (its schema flags), and the model it resolves to.
fn images_options(path: &str) -> Result<Value, String> {
    let provider = require_provider(path)?;
    let model = query_param(path, "model").filter(|m| !m.is_empty());
    let r = lexi_host::resolve_model(&provider, model.as_deref(), Some("image"))
        .map_err(|e| e.to_string())?;
    // The prompt and model are set by the tab itself.
    let options: Vec<Value> = lexi_core::cli_flags(&r.effective)
        .into_iter()
        .filter(|f| !matches!(f["path"].as_str(), Some("prompt" | "model")))
        .collect();
    Ok(json!({
        "provider": provider,
        "model": r.model,
        "api": r.api,
        "options": options,
    }))
}

/// `POST /api/images/generate` — `{provider, model?, prompt, options: {path: value}}`.
/// Generates, saves each image to the images folder, and returns them with
/// their data inline (`data_url`) for the page to show.
async fn images_generate(body: Value) -> Result<Value, String> {
    use base64::prelude::*;
    let provider = require_field(&body, "provider")?;
    let prompt = require_field(&body, "prompt")?;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let set_paths: Vec<(String, Value)> = body
        .get("options")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter(|(_, v)| !v.is_null() && *v != "")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let req = lexi_host::ChatRequest {
        prompt: json!({ "prompt": prompt, "role": "user" }),
        model,
        output: Some("image".into()),
        set_paths,
        ..Default::default()
    };
    let chat = lexi_host::prepare_chat(&provider, &req, lexi_host::ChatMode::Buffered)
        .map_err(|e| e.to_string())?;
    let res = lexi_host::run_chat(&chat, |_| {})
        .await
        .map_err(|e| e.to_string())?;
    let dir = images_dir();
    let saved = lexi_host::generate::save_artifacts(&chat, &res, &dir)
        .await
        .map_err(|e| e.to_string())?;
    if saved.is_empty() {
        let text = res.text.trim();
        return Err(if text.is_empty() {
            "the provider returned no images".to_string()
        } else {
            format!("the provider returned no images: {text}")
        });
    }
    let images: Vec<Value> = saved
        .iter()
        .map(|a| {
            let mut v = a.to_value();
            if let Ok(bytes) = std::fs::read(&a.path) {
                v["data_url"] = Value::from(format!(
                    "data:{};base64,{}",
                    a.media_type,
                    BASE64_STANDARD.encode(bytes)
                ));
            }
            v
        })
        .collect();
    Ok(json!({
        "provider": provider,
        "model": chat.model,
        "images": images,
        "warnings": chat.warnings,
    }))
}

/// `POST /api/images/reveal` — `{path}` shows that file in the file manager;
/// `{}` opens the images folder (creating it first).
fn images_reveal(body: Value) -> Result<Value, String> {
    use std::process::Command;
    let file = body
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let status = match file {
        Some(path) => {
            if !std::path::Path::new(path).exists() {
                return Err(format!("{path} no longer exists"));
            }
            if cfg!(target_os = "macos") {
                Command::new("open").args(["-R", path]).status()
            } else if cfg!(target_os = "windows") {
                Command::new("explorer")
                    .arg(format!("/select,{path}"))
                    .status()
            } else {
                let parent = std::path::Path::new(path)
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| ".".into());
                Command::new("xdg-open").arg(parent).status()
            }
        }
        None => {
            let dir = images_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(target_os = "windows") {
                "explorer"
            } else {
                "xdg-open"
            };
            Command::new(opener).arg(&dir).status()
        }
    };
    status.map_err(|e| e.to_string())?;
    Ok(json!({}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{percent_decode, query_param};

    #[test]
    fn query_param_extracts_and_decodes() {
        assert_eq!(
            query_param("/api/models?provider=openai", "provider").as_deref(),
            Some("openai")
        );
        assert_eq!(
            query_param("/api/models?x=1&provider=a%2Fb", "provider").as_deref(),
            Some("a/b")
        );
        assert_eq!(query_param("/api/models", "provider"), None);
        assert_eq!(query_param("/api/models?other=1", "provider"), None);
    }

    #[test]
    fn themes_are_seeded_and_the_selection_falls_back_to_the_default() {
        use super::themes_in;
        let home = tempfile::tempdir().unwrap();
        // First use writes the shipped file; an unknown theme falls back to the default.
        let t = themes_in(home.path(), Some("dark-green"));
        assert!(home.path().join("themes.json").exists());
        assert_eq!(t["default"], "dark-cyan");
        assert_eq!(t["selected"], "dark-cyan");
        assert!(t["warning"].is_null());
        let ids: Vec<&str> = t["themes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "system",
                "dark-cyan",
                "dark-purple",
                "nvidia-dark",
                "nvidia-light"
            ]
        );
        // A saved choice that exists is kept.
        assert_eq!(
            themes_in(home.path(), Some("nvidia-dark"))["selected"],
            "nvidia-dark"
        );
        // The user's file is what's read: an added theme shows up.
        std::fs::write(
            home.path().join("themes.json"),
            r##"{"default":"mine","offered":["system","dark-cyan","dark-purple","nvidia-dark","nvidia-light"],"themes":[{"id":"mine","name":"Mine","vars":{"--bg":"#000"}}]}"##,
        )
        .unwrap();
        let t = themes_in(home.path(), None);
        assert_eq!(t["selected"], "mine");
        assert_eq!(t["themes"].as_array().unwrap().len(), 1);
        // A file from before a theme shipped gets it added once; a shipped
        // theme the user deleted stays deleted.
        std::fs::write(
            home.path().join("themes.json"),
            r##"{"themes":[{"id":"dark-cyan","name":"Dark Cyan","vars":{"--bg":"#000"}}]}"##,
        )
        .unwrap();
        let t = themes_in(home.path(), None);
        assert!(t["added"].as_array().unwrap().iter().any(|n| n == "System"));
        assert!(!t["added"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "Dark Cyan"));
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("themes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["offered"].as_array().unwrap().len(), 5);
        let mut pruned = saved.clone();
        pruned["themes"]
            .as_array_mut()
            .unwrap()
            .retain(|t| t["id"] != "system");
        std::fs::write(
            home.path().join("themes.json"),
            serde_json::to_string(&pruned).unwrap(),
        )
        .unwrap();
        let t = themes_in(home.path(), None);
        assert!(t["added"].as_array().unwrap().is_empty());
        assert!(!t["themes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "system"));
        // A broken file falls back to the shipped themes, with a warning.
        std::fs::write(home.path().join("themes.json"), "{not json").unwrap();
        let t = themes_in(home.path(), Some("nvidia-light"));
        assert_eq!(t["selected"], "nvidia-light");
        assert!(t["warning"]
            .as_str()
            .unwrap()
            .contains("using the built-in themes"));
    }

    #[test]
    fn percent_decode_handles_escapes_and_plus() {
        assert_eq!(percent_decode("meta%2Fllama+3"), "meta/llama 3");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn def_name_strips_prefix_and_decodes() {
        use super::def_name;
        assert_eq!(
            def_name("/api/definitions/messages", "/api/definitions/"),
            "messages"
        );
        assert_eq!(
            def_name("/api/definitions/raw/chat/", "/api/definitions/raw/"),
            "chat"
        );
        assert_eq!(
            def_name("/api/definitions/a%20b", "/api/definitions/"),
            "a b"
        );
    }

    #[test]
    fn num_field_coerces_numbers_and_numeric_strings() {
        use super::num_field;
        use serde_json::json;
        let body = json!({ "a": 256, "b": "0.7", "c": "nope", "d": true });
        assert_eq!(num_field(&body, "a"), Some(json!(256)));
        assert_eq!(num_field(&body, "b"), Some(json!(0.7)));
        assert_eq!(num_field(&body, "c"), None);
        assert_eq!(num_field(&body, "d"), None);
        assert_eq!(num_field(&body, "missing"), None);
    }

    #[test]
    fn int_field_coerces_to_integers() {
        use super::int_field;
        use serde_json::json;
        let body = json!({ "a": 4096, "b": "128000", "c": 1.9, "d": "x", "e": true });
        assert_eq!(int_field(&body, "a"), Some(json!(4096)));
        assert_eq!(int_field(&body, "b"), Some(json!(128000)));
        assert_eq!(int_field(&body, "c"), Some(json!(1))); // truncates floats
        assert_eq!(int_field(&body, "d"), None);
        assert_eq!(int_field(&body, "e"), None);
        assert_eq!(int_field(&body, "missing"), None);
    }

    /// The Images tab's calls end to end against a mock image API: the
    /// provider is found, its settings listed, and a generation sends the
    /// chosen settings and saves the images to `images_dir`.
    /// Tests that point `$LEXI_HOME` somewhere take turns.
    pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn images_generate_against_a_mock_provider() {
        let _env = ENV_LOCK.lock().await;
        use serde_json::json;
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .and(body_partial_json(
                json!({ "model": "img-model", "size": "1024x1024", "n": 2 }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1,
                "data": [
                    { "b64_json": "iVBORw0KGgo=", "revised_prompt": "a calm fox" },
                    { "b64_json": "iVBORw0KGgo=" }
                ]
            })))
            .mount(&server)
            .await;

        let home = tempfile::tempdir().unwrap();
        let out = home.path().join("pictures");
        std::env::set_var("LEXI_HOME", home.path());
        std::fs::create_dir_all(home.path().join("providers")).unwrap();
        std::fs::write(
            home.path().join("providers/providers-config.yaml"),
            format!(
                "providers:\n  mockimg:\n    api_key: k\n    apis:\n      - api: image-generations\n        url: {}\n        default_model: img-model\n",
                server.uri()
            ),
        )
        .unwrap();
        std::fs::write(
            home.path().join("config.json"),
            serde_json::to_string(&json!({ "images_dir": out.display().to_string() })).unwrap(),
        )
        .unwrap();

        let providers = super::handle("GET", "/api/images/providers", None)
            .await
            .unwrap();
        let mock = providers["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "mockimg")
            .unwrap();
        assert_eq!(mock["images"], true);
        assert_eq!(mock["default_model"], "img-model");
        assert_eq!(providers["images_dir"], out.display().to_string());

        // A provider with only an image API is left out of the Chat tab.
        let all = super::handle("GET", "/api/providers", None).await.unwrap();
        let mock = all["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "mockimg")
            .unwrap();
        assert_eq!(mock["chat"], false);

        let opts = super::handle("GET", "/api/images/options?provider=mockimg", None)
            .await
            .unwrap();
        let paths: Vec<&str> = opts["options"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|o| o["path"].as_str())
            .collect();
        assert!(paths.contains(&"size") && paths.contains(&"n"), "{paths:?}");
        assert!(!paths.contains(&"prompt") && !paths.contains(&"model"));

        let res = super::handle(
            "POST",
            "/api/images/generate",
            Some(json!({
                "provider": "mockimg",
                "prompt": "A calm fox",
                "options": { "size": "1024x1024", "n": 2, "quality": "" }
            })),
        )
        .await
        .unwrap();
        let images = res["images"].as_array().unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0]["revised_prompt"], "a calm fox");
        assert!(images[0]["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert!(out.join("a-calm-fox-1.png").exists() && out.join("a-calm-fox-2.png").exists());
    }

    /// Every button with hover help says what its action is on the command
    /// line: `data-cmd="NAME"` (a key of `CLI_COMMANDS`, whose ">_" shows the
    /// `lexi` command and Python) or `data-cmd="none"`. And every name used
    /// is defined.
    #[test]
    fn frontend_buttons_with_hover_help_name_their_commands() {
        let html = include_str!("../frontend/index.html");
        let line_of = |at: usize| html[..at].lines().count();
        let mut missing = Vec::new();
        let mut used = std::collections::BTreeSet::new();
        let mut at = 0;
        while let Some(i) = html[at..].find("<button") {
            let start = at + i;
            let end = start + html[start..].find('>').expect("an unclosed <button");
            let tag = &html[start..end];
            if tag.contains("data-tip") && !tag.contains("data-cmd=") {
                missing.push(format!(
                    "line {}: {}",
                    line_of(start),
                    tag.chars().take(120).collect::<String>()
                ));
            }
            at = end;
        }
        for (pre, quote) in [("data-cmd=\"", '"'), ("'data-cmd', '", '\'')] {
            for part in html.split(pre).skip(1) {
                let name = &part[..part.find(quote).unwrap()];
                if name != "none" {
                    used.insert(name.to_string());
                }
            }
        }
        let undefined: Vec<&String> = used
            .iter()
            .filter(|n| !html.contains(&format!("\n      '{n}': ")))
            .collect();
        assert!(
            missing.is_empty(),
            "buttons with data-tip but no data-cmd:\n{}",
            missing.join("\n")
        );
        assert!(
            undefined.is_empty(),
            "data-cmd names missing from CLI_COMMANDS: {undefined:?}"
        );
        assert!(used.len() > 30, "found only {} data-cmd names", used.len());
    }

    /// Every local file the page loads (`src="…"`: scripts, images) is in
    /// the frontend folder. (The Markdown libraries were once referenced but
    /// missing, so chat silently showed plain text.)
    #[test]
    fn frontend_local_files_exist() {
        let html = include_str!("../frontend/index.html");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend");
        let mut missing = Vec::new();
        for part in html.split("src=\"").skip(1) {
            let src = &part[..part.find('"').unwrap_or(0)];
            let dynamic = src.contains("${") || src.contains(':') || src.is_empty();
            if !dynamic && !dir.join(src.trim_start_matches('/')).is_file() {
                missing.push(src.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "files the page loads but frontend/ lacks: {missing:?}"
        );
    }

    /// The frontend is one script: a second top-level `function name(` with
    /// the same name silently replaces the first everywhere (a release-notes
    /// `renderMarkdown` once broke chat that way).
    #[test]
    fn frontend_top_level_functions_have_unique_names() {
        let html = include_str!("../frontend/index.html");
        let mut seen = std::collections::HashMap::new();
        let mut dups = Vec::new();
        for (i, line) in html.lines().enumerate() {
            // Top-level script code is indented four spaces.
            let Some(rest) = line.strip_prefix("    ") else {
                continue;
            };
            if rest.starts_with(' ') {
                continue;
            }
            // `function f(`, `async function f(`, `const f =`, `let f =` — a
            // repeated name either replaces the first or (const/let) stops
            // the whole script with a SyntaxError.
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            let Some(decl) = ["function ", "const ", "let "]
                .iter()
                .find_map(|kw| rest.strip_prefix(kw))
            else {
                continue;
            };
            let name: String = decl
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if name.is_empty() {
                continue; // destructuring
            }
            if let Some(first) = seen.insert(name.clone(), i + 1) {
                dups.push(format!("{name} (lines {first} and {})", i + 1));
            }
        }
        assert!(dups.is_empty(), "duplicate top-level names: {dups:?}");
    }

    #[test]
    fn release_notes_have_a_section_for_this_version() {
        let sections = super::release_sections(super::RELEASE_NOTES);
        assert!(
            sections
                .iter()
                .any(|(v, _, body)| v == super::APP_VERSION && !body.is_empty()),
            "CHANGELOG.md needs a `## {}` section with notes",
            super::APP_VERSION
        );
    }

    #[test]
    fn unseen_notes_are_the_releases_since_the_last_one_opened() {
        let text = "# Notes\nintro\n\n## Unreleased\n- next\n\n## 0.3.0 — 2026-12-01\n- three\n\n## 0.2.0\n- two\n\n## 0.1.0\n- one\n";
        let versions = |seen: Option<&str>, cur: &str| -> Vec<String> {
            super::unseen_notes(text, seen, cur)
                .iter()
                .map(|n| n["version"].as_str().unwrap().to_string())
                .collect()
        };
        // First run: unreleased changes and this version's notes only.
        assert_eq!(versions(None, "0.3.0"), ["Unreleased", "0.3.0"]);
        // An upgrade shows every release since the last one opened.
        assert_eq!(
            versions(Some("0.1.0"), "0.3.0"),
            ["Unreleased", "0.3.0", "0.2.0"]
        );
        assert_eq!(versions(Some("0.2.0"), "0.2.0"), ["Unreleased"]);
        let notes = super::unseen_notes(text, None, "0.3.0");
        assert_eq!(notes[1]["title"], "0.3.0 — 2026-12-01");
        assert_eq!(notes[1]["notes"], "- three");
        // An empty Unreleased section is left out.
        let quiet = "## Unreleased\n\n## 0.2.0\n- two\n";
        assert_eq!(super::unseen_notes(quiet, None, "0.2.0").len(), 1);
    }

    /// The startup screen's calls: credentials per provider, saving / testing
    /// / removing a key, and remembering the version once it's been opened.
    #[tokio::test]
    async fn startup_reports_credentials_and_the_wizard_saves_keys() {
        let _env = ENV_LOCK.lock().await;
        use serde_json::json;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer sk-good-key-1234"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "id": "m" }] })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let home = tempfile::tempdir().unwrap();
        std::env::set_var("LEXI_HOME", home.path());
        std::fs::create_dir_all(home.path().join("providers")).unwrap();
        std::fs::write(
            home.path().join("providers/providers-config.yaml"),
            format!(
                "providers:\n  openai:\n    template: openai\n    url: {}\n    api_key: ${{LEXI_TEST_UNSET_KEY_VAR}}\n",
                server.uri()
            ),
        )
        .unwrap();

        let start = super::handle("GET", "/api/startup", None).await.unwrap();
        assert_eq!(start["version"], super::APP_VERSION);
        assert_eq!(start["new_version"], true);
        assert_eq!(start["has_credentials"], false);
        assert!(start["notes"].as_array().is_some_and(|n| !n.is_empty()));
        let openai = &start["providers"][0];
        assert_eq!(openai["display"], "OpenAI");
        assert_eq!(openai["env"], "LEXI_TEST_UNSET_KEY_VAR");
        assert_eq!(openai["env_set"], false);
        assert!(openai["source"].is_null());
        assert!(openai["setup"]["keys_url"]
            .as_str()
            .unwrap()
            .starts_with("https://"));
        // ...and the provider list doesn't count the unset variable as a key.
        let list = super::handle("GET", "/api/providers", None).await.unwrap();
        assert_eq!(list["providers"][0]["api_key"], "[missing]");
        // The Providers tab's Show button reads the config file in-process.
        let src = super::handle("GET", "/api/providers/config-source", None)
            .await
            .unwrap();
        assert!(src["text"]
            .as_str()
            .unwrap()
            .contains("LEXI_TEST_UNSET_KEY_VAR"));
        assert!(src["path"]
            .as_str()
            .unwrap()
            .ends_with("providers-config.yaml"));

        // A wrong key is refused by the provider; the right one is verified.
        let key = |action: &str, value: &str| json!({ "action": action, "provider": "openai", "value": value, "reveal": true });
        let saved = super::handle("POST", "/api/providers/api-key", Some(key("set", "sk-bad")))
            .await
            .unwrap();
        assert_eq!(saved["source"], "saved");
        // The saved key comes after the (unset) variable; the config isn't
        // changed.
        assert_eq!(saved["added"], false);
        assert_eq!(saved["location"], "providers/openai/api-key");
        assert_eq!(saved["store"]["kind"], "file");
        let states: Vec<&str> = saved["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["state"].as_str().unwrap())
            .collect();
        assert_eq!(states, ["missing", "used"]);
        let check = super::handle(
            "POST",
            "/api/providers/check",
            Some(json!({ "provider": "openai" })),
        )
        .await
        .unwrap();
        assert_eq!(check["status"], "invalid api-key");
        super::handle(
            "POST",
            "/api/providers/api-key",
            Some(key("set", "sk-good-key-1234")),
        )
        .await
        .unwrap();
        let check = super::handle(
            "POST",
            "/api/providers/check",
            Some(json!({ "provider": "openai" })),
        )
        .await
        .unwrap();
        assert_eq!(
            (check["status"].as_str(), check["verified"].as_bool()),
            (Some("ready"), Some(true))
        );
        let shown = super::handle("POST", "/api/providers/api-key", Some(key("show", "")))
            .await
            .unwrap();
        assert_eq!(shown["key"], "sk-good-key-1234");

        // Opened once: no notes next time, and a key is set.
        super::handle("POST", "/api/startup/seen", None)
            .await
            .unwrap();
        let again = super::handle("GET", "/api/startup", None).await.unwrap();
        assert_eq!(again["new_version"], false);
        assert_eq!(again["notes"], json!([]));
        assert_eq!(again["has_credentials"], true);
        // ...but the header logo's About view still has this version's notes.
        let about = super::handle("GET", "/api/startup?about=1", None)
            .await
            .unwrap();
        assert!(about["notes"].as_array().is_some_and(|n| !n.is_empty()));

        let removed = super::handle("POST", "/api/providers/api-key", Some(key("remove", "")))
            .await
            .unwrap();
        assert_eq!(removed["removed"], true);
        assert!(removed["source"].is_null());
        assert!(super::handle(
            "POST",
            "/api/open-url",
            Some(json!({ "url": "file:///etc/passwd" }))
        )
        .await
        .is_err());
    }
}
