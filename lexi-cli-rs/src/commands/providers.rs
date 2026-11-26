//! `providers` — the providers in `providers/providers-config.yaml`.

use serde_json::{json, Value};

use crate::cli::{ApiKeyCmd, KeySourceArg, KeySourceCmd, ProviderApiCmd, ProvidersCmd};
use crate::output::{scalar, table, Io};

pub(crate) async fn run(io: &mut Io<'_>, cmd: ProvidersCmd) -> u8 {
    match cmd {
        ProvidersCmd::List { raw, table } => list(io, raw, table),
        ProvidersCmd::Status { name, raw } => status(io, &name, raw).await,
        ProvidersCmd::Add {
            name,
            url,
            default_model,
            api_key,
            api,
            hosted_models,
        } => add(
            io,
            &name,
            &url,
            &default_model,
            api_key.as_deref(),
            &api,
            &hosted_models,
        ),
        ProvidersCmd::ApiKey { cmd } => match cmd {
            ApiKeyCmd::Set { name, value, api } => key_set(io, &name, value, api.as_deref()),
            ApiKeyCmd::Show { name, reveal, api } => key_show(io, &name, reveal, api.as_deref()),
            ApiKeyCmd::Remove { name, api } => key_remove(io, &name, api.as_deref()),
            ApiKeyCmd::Source { cmd } => key_sources(io, cmd),
        },
        ProvidersCmd::Api { cmd } => api(io, cmd),
        ProvidersCmd::Upgrade {
            names,
            template,
            yes,
        } => upgrade(io, &names, template.as_deref(), yes),
        ProvidersCmd::Rename { old, new } => match lexi_host::config::rename_provider(&old, &new) {
            Ok(()) => io.result(
                format!("Renamed provider {old} -> {new}."),
                json!({ "renamed": old, "to": new }),
            ),
            Err(e) => io.fail(e),
        },
        ProvidersCmd::Rm { name } => match lexi_host::config::remove_provider(&name) {
            Ok(()) => io.result(
                format!("Removed provider {name}."),
                json!({ "removed": name }),
            ),
            Err(e) => io.fail(e),
        },
    }
}

/// The provider's config entry, or fail with "unknown provider".
fn provider_cfg(io: &mut Io<'_>, name: &str) -> Result<Value, u8> {
    lexi_host::admin::provider_config(name).map_err(|e| io.fail(e))
}

async fn status(io: &mut Io<'_>, name: &str, raw: bool) -> u8 {
    match lexi_host::check_api_key(name).await {
        Ok(st) => {
            let json = json!({ "provider": name, "status": st.status, "source": st.source, "from": st.from });
            let human = if raw {
                json.to_string()
            } else {
                let from = st
                    .from
                    .map(|s| format!(" (key from {s})"))
                    .unwrap_or_default();
                format!("{name}: {}{from}", st.status)
            };
            io.result(human, json)
        }
        Err(e) => io.fail(e),
    }
}

fn add(
    io: &mut Io<'_>,
    name: &str,
    url: &str,
    default_model: &str,
    api_key: Option<&str>,
    api: &str,
    hosted_models: &str,
) -> u8 {
    if let Err(e) =
        lexi_host::admin::create_provider(name, url, default_model, api_key, api, hosted_models)
    {
        return io.fail(e);
    }
    io.result(format!("Added provider {name}."), json!({ "added": name }))
}

/// What a saved key means for messages: where it went, whether the config
/// changed, and a warning when an earlier source wins.
fn saved_message(lead: &str, saved: &lexi_host::config::SavedKey) -> String {
    let mut msg = format!("{lead} {}.", saved.location);
    if saved.added {
        msg.push_str(" Updated its api_key in providers-config.yaml.");
    }
    match &saved.used {
        Some(used) if *used != saved.location => msg.push_str(&format!(
            "\nNote: {used} comes first and has a key, so that key is used."
        )),
        _ => {}
    }
    msg
}

fn key_set(io: &mut Io<'_>, name: &str, value: Option<String>, api: Option<&str>) -> u8 {
    if let Err(code) = provider_cfg(io, name) {
        return code;
    }
    let key = match value {
        Some(v) => v,
        None => match io.read_secret("Enter API key: ") {
            Ok(k) => k,
            Err(e) => return io.fail(e),
        },
    };
    match lexi_host::config::set_api_key(name, api, &key) {
        Ok(saved) => {
            let mut json = saved.to_value();
            json["provider"] = json!(name);
            io.result(
                saved_message(&format!("Saved the API key for {name} in"), &saved),
                json,
            )
        }
        Err(e) => io.fail(e),
    }
}

