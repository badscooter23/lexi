//! Admin operations in their machine-interface shapes
//! (`docs/spec/cli-machine-v1.md` §3), shared by the CLI's `--json` output and
//! the Python library. The file-level reads and writes live in [`crate::config`];
//! this module composes them into the summaries every surface returns.

use serde_json::{json, Map, Value};

use crate::config::{self, ACTIVE_PROMPT};
use crate::keystore::Source;
use crate::HostError;

/// App settings every install has, overlaid by `config.json`.
pub const SETTING_DEFAULTS: &[(&str, &str)] = &[
    ("cli_name", "lexi"),
    ("prompt", "cmd"),
    ("prompt_delimiter", ">"),
    ("edit_mode", "vi"),
];

/// The app settings: [`SETTING_DEFAULTS`] overlaid with the saved `config.json`.
pub fn settings() -> Result<Map<String, Value>, HostError> {
    let mut merged: Map<String, Value> = SETTING_DEFAULTS
        .iter()
        .map(|(k, v)| (k.to_string(), Value::from(*v)))
        .collect();
    merged.insert(
        "request_timeout".into(),
        Value::from(config::DEFAULT_REQUEST_TIMEOUT),
    );
    if let Value::Object(saved) = config::load_config()? {
        merged.extend(saved);
    }
    merged.shift_remove("version"); // legacy key; the version comes from the binary
    Ok(merged)
}

/// A provider's entry in `providers-config.yaml`, or `UnknownProvider`.
pub fn provider_config(name: &str) -> Result<Value, HostError> {
    config::load_providers()?
        .get("providers")
        .and_then(|p| p.get(name))
        .cloned()
        .ok_or_else(|| HostError::UnknownProvider(name.to_string()))
}

/// Every provider: `{name, template, url, api, apis, default_model,
/// hosted_models, hosted_model_fields, key_source}` (never the key itself).
/// `api` is the catch-all binding's API; `apis` lists every binding's.
pub fn provider_summaries() -> Result<Vec<Value>, HostError> {
    let cfg = config::load_providers()?;
    let Some(providers) = cfg.get("providers").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    providers
        .iter()
        .map(|(name, pcfg)| {
            let bindings = lexi_core::provider_bindings(pcfg);
            let catch_all = bindings
                .iter()
                .find(|b| b.get("for").is_none())
                .or(bindings.first());
            Ok(json!({
                "name": name,
                "template": pcfg.get("template"),
                "url": pcfg.get("url"),
                "api": catch_all.and_then(|b| b.get("api")),
                "apis": bindings.iter().filter_map(|b| b.get("api")).collect::<Vec<_>>(),
                "default_model": catch_all
                    .and_then(|b| b.get("default_model"))
                    .or(pcfg.get("default_model")),
                "hosted_models": crate::hosted::catalog_cfg(pcfg).get("path").cloned(),
                "hosted_model_fields": crate::hosted::catalog_cfg(pcfg).get("fields").cloned(),
                // A key that can't be read (an unreadable file) shows as none.
                "key_source": config::api_key_source(name, pcfg).ok().flatten(),
            }))
        })
        .collect()
}

/// Add a provider (`providers add`): checks the API definition exists, writes
/// the entry, saves `api_key` (in the provider's owner-only key file — never
/// the config, which gets the source), and makes
/// `default_model` its default.
pub fn create_provider(
    name: &str,
    url: &str,
    default_model: &str,
    api_key: Option<&str>,
    api: &str,
    hosted_models: &str,
) -> Result<(), HostError> {
    config::load_api_def(api)?;
    let mut entry = Map::new();
    entry.insert("url".into(), Value::from(url));
    entry.insert("hosted_models".into(), Value::from(hosted_models));
    entry.insert("api".into(), Value::from(api));
    entry.insert("default_model".into(), Value::from(default_model));
    config::add_provider(name, &entry)?;
    if let Some(key) = api_key {
        config::set_api_key(name, None, key)?;
    }
    config::set_kind_default(name, "chat", Some(default_model))
}

fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() > 8 {
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}{}{tail}", "*".repeat(chars.len() - 8))
    } else {
        "*".repeat(chars.len())
    }
}

