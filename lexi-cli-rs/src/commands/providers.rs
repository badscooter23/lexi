//! `providers` — the providers in `providers/providers-config.yaml`.

use serde_json::{json, Value};

use crate::cli::{ApiKeyCmd, KeySourceArg, KeySourceCmd, KeysCmd, ProviderApiCmd, ProvidersCmd};
use crate::output::{scalar, table, Io};

pub(crate) async fn run(io: &mut Io<'_>, cmd: ProvidersCmd) -> u8 {
    match cmd {
        ProvidersCmd::List { raw, table } => list(io, raw, table),
        ProvidersCmd::Status { name, raw, key } => {
            Box::pin(status(io, &name, raw, key.as_deref())).await
        }
        ProvidersCmd::Add {
            name,
            url,
            default_model,
            api_key,
            key_name,
            api,
            hosted_models,
        } => add(
            io,
            &name,
            &url,
            &default_model,
            key_name.as_deref().zip(api_key.as_deref()),
            &api,
            &hosted_models,
        ),
        ProvidersCmd::ApiKey { cmd } => match cmd {
            ApiKeyCmd::Set { name, value, key } => match key_target(io, &name, key.as_deref()) {
                Ok((p, target)) => key_set(io, &p, &target, value),
                Err(code) => code,
            },
            ApiKeyCmd::Show { name, reveal, key } => match key_target(io, &name, key.as_deref()) {
                Ok((_, target)) => key_show(io, &target, reveal),
                Err(code) => code,
            },
            ApiKeyCmd::Remove { name, key } => match key_target(io, &name, key.as_deref()) {
                Ok((p, target)) => key_remove(io, &p, &target),
                Err(code) => code,
            },
            ApiKeyCmd::Source { cmd } => key_sources(io, cmd),
        },
        ProvidersCmd::Keys { cmd } => keys(io, cmd),
        ProvidersCmd::Api { cmd } => api(io, cmd),
        ProvidersCmd::Upgrade {
            names,
            template,
            yes,
        } => {
            for n in &names {
                if let Err(code) = super::plain_provider(io, n) {
                    return code;
                }
            }
            upgrade(io, &names, template.as_deref(), yes)
        }
        ProvidersCmd::Rename { old, .. } if old.contains('#') => {
            super::plain_provider(io, &old).err().unwrap_or(1)
        }
        ProvidersCmd::Rm { name } if name.contains('#') => {
            super::plain_provider(io, &name).err().unwrap_or(1)
        }
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

/// A key command's provider (`openai`, `openai#work`, `openai#`) and
/// `--name`: the provider, and the target the key functions take
/// (`openai#work`, or `openai` when no key is named).
fn key_target(io: &mut Io<'_>, name: &str, key: Option<&str>) -> Result<(String, String), u8> {
    let (p, _, label) = super::request_key(io, name, key)?;
    Ok((p, label))
}

/// A `providers keys` command's provider and key: `openai#work`, or
/// `openai work` (`openai#` / `openai ''` the unnamed key); `None` when
/// neither names one.
fn keys_target(
    io: &mut Io<'_>,
    name: &str,
    key: Option<&str>,
) -> Result<(String, Option<String>), u8> {
    let (p, inline) = lexi_host::config::split_key_arg(name, None).map_err(|e| io.fail(e))?;
    match (inline, key) {
        (Some(a), Some(b)) => Err(io.fail(format!(
            "{name} already names the key {}: leave out {b:?}",
            lexi_host::config::key_label(&a)
        ))),
        (Some(a), None) => Ok((p, Some(a))),
        (None, Some(b)) => lexi_host::config::split_key_arg(&p, Some(b)).map_err(|e| io.fail(e)),
        (None, None) => Ok((p, None)),
    }
}

/// `openai#work`, `openai#` — a key as written after its provider.
fn written(provider: &str, key: &str) -> String {
    format!("{provider}{}", lexi_host::config::key_label(key))
}

/// The provider's keys and what to do with them (`providers keys …`).
fn keys(io: &mut Io<'_>, cmd: KeysCmd) -> u8 {
    use lexi_host::config as c;
    match cmd {
        KeysCmd::List { name } => {
            if let Err(code) = super::plain_provider(io, &name) {
                return code;
            }
            if let Err(code) = provider_cfg(io, &name) {
                return code;
            }
            match c::key_names(&name) {
                Ok(v) => io.result(keys_text(&v), v),
                Err(e) => io.fail(e),
            }
        }
        KeysCmd::Add { name, key } => {
            let (p, key) = match keys_target(io, &name, key.as_deref()) {
                Ok((p, Some(k))) => (p, k),
                Ok((p, None)) => {
                    return io.fail(format!("name the key: lexi providers keys add {p}#NAME"))
                }
                Err(code) => return code,
            };
            match c::add_named_key(&p, &key) {
                Ok(true) => io.result(
                    format!(
                        "Added the key {}. Save its value with: lexi providers api-key set {}",
                        written(&p, &key),
                        written(&p, &key)
                    ),
                    json!({ "provider": p, "key_name": key, "added": true }),
                ),
                Ok(false) => io.result(
                    format!("{p} already has the key {}.", written(&p, &key)),
                    json!({ "provider": p, "key_name": key, "added": false }),
                ),
                Err(e) => io.fail(e),
            }
        }
        KeysCmd::Rename { name, old, new } => {
            // `rename openai#old new`, or `rename openai old new`.
            let (p, old, new) = match (name.contains('#'), new) {
                (true, None) => match keys_target(io, &name, None) {
                    Ok((p, Some(k))) => (p, k, old),
                    Ok((p, None)) => (p, String::new(), old),
                    Err(code) => return code,
                },
                (false, Some(new)) => match keys_target(io, &name, Some(&old)) {
                    Ok((p, k)) => (p, k.unwrap_or_default(), new),
                    Err(code) => return code,
                },
                (true, Some(extra)) => {
                    return io.fail(format!(
                        "{name} already names the key: write `lexi providers keys rename {name} {old}` (not {extra:?} too)"
                    ))
                }
                (false, None) => {
                    return io.fail(format!(
                        "give the key and its new name: lexi providers keys rename {name}#{old} NEW"
                    ))
                }
            };
            match c::rename_named_key(&p, &old, &new) {
                Ok(()) => io.result(
                    format!("Renamed {} -> {}.", written(&p, &old), written(&p, &new)),
                    json!({ "provider": p, "renamed": old, "to": new }),
                ),
                Err(e) => io.fail(e),
            }
        }
        KeysCmd::Rm { name, key } => {
            let (p, key) = match keys_target(io, &name, key.as_deref()) {
                Ok((p, Some(k))) => (p, k),
                Ok((p, None)) => {
                    return io.fail(format!("name the key: lexi providers keys rm {p}#NAME"))
                }
                Err(code) => return code,
            };
            match c::remove_named_key(&p, &key) {
                Ok(()) => {
                    let after = c::key_names(&p).ok();
                    let note = match after.as_ref().and_then(|v| v["reason"].as_str()) {
                        Some("no key") => format!(" {p} has no key now."),
                        Some("no default key") => format!(
                            " {p} has no default key now: requests must name one until you choose one with `lexi providers keys default {p} NAME`."
                        ),
                        _ => String::new(),
                    };
                    io.result(
                        format!(
                            "Removed the key {} and its saved key.{note}",
                            written(&p, &key)
                        ),
                        json!({ "provider": p, "removed": key }),
                    )
                }
                Err(e) => io.fail(e),
            }
        }
        KeysCmd::Default { name, key } => {
            let (p, key) = match keys_target(io, &name, key.as_deref()) {
                Ok(v) => v,
                Err(code) => return code,
            };
            match key {
                None => match c::key_names(&p) {
                    Ok(v) => {
                        let mut msg = match v["default_key"].as_str() {
                            Some(d) => format!(
                                "{p}'s default key is {}: requests that name no key use it.",
                                written(&p, d)
                            ),
                            None => {
                                format!("{p} has no default key ({}).", scalar(v.get("reason")))
                            }
                        };
                        if v["variable"]["set"] == true {
                            msg.push_str(&format!(
                                " But ${} is set, so they use that instead.",
                                scalar(v["variable"].get("name"))
                            ));
                        }
                        io.result(
                            msg,
                            json!({ "provider": p, "default_key": v["default_key"] }),
                        )
                    }
                    Err(e) => io.fail(e),
                },
                Some(key) => match c::set_default_key(&p, &key) {
                    Ok(()) => io.result(
                        format!(
                            "{p}'s requests now use the key {} unless they name another.",
                            written(&p, &key)
                        ),
                        json!({ "provider": p, "default_key": key }),
                    ),
                    Err(e) => io.fail(e),
                },
            }
        }
    }
}

/// The variable's line for listings: `$OPENAI_API_KEY is set: requests that
/// name no key use it.` (none without a variable).
fn variable_line(v: &Value) -> Option<String> {
    let name = v["variable"]["name"].as_str()?;
    Some(if v["variable"]["set"] == true {
        format!("${name} is set: requests that name no key use it.")
    } else {
        format!("${name} isn't set (when it is, requests that name no key use it).")
    })
}

/// A key's status for listings: `found`, `tested ✓ (5 minutes ago)`,
/// `rejected (…)`, `failed (…)` (its last test), or nothing without a key.
fn key_state(k: &Value) -> String {
    if !k["error"].is_null() || k["source"].is_null() {
        return String::new();
    }
    let ago = |c: &Value| {
        let at = c["at"].as_u64().unwrap_or(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(at);
        let d = now.saturating_sub(at);
        let (n, unit) = match d {
            0..=59 => return "just now".to_string(),
            60..=3599 => (d / 60, "minute"),
            3600..=86399 => (d / 3600, "hour"),
            _ => (d / 86400, "day"),
        };
        format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
    };
    match k["check"]["status"].as_str() {
        Some("ok") => format!("tested ✓ ({})", ago(&k["check"])),
        Some("rejected") => format!("rejected ({})", ago(&k["check"])),
        Some("failed") => format!("test failed ({})", ago(&k["check"])),
        _ => "found".into(),
    }
}

/// `openai's keys (★ = used when a request names none):` then one line per
/// key — how it's written and where it comes from — then the variable, and
/// what's not ready.
fn keys_text(v: &Value) -> String {
    let provider = scalar(v.get("provider"));
    let rows = v["keys"].as_array().cloned().unwrap_or_default();
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push(format!(
            "{provider} has no keys. Add one with `lexi providers api-key set {provider}#NAME`."
        ));
    } else {
        let width = rows
            .iter()
            .map(|k| scalar(k.get("label")).len())
            .max()
            .unwrap_or(0);
        lines.push(format!(
            "{provider}'s keys (★ = used when a request names none):"
        ));
        for k in rows {
            let mark = if k["default"] == true { '★' } else { ' ' };
            let from = match (k["error"].as_str(), k["from"].as_str()) {
                (Some(e), _) => format!("error: {e}"),
                (None, Some(f)) => f.to_string(),
                (None, None) => "no key".into(),
            };
            let old = if k["name"] == "" {
                "  (saved before keys had names; rename it to give it one)"
            } else {
                ""
            };
            let label = scalar(k.get("label"));
            let state = match key_state(&k) {
                s if s.is_empty() => s,
                s => format!("  {s}"),
            };
            lines.push(format!("{mark} {label:width$}  {from}{state}{old}"));
        }
    }
    if let Some(l) = variable_line(v) {
        lines.push(l);
    }
    if let Some(r) = v["reason"].as_str() {
        lines.push(format!("Not ready for requests that name no key: {r}."));
    }
    lines.push(format!(
        "Use a key with {provider}#KEY (or --key KEY); add one with `lexi providers api-key set {provider}#NAME`."
    ));
    lines.join("\n")
}

/// One check's line: `openai#work: ready (key from …)`.
fn status_line(row: &Value) -> String {
    let label = scalar(row.get("key"));
    if let Some(e) = row["error"].as_str() {
        return format!("{label}: error: {e}");
    }
    let detail = match (row["from"].as_str(), row["reason"].as_str()) {
        (Some(f), _) => format!(" (key from {f})"),
        (None, Some(r)) => format!(": {r}"),
        _ => String::new(),
    };
    format!("{label}: {}{detail}", scalar(row.get("status")))
}

/// Check a key — or, with none named, every key and the variable (§12.5).
async fn status(io: &mut Io<'_>, name: &str, raw: bool, key: Option<&str>) -> u8 {
    let (provider, key, label) = match super::request_key(io, name, key) {
        Ok(k) => k,
        Err(code) => return code,
    };
    if key.is_some() {
        let check = lexi_host::check_api_key(&provider);
        return match lexi_host::config::with_key(key, check).await {
            Ok(st) => {
                let mut json = st.to_value(&label);
                json["provider"] = json!(provider);
                let human = if raw {
                    json.to_string()
                } else {
                    status_line(&json)
                };
                io.result(human, json)
            }
            Err(e) => io.fail(e),
        };
    }
    let all = match Box::pin(lexi_host::check_provider_keys(&provider)).await {
        Ok(v) => v,
        Err(e) => return io.fail(e),
    };
    let mut lines: Vec<String> = all["keys"]
        .as_array()
        .map(|rows| rows.iter().map(status_line).collect())
        .unwrap_or_default();
    if let Some(st) = all["variable"]["status"].as_str() {
        lines.push(format!("${}: {st}", scalar(all["variable"].get("name"))));
    }
    lines.push(match (&all["uses"], all["reason"].as_str()) {
        (u, _) if u.get("variable").is_some() => format!(
            "{provider}: requests that name no key use ${} (from your environment).",
            scalar(u.get("variable"))
        ),
        (u, _) if u.get("key").is_some() => format!(
            "{provider}: requests that name no key use {} ★.",
            written(&provider, &scalar(u.get("key")))
        ),
        (_, Some("no key")) => format!(
            "{provider}: not ready (no key): add one with `lexi providers api-key set {provider}#NAME`."
        ),
        (_, Some(r)) => format!("{provider}: not ready for requests that name no key ({r})."),
        _ => String::new(),
    });
    let human = if raw {
        all.to_string()
    } else {
        lines.join("\n")
    };
    io.result(human, all)
}

fn add(
    io: &mut Io<'_>,
    name: &str,
    url: &str,
    default_model: &str,
    api_key: Option<(&str, &str)>,
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
        msg.push_str(" Updated providers-config.yaml.");
    }
    match &saved.used {
        Some(used) if *used != saved.location => msg.push_str(&format!(
            "\nNote: {used} comes first and has a key, so that key is used."
        )),
        _ => {}
    }
    msg
}

fn key_set(io: &mut Io<'_>, provider: &str, target: &str, value: Option<String>) -> u8 {
    if let Err(code) = provider_cfg(io, provider) {
        return code;
    }
    // A command that changes a key names it (§12.5): refused before asking
    // for the key.
    if !target.contains('#') {
        return match lexi_host::config::set_api_key(target, "") {
            Err(e) => io.fail(e),
            Ok(_) => 1,
        };
    }
    let key = match value {
        Some(v) => v,
        None => match io.read_secret("Enter API key: ") {
            Ok(k) => k,
            Err(e) => return io.fail(e),
        },
    };
    match lexi_host::config::set_api_key(target, &key) {
        Ok(saved) => {
            let mut json = saved.to_value();
            json["provider"] = json!(provider);
            json["key_name"] = json!(target.split_once('#').map(|(_, k)| k));
            let mut msg = saved_message(&format!("Saved the API key for {target} in"), &saved);
            if saved.added {
                msg = msg.replace(
                    "Updated providers-config.yaml.",
                    "Added the key to providers-config.yaml.",
                );
            }
            io.result(msg, json)
        }
        Err(e) => io.fail(e),
    }
}

/// One key's sources in order with what was found there, then the key
/// (masked unless --reveal).
fn one_key_lines(info: &Value, lines: &mut Vec<String>, indent: &str) {
    let sources = info["sources"].as_array().cloned().unwrap_or_default();
    if sources.is_empty() {
        lines.push(format!("{indent}Its api_key names no source."));
    } else {
        lines.push(format!("{indent}Looked for in this order:"));
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
                "{indent}  {}. {}{saved} — {}{note}",
                i + 1,
                scalar(s.get("from")),
                scalar(s.get("state"))
            ));
        }
    }
    match (info["key"].as_str(), info["error"].as_str()) {
        (_, Some(err)) => lines.push(format!("{indent}No API key: {err}")),
        (Some(key), None) => lines.push(format!("{indent}Key: {key}")),
        (None, None) => lines.push(format!("{indent}No API key found.")),
    }
}