/// Each `api_key` source in order with what was found there, then the key
/// (`sk-a…wxyz`-style masking unless --reveal).
fn key_show(io: &mut Io<'_>, name: &str, reveal: bool, api: Option<&str>) -> u8 {
    let info = match lexi_host::admin::api_key_info(name, api, reveal) {
        Ok(info) => info,
        Err(e) => return io.fail(e),
    };
    let mut lines = Vec::new();
    let whose = match (api, info["own"].as_bool()) {
        (Some(a), Some(false)) => format!("The {a} API uses {name}'s key. "),
        _ => String::new(),
    };
    let sources = info["sources"].as_array().cloned().unwrap_or_default();
    if sources.is_empty() {
        lines.push(format!("{whose}{name}'s api_key names no source."));
    } else {
        lines.push(format!("{whose}Looked for in this order:"));
        for (i, s) in sources.iter().enumerate() {
            let note = s["note"]
                .as_str()
                .map(|n| format!(": {n}"))
                .unwrap_or_default();
            let saved = if s["kind"] == "saved" {
                " (saved key)"
            } else {
                ""
            };
            lines.push(format!(
                "  {}. {}{saved} — {}{note}",
                i + 1,
                scalar(s.get("from")),
                scalar(s.get("state"))
            ));
        }
    }
    match (info["key"].as_str(), info["error"].as_str()) {
        (_, Some(err)) => lines.push(format!("No API key: {err}")),
        (Some(key), None) => lines.push(format!("Key: {key}")),
        (None, None) => lines.push(format!("No API key configured for {name}.")),
    }
    io.result(lines.join("\n"), info)
}

fn key_remove(io: &mut Io<'_>, name: &str, api: Option<&str>) -> u8 {
    if let Err(code) = provider_cfg(io, name) {
        return code;
    }
    match lexi_host::config::remove_api_key(name, api) {
        Ok(true) => io.result(
            format!("Removed the saved API key for {name}."),
            json!({ "removed": true }),
        ),
        Ok(false) => io.result(
            format!("No saved API key for {name}."),
            json!({ "removed": false }),
        ),
        Err(e) => io.fail(e),
    }
}

/// A `--env`/`--file`/`--command`/`--saved` choice as an `api_key` entry.
fn source_value(arg: &KeySourceArg) -> Result<Value, String> {
    if let Some(v) = &arg.env {
        return Ok(json!({ "env": v }));
    }
    if let Some(p) = &arg.file {
        return Ok(json!({ "file": p }));
    }
    if let Some(c) = &arg.command {
        return match shlex::split(c) {
            Some(argv) if !argv.is_empty() => Ok(json!({ "command": argv })),
            _ => Err(format!("can't read the command {c:?} (check its quotes)")),
        };
    }
    Ok(json!(lexi_host::config::SAVED_SOURCE))
}

/// The sources as numbered lines.
fn sources_text(list: &Value) -> String {
    let rows = list["sources"].as_array().cloned().unwrap_or_default();
    let mut lines = vec!["Looked for in this order:".to_string()];
    for (i, r) in rows.iter().enumerate() {
        let what = match r["kind"].as_str() {
            Some("saved") if r["implicit"] == true => " (saved key; last unless api_key places it)",
            Some("saved") => " (saved key)",
            _ => "",
        };
        lines.push(format!("  {}. {}{what}", i + 1, scalar(r.get("from"))));
    }
    lines.join("\n")
}

fn key_sources(io: &mut Io<'_>, cmd: KeySourceCmd) -> u8 {
    match cmd {
        KeySourceCmd::List { name, api } => {
            match lexi_host::config::key_source_list(&name, api.as_deref()) {
                Ok(list) => io.result(sources_text(&list), list),
                Err(e) => io.fail(e),
            }
        }
        KeySourceCmd::Add {
            name,
            source,
            first,
            api,
        } => {
            let value = match source_value(&source) {
                Ok(v) => v,
                Err(e) => return io.fail(e),
            };
            match lexi_host::config::add_key_source(&name, api.as_deref(), &value, first) {
                Ok(list) => io.result(
                    format!("Updated {name}'s api_key.\n{}", sources_text(&list)),
                    list,
                ),
                Err(e) => io.fail(e),
            }
        }
        KeySourceCmd::Rm { name, source, api } => {
            let value = match source_value(&source) {
                Ok(v) => v,
                Err(e) => return io.fail(e),
            };
            match lexi_host::config::remove_key_source(&name, api.as_deref(), &value) {
                Ok(removed) => {
                    let msg = if removed {
                        format!("Removed it from {name}'s api_key.")
                    } else {
                        format!("{name}'s api_key doesn't list it.")
                    };
                    io.result(msg, json!({ "provider": name, "removed": removed }))
                }
                Err(e) => io.fail(e),
            }
        }
    }
}