/// A provider's (or, with `api`, one API's) API key: `{provider, api, own,
/// source, from, key, error, sources, save_to, store, env}`.
/// - `sources`: where it's looked for, in order — its `api_key` list with the
///   saved key in its place — each `{kind, from, state, note?}` (`kind`:
///   `env`, `file`, `command`, `saved`, `config`; `state`: `used`,
///   `missing`, `error`, `not run`, `not checked`); `own` is false when the
///   API has no list of its own and uses the provider's.
/// - `source`/`from`: the kind and place of the key that's used, and `key` it
///   (masked unless `reveal`) — all null when there's none; `error` is a read
///   error that stopped the lookup. Commands run only with `reveal`: else a
///   command is `not run`, and is `source` when it would be used.
/// - `save_to`: the file `providers api-key set` saves the key in; `store`:
///   `{kind, available, locked}` — where saved keys are kept (`file`).
/// - `env`: the first environment variable the list reads, if any.
pub fn api_key_info(name: &str, api: Option<&str>, reveal: bool) -> Result<Value, HostError> {
    provider_config(name)?;
    let plan = config::key_plan(name, api, reveal)?;
    let lookup = &plan.lookup;
    let used = lookup.source();
    let env = plan.sources.iter().find_map(|s| match s {
        Source::Env(v) => Some(v.clone()),
        _ => None,
    });
    Ok(json!({
        "provider": name,
        "api": api,
        "own": plan.own,
        "source": used.map(Source::kind),
        "from": used.map(Source::describe),
        "key": lookup.key.as_deref().map(|k| if reveal { k.to_string() } else { mask(k) }),
        "error": lookup.failure().map(|(s, e)| format!("couldn't read {}: {e}", s.describe())),
        "sources": lookup.to_value(),
        "save_to": plan.save_to,
        "store": { "kind": "file", "available": true, "locked": false },
        "env": env,
    }))
}

/// A new prompt's fields before any are set.
pub fn prompt_defaults() -> Value {
    json!({ "prompt": "", "max_tokens": 256, "temperature": 0.7, "role": "user" })
}

/// A saved prompt as a summary: `{name, active, …its fields}`.
pub fn prompt_value(name: &str, body: &Value) -> Value {
    let mut obj = Map::new();
    obj.insert("name".into(), Value::from(name));
    obj.insert("active".into(), Value::from(name == ACTIVE_PROMPT));
    if let Value::Object(fields) = body {
        for (k, v) in fields {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Value::Object(obj)
}

/// Every saved prompt as a summary — the active prompt (`$$`) first, then by name.
pub fn prompt_summaries() -> Result<Vec<Value>, HostError> {
    let mut prompts = config::load_prompts()?;
    prompts.sort_by_key(|(n, _)| (n != ACTIVE_PROMPT, n.clone()));
    Ok(prompts.iter().map(|(n, b)| prompt_value(n, b)).collect())
}

/// Create or update a saved prompt: [`prompt_defaults`], overlaid by the
/// existing prompt, overlaid by `fields`. The result must have prompt text.
/// Returns the saved prompt's summary.
pub fn update_prompt(name: &str, fields: &Map<String, Value>) -> Result<Value, HostError> {
    let mut body = prompt_defaults();
    let existing = config::load_prompts()?.into_iter().find(|(n, _)| n == name);
    for (k, v) in existing
        .and_then(|(_, b)| b.as_object().cloned())
        .into_iter()
        .flatten()
        .chain(fields.clone())
    {
        body[k.as_str()] = v;
    }
    if body["prompt"].as_str().is_none_or(|t| t.trim().is_empty()) {
        return Err(HostError::Prompt("a prompt needs prompt text".into()));
    }
    config::save_prompt(name, &body)?;
    Ok(prompt_value(name, &body))
}

/// A definition's response text path, as `api list` shows it.
fn text_path(def: &Value) -> String {
    let te = def.pointer("/response/text_extract");
    if let Some(p) = te.and_then(|t| t.get("text_path")).and_then(Value::as_str) {
        return p.to_string();
    }
    match te.and_then(|t| t.get("items_path")).and_then(Value::as_str) {
        Some(items) => format!("items:{items}"),
        None => String::new(),
    }
}

/// Every API definition: `{name, description, path, text_path, builtin}`.
pub fn api_summaries() -> Result<Vec<Value>, HostError> {
    let shipped = config::embedded_api_names();
    config::list_api_defs()?
        .into_iter()
        .map(|(name, desc)| {
            let def = config::load_api_def(&name)?;
            Ok(json!({
                "name": name,
                "description": desc,
                "path": def.pointer("/request/path"),
                "text_path": text_path(&def),
                "builtin": shipped.contains(&name.as_str()),
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keys() {
        assert_eq!(mask("sk-abcdefghij"), "sk-a*****ghij");
        assert_eq!(mask("short"), "*****");
    }

    #[test]
    fn prompt_values_mark_the_active_prompt() {
        let v = prompt_value("$$", &json!({"prompt": "hi", "name": "ignored"}));
        assert_eq!(v["active"], true);
        assert_eq!(v["name"], "$$");
        assert_eq!(v["prompt"], "hi");
    }
}