/// A key's sources and key — or, with none named, every key's, and the
/// variable (§12.5).
fn key_show(io: &mut Io<'_>, target: &str, reveal: bool) -> u8 {
    let info = match lexi_host::admin::api_key_info(target, reveal) {
        Ok(info) => info,
        Err(e) => return io.fail(e),
    };
    let mut lines = Vec::new();
    if target.contains('#') {
        one_key_lines(&info, &mut lines, "");
        return io.result(lines.join("\n"), info);
    }
    let provider = scalar(info.get("provider"));
    let keys = info["keys"].as_array().cloned().unwrap_or_default();
    if keys.is_empty() {
        lines.push(format!(
            "{provider} has no keys. Add one with `lexi providers api-key set {provider}#NAME`."
        ));
    }
    for k in &keys {
        let star = if k["default"] == true { " ★" } else { "" };
        lines.push(format!(
            "{}{star}",
            written(&provider, &scalar(k.get("key_name")))
        ));
        one_key_lines(k, &mut lines, "  ");
    }
    if let Some(l) = variable_line(&info) {
        lines.push(l);
    }
    io.result(lines.join("\n"), info)
}

fn key_remove(io: &mut Io<'_>, provider: &str, target: &str) -> u8 {
    if let Err(code) = provider_cfg(io, provider) {
        return code;
    }
    match lexi_host::config::remove_api_key(target) {
        Ok(true) => io.result(
            format!("Removed the saved API key for {target}."),
            json!({ "removed": true }),
        ),
        Ok(false) => io.result(
            format!("No saved API key for {target}."),
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
fn sources_lines(rows: &[Value], lines: &mut Vec<String>, indent: &str) {
    lines.push(format!("{indent}Looked for in this order:"));
    for (i, r) in rows.iter().enumerate() {
        let what = match r["kind"].as_str() {
            Some("saved") if r["implicit"] == true => " (saved key; last unless api_key places it)",
            Some("saved") => " (saved key)",
            _ => "",
        };
        lines.push(format!(
            "{indent}  {}. {}{what}",
            i + 1,
            scalar(r.get("from"))
        ));
    }
}

fn sources_text(list: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(keys) = list["keys"].as_array() {
        let provider = scalar(list.get("provider"));
        if keys.is_empty() {
            lines.push(format!("{provider} has no keys."));
        }
        for k in keys {
            let star = if k["default"] == true { " ★" } else { "" };
            lines.push(format!(
                "{}{star}",
                written(&provider, &scalar(k.get("key_name")))
            ));
            sources_lines(
                k["sources"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                &mut lines,
                "  ",
            );
        }
        if let Some(l) = variable_line(list) {
            lines.push(l);
        }
    } else {
        sources_lines(
            list["sources"].as_array().map(Vec::as_slice).unwrap_or(&[]),
            &mut lines,
            "",
        );
    }
    lines.join("\n")
}

fn key_sources(io: &mut Io<'_>, cmd: KeySourceCmd) -> u8 {
    match cmd {
        KeySourceCmd::List { name, key } => {
            let target = match key_target(io, &name, key.as_deref()) {
                Ok((_, t)) => t,
                Err(code) => return code,
            };
            match lexi_host::config::key_source_list(&target) {
                Ok(list) => io.result(sources_text(&list), list),
                Err(e) => io.fail(e),
            }
        }
        KeySourceCmd::Add {
            name,
            source,
            first,
            key,
        } => {
            let target = match key_target(io, &name, key.as_deref()) {
                Ok((_, t)) => t,
                Err(code) => return code,
            };
            let value = match source_value(&source) {
                Ok(v) => v,
                Err(e) => return io.fail(e),
            };
            match lexi_host::config::add_key_source(&target, &value, first) {
                Ok(list) => io.result(
                    format!("Updated {target}'s api_key.\n{}", sources_text(&list)),
                    list,
                ),
                Err(e) => io.fail(e),
            }
        }
        KeySourceCmd::Rm { name, source, key } => {
            let target = match key_target(io, &name, key.as_deref()) {
                Ok((_, t)) => t,
                Err(code) => return code,
            };
            let value = match source_value(&source) {
                Ok(v) => v,
                Err(e) => return io.fail(e),
            };
            match lexi_host::config::remove_key_source(&target, &value) {
                Ok(removed) => {
                    let msg = if removed {
                        format!("Removed it from {target}'s api_key.")
                    } else {
                        format!("{target}'s api_key doesn't list it.")
                    };
                    io.result(msg, json!({ "provider": target, "removed": removed }))
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