fn list(io: &mut Io<'_>, raw: bool, as_table: bool) -> u8 {
    let rows = match lexi_host::admin::provider_summaries() {
        Ok(r) => r,
        Err(e) => return io.fail(e),
    };
    let json = Value::Array(rows.clone());

    let human = if raw {
        super::read_home_file("providers/providers-config.yaml").unwrap_or_default()
    } else if rows.is_empty() {
        "No providers configured.".to_string()
    } else if as_table {
        let body = rows
            .iter()
            .map(|r| {
                let status = if r["key_source"].is_null() {
                    "no api-key"
                } else {
                    "ready"
                };
                vec![
                    scalar(r.get("name")),
                    scalar(r.get("url")),
                    scalar(r.get("api")),
                    scalar(r.get("key_source")),
                    status.to_string(),
                ]
                .into_iter()
                .map(|c| if c.is_empty() { "—".to_string() } else { c })
                .collect()
            })
            .collect();
        table(
            "Providers",
            &["Provider", "URL", "API", "Key Source", "Status"],
            body,
        )
    } else {
        rows.iter()
            .map(|r| {
                let fields = r["hosted_model_fields"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|v| scalar(Some(v)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let key = match r["key_source"].as_str() {
                    Some(s) => format!("[{s}]"),
                    None => "[missing]".to_string(),
                };
                format!(
                    "{}: url={} api={} hosted={} fields=[{fields}] api_key={key}",
                    scalar(r.get("name")),
                    scalar(r.get("url")),
                    scalar(r.get("api")),
                    scalar(r.get("hosted_models")),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    io.result(human, json)
}

fn api(io: &mut Io<'_>, cmd: ProviderApiCmd) -> u8 {
    match cmd {
        ProviderApiCmd::List { name } => match lexi_host::config::provider_apis(&name) {
            Ok(list) => {
                let mut lines = vec![format!("APIs for {name}:")];
                for b in &list {
                    let serves = match b.get("for").and_then(Value::as_array) {
                        Some(tags) if !tags.is_empty() => {
                            let t: Vec<String> = tags.iter().map(|t| scalar(Some(t))).collect();
                            format!("models tagged {}", t.join(" "))
                        }
                        _ => "everything else (catch-all)".into(),
                    };
                    let mut line = format!("  {} — {serves}", scalar(b.get("api")));
                    if let Some(m) = b.get("default_model").filter(|v| !v.is_null()) {
                        line.push_str(&format!(", default {}", scalar(Some(m))));
                    }
                    if let Some(u) = b.get("url").filter(|v| !v.is_null()) {
                        line.push_str(&format!(", at {}", scalar(Some(u))));
                    }
                    line.push_str(&format!(" [{}]", scalar(b.get("source"))));
                    if b["enabled"] == false {
                        line.push_str(" (off)");
                    }
                    lines.push(line);
                }
                io.result(lines.join("\n"), Value::Array(list))
            }
            Err(e) => io.fail(e),
        },
        ProviderApiCmd::Add {
            name,
            api,
            for_tags,
            default_model,
            url,
        } => {
            let mut b = serde_json::Map::new();
            b.insert("api".into(), Value::from(api.as_str()));
            if !for_tags.is_empty() {
                b.insert("for".into(), json!(for_tags));
            }
            if let Some(m) = default_model {
                b.insert("default_model".into(), Value::from(m));
            }
            if let Some(u) = url {
                b.insert("url".into(), Value::from(u));
            }
            match lexi_host::config::add_provider_api(&name, &b) {
                Ok(existed) => {
                    let verb = if existed { "Updated" } else { "Added" };
                    io.result(
                        format!("{verb} the {api} API on {name}."),
                        json!({ "provider": name, "api": api, "existed": existed }),
                    )
                }
                Err(e) => io.fail(e),
            }
        }
        ProviderApiCmd::Rm { name, api } => {
            match lexi_host::config::remove_provider_api(&name, &api) {
                Ok(()) => io.result(
                    format!("Removed the {api} API from {name}."),
                    json!({ "provider": name, "removed": api }),
                ),
                Err(e) => io.fail(e),
            }
        }
    }
}

fn upgrade(io: &mut Io<'_>, names: &[String], template: Option<&str>, yes: bool) -> u8 {
    let names: Vec<String> = if names.is_empty() {
        match lexi_host::config::load_providers() {
            Ok(cfg) => lexi_host::config::provider_names(&cfg)
                .into_iter()
                .filter(|n| lexi_host::config::template_names().contains(&n.as_str()))
                .collect(),
            Err(e) => return io.fail(e),
        }
    } else {
        names.to_vec()
    };
    let mut results = Vec::new();
    let mut lines = Vec::new();
    for n in &names {
        match lexi_host::config::upgrade_provider(n, template, yes) {
            Ok(r) => {
                if r["changed"] == false {
                    lines.push(format!(
                        "{n}: already uses template {}",
                        scalar(r.get("template"))
                    ));
                } else {
                    let yaml = |v: &Value| {
                        serde_yaml::to_string(v)
                            .unwrap_or_default()
                            .lines()
                            .map(|l| format!("    {l}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    lines.push(format!(
                        "{n}: → template {}\n  before:\n{}\n  after:\n{}",
                        scalar(r.get("template")),
                        yaml(&r["before"]),
                        yaml(&r["after"])
                    ));
                    if let Some(b) = r.get("backup").and_then(Value::as_str) {
                        lines.push(format!("  upgraded (backup: {b})"));
                    }
                }
                results.push(r);
            }
            Err(e) => return io.fail(e),
        }
    }
    if names.is_empty() {
        lines.push("No providers with a shipped template.".into());
    } else if !yes && results.iter().any(|r| r["changed"] == true) {
        lines.push("Run again with --yes to apply.".into());
    }
    io.result(lines.join("\n"), Value::Array(results))
}
