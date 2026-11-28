//! `models` — a provider's configured models, hosted models, default and aliases.

use serde_json::{json, Value};

use crate::cli::{ModelAliasCmd, ModelMeta, ModelsCmd};
use crate::output::{pretty, scalar, table, Io};

pub(crate) async fn run(io: &mut Io<'_>, cmd: ModelsCmd) -> u8 {
    match cmd {
        ModelsCmd::List {
            provider,
            raw,
            aliases,
            table,
            details,
        } => list(io, &provider, raw, aliases, table, details),
        ModelsCmd::Hosted {
            provider,
            raw,
            schema: true,
            ..
        } => hosted_schema(io, &provider, raw),
        ModelsCmd::Hosted {
            provider,
            raw,
            table,
            tags,
            cached,
            schema: false,
            key,
        } => match super::request_key(io, &provider, key.as_deref()) {
            Ok((p, key, _)) => {
                let list = hosted(io, &p, raw, table, &tags, cached);
                lexi_host::config::with_key(key, list).await
            }
            Err(code) => code,
        },
        ModelsCmd::Default {
            provider,
            model,
            kind,
            clear,
        } => default(io, &provider, model.as_deref(), kind.as_deref(), clear),
        ModelsCmd::Alias { cmd } => match cmd {
            ModelAliasCmd::List { provider, raw } => alias_list(io, &provider, raw),
            ModelAliasCmd::Set {
                provider,
                alias,
                model,
            } => alias_set(io, &provider, &alias, &model),
            ModelAliasCmd::Rm { provider, alias } => alias_rm(io, &provider, &alias),
        },
        ModelsCmd::Add {
            provider,
            model,
            meta,
        } => upsert(io, &provider, &model, meta, false),
        ModelsCmd::Set {
            provider,
            model,
            meta,
        } => upsert(io, &provider, &model, meta, true),
        ModelsCmd::Tag {
            provider,
            model,
            ops,
            clear,
        } => tag(io, &provider, &model, &ops, clear),
        ModelsCmd::Tags { provider, model } => tags(io, &provider, &model),
        ModelsCmd::Rm { provider, model } => {
            if let Err(code) = require_provider(io, &provider) {
                return code;
            }
            match lexi_host::config::delete_model(&provider, &model) {
                Ok(()) => io.result(
                    format!("Removed model {model} from {provider}."),
                    json!({ "provider": provider, "removed": model }),
                ),
                Err(e) => io.fail(e),
            }
        }
    }
}

/// Fail unless `provider` is in providers-config.yaml.
fn require_provider(io: &mut Io<'_>, provider: &str) -> Result<Value, u8> {
    super::plain_provider(io, provider)?;
    let cfg = lexi_host::config::load_providers().map_err(|e| io.fail(e))?;
    cfg.get("providers")
        .and_then(|p| p.get(provider))
        .cloned()
        .ok_or_else(|| io.fail(lexi_host::HostError::UnknownProvider(provider.into())))
}

/// The model's metadata other than `name`/`description`, sorted by key.
fn extra_metadata(meta: &Value) -> Vec<(String, String)> {
    let mut extra: Vec<(String, String)> = meta
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| *k != "name" && *k != "description")
                .map(|(k, v)| (k.clone(), scalar(Some(v))))
                .collect()
        })
        .unwrap_or_default();
    extra.sort();
    extra
}

fn list(
    io: &mut Io<'_>,
    provider: &str,
    raw: bool,
    with_aliases: bool,
    as_table: bool,
    details: bool,
) -> u8 {
    if let Err(code) = require_provider(io, provider) {
        return code;
    }
    let models = match lexi_host::config::load_models(provider) {
        Ok(m) => m,
        Err(e) => return io.fail(e),
    };
    let aliases = match lexi_host::config::model_aliases(provider) {
        Ok(a) => a,
        Err(e) => return io.fail(e),
    };
    let json = match lexi_host::models::model_summaries(provider) {
        Ok(s) => Value::Array(s),
        Err(e) => return io.fail(e),
    };
    let tag_line = |id: &str| -> Option<String> {
        let entry = json.as_array()?.iter().find(|m| m["id"] == id)?;
        let tags: Vec<&str> = entry["tags"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        Some(format!(
            "api: {} | tags: {}",
            scalar(entry.get("api")),
            tags.join(" ")
        ))
    };

    if raw {
        let mut text =
            super::read_home_file(&format!("providers/{provider}/configured-models.yaml"))
                .unwrap_or_default();
        if with_aliases {
            let a = super::read_home_file(&format!("providers/{provider}/model-aliases.yaml"))
                .unwrap_or_default();
            text = format!("{}\n# aliases\n{a}", text.trim_end());
        }
        return io.result(text, json);
    }
    if models.is_empty() {
        return io.result(format!("No models configured for {provider}."), json);
    }
    let human = if as_table {
        let rows = models
            .iter()
            .map(|m| {
                let name = m.name.clone().unwrap_or_else(|| m.id.clone());
                let label = if m.default {
                    format!("{name} (default {})", m.default_for.join(", "))
                } else {
                    name
                };
                let mut aliases = m.aliases.clone();
                aliases.sort();
                let mut desc: Vec<String> = m.description.iter().cloned().collect();
                if details {
                    desc.extend(
                        extra_metadata(&m.metadata)
                            .into_iter()
                            .map(|(k, v)| format!("{k}: {v}")),
                    );
                    desc.extend(tag_line(&m.id));
                }
                vec![label, aliases.join(", "), desc.join("\n")]
            })
            .collect();
        table(
            &format!("Models for {provider}"),
            &["Model Name", "Alias", "Description"],
            rows,
        )
    } else {
        let mut lines = vec![format!("Models for {provider}:")];
        for m in &models {
            let mut extra = Vec::new();
            if m.default {
                extra.push(format!("default {}", m.default_for.join(", ")));
            }
            if !m.aliases.is_empty() {
                let mut a = m.aliases.clone();
                a.sort();
                extra.push(format!("alias: {}", a.join(", ")));
            }
            let suffix = if extra.is_empty() {
                String::new()
            } else {
                format!(" ({})", extra.join(" "))
            };
            lines.push(format!(
                "  {}: {} - {}{suffix}",
                m.id,
                m.name.clone().unwrap_or_default(),
                m.description.clone().unwrap_or_default(),
            ));
            if details {
                for (k, v) in extra_metadata(&m.metadata) {
                    lines.push(format!("    {k}: {v}"));
                }
                if let Some(t) = tag_line(&m.id) {
                    lines.push(format!("    {t}"));
                }
            }
        }
        if !aliases.is_empty() {
            lines.push("Aliases:".into());
            for (alias, target) in &aliases {
                lines.push(format!("  {alias} -> {}", scalar(Some(target))));
            }
        }
        lines.join("\n")
    };
    io.result(human, json)
}

/// `models hosted --schema`: the endpoint and catalog settings, as YAML.
fn hosted_schema(io: &mut Io<'_>, provider: &str, raw: bool) -> u8 {
    let info = match lexi_host::hosted::hosted_schema(provider) {
        Ok(v) => v,
        Err(e) => return io.fail(e),
    };
    if raw {
        return io.result(pretty(&info), info);
    }
    let url = scalar(info.get("url"));
    let mut lines = vec![format!("{provider}'s hosted models: GET {url}")];
    let empty = info["catalog"].as_object().is_none_or(|c| c.is_empty());
    if empty {
        lines.push(
            "No catalog settings: the models are read from the response's `data` (or `models`) list, by `id`."
                .into(),
        );
    } else {
        let from = match (info["source"].as_str(), info["template"].as_str()) {
            (Some("template"), Some(t)) => format!("the {t} template"),
            (Some("config"), Some(t)) => format!("providers-config.yaml (over the {t} template)"),
            _ => "providers-config.yaml".to_string(),
        };
        lines.push(format!("Catalog settings, from {from}:"));
        let yaml = serde_yaml::to_string(&info["catalog"]).unwrap_or_default();
        lines.extend(yaml.lines().map(|l| format!("  {l}")));
    }
    io.result(lines.join("\n"), info)
}

async fn hosted(
    io: &mut Io<'_>,
    provider: &str,
    raw: bool,
    as_table: bool,
    tags: &[String],
    cached: bool,
) -> u8 {
    let pcfg = match require_provider(io, provider) {
        Ok(c) => c,
        Err(code) => return code,
    };
    let items = match lexi_host::models::hosted_summaries(provider, cached, tags).await {
        Ok(i) => i,
        Err(e) => return io.fail(e),
    };
    let json = Value::Array(items.clone());
    if raw {
        return io.result(pretty(&json), json);
    }
    if items.is_empty() {
        let what = if tags.is_empty() {
            String::new()
        } else {
            format!(" tagged {}", tags.join(" "))
        };
        return io.result(format!("No hosted models{what} for {provider}."), json);
    }
    // Columns: the catalog's fields (else the first item's keys), then Lexi's
    // classification: kind and the API that serves it.
    let catalog_fields = pcfg
        .pointer("/catalog/fields")
        .or_else(|| pcfg.get("hosted_model_fields"))
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty());
    let mut fields: Vec<String> = catalog_fields
        .map(|a| a.iter().map(|v| scalar(Some(v))).collect())
        .or_else(|| {
            items[0].as_object().map(|o| {
                o.keys()
                    .filter(|k| !["kind", "api", "tags"].contains(&k.as_str()))
                    .cloned()
                    .collect()
            })
        })
        .unwrap_or_else(|| vec!["id".into()]);
    fields.extend(["kind".to_string(), "api".to_string()]);
    let cells = |item: &Value| -> Vec<String> {
        match item.as_object() {
            Some(o) => fields.iter().map(|f| scalar(o.get(f))).collect(),
            None => {
                let mut row = vec![scalar(Some(item))];
                row.resize(fields.len(), String::new());
                row
            }
        }
    };
    let human = if as_table {
        let headers: Vec<&str> = fields.iter().map(String::as_str).collect();
        table(
            &format!("Hosted models for {provider}"),
            &headers,
            items.iter().map(cells).collect(),
        )
    } else {
        let mut lines = vec![format!("Hosted models for {provider}:")];
        for item in &items {
            if item.is_object() {
                let pairs: Vec<String> = fields
                    .iter()
                    .zip(cells(item))
                    .map(|(f, v)| format!("{f}={v}"))
                    .collect();
                lines.push(format!("  {}", pairs.join(", ")));
            } else {
                lines.push(format!("  {}", scalar(Some(item))));
            }
        }
        lines.join("\n")
    };
    io.result(human, json)
}

/// `gpt-4o (yours)`, `gpt-4o (the provider's)`, `none`.
fn default_label(row: &Value) -> String {
    match row["model"].as_str() {
        Some(m) if row["source"] == "user" => format!("{m} (yours)"),
        Some(m) => format!("{m} (the provider's)"),
        None => "none".into(),
    }
}

fn default(
    io: &mut Io<'_>,
    provider: &str,
    model: Option<&str>,
    kind: Option<&str>,
    clear: bool,
) -> u8 {
    if let Err(code) = require_provider(io, provider) {
        return code;
    }
    if clear {
        let kind = kind.unwrap_or("chat");
        return match lexi_host::models::clear_default(provider, kind) {
            Ok(v) => {
                let now = default_label(&v);
                let msg = if v["cleared"] == true {
                    format!("Cleared your default {kind} model for {provider}; it's {now} now.")
                } else {
                    format!("You had no default {kind} model for {provider}; it's {now}.")
                };
                io.result(msg, v)
            }
            Err(e) => io.fail(e),
        };
    }
    if let Some(model) = model {
        return match lexi_host::models::set_default(provider, model, kind) {
            Ok(v) => io.result(
                format!(
                    "{} is now the default {} model for {provider}.",
                    scalar(v.get("model")),
                    scalar(v.get("kind"))
                ),
                v,
            ),
            Err(e) => io.fail(e),
        };
    }
    let v = match lexi_host::models::defaults(provider, kind) {
        Ok(v) => v,
        Err(e) => return io.fail(e),
    };
    if let Some(kind) = kind {
        let msg = format!("Default {kind} model for {provider}: {}", default_label(&v));
        return io.result(msg, v);
    }
    let mut lines = vec![format!("Default models for {provider}:")];
    lines.extend(
        v["defaults"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| format!("  {}: {}", scalar(r.get("kind")), default_label(r))),
    );
    io.result(lines.join("\n"), v)
}

fn alias_list(io: &mut Io<'_>, provider: &str, raw: bool) -> u8 {
    if let Err(code) = require_provider(io, provider) {
        return code;
    }
    let aliases = match lexi_host::config::model_aliases(provider) {
        Ok(a) => a,
        Err(e) => return io.fail(e),
    };
    let mut aliases: Vec<(String, Value)> = aliases.into_iter().collect();
    aliases.sort_by(|a, b| a.0.cmp(&b.0));
    let json = Value::Array(
        aliases
            .iter()
            .map(|(a, m)| json!({ "alias": a, "model": m }))
            .collect(),
    );
    if raw {
        let path = format!("providers/{provider}/model-aliases.yaml");
        let text = super::read_home_file(&path)
            .unwrap_or_else(|_| format!("No aliases file for {provider}."));
        return io.result(text, json);
    }
    let human = if aliases.is_empty() {
        format!("No aliases configured for {provider}.")
    } else {
        aliases
            .iter()
            .map(|(a, m)| format!("  {a} -> {}", scalar(Some(m))))
            .collect::<Vec<_>>()
            .join("\n")
    };
    io.result(human, json)
}

fn upsert(io: &mut Io<'_>, provider: &str, model: &str, meta: ModelMeta, is_set: bool) -> u8 {
    if let Err(code) = require_provider(io, provider) {
        return code;
    }
    let mut fields = serde_json::Map::new();
    if let Some(n) = meta.name {
        fields.insert("name".into(), Value::from(n));
    }
    if let Some(d) = meta.description {
        fields.insert("description".into(), Value::from(d));
    }
    if let Some(m) = meta.max_tokens {
        fields.insert("max_tokens".into(), Value::from(m));
    }
    if let Some(c) = meta.context_window {
        fields.insert("context_window".into(), Value::from(c));
    }
    match lexi_host::config::upsert_model(provider, model, &fields, meta.default) {
        Ok(existed) => {
            let action = if is_set && existed {
                "Updated"
            } else {
                "Added"
            };
            io.result(
                format!("{action} model {model} for {provider}."),
                json!({ "provider": provider, "model": model, "existed": existed }),
            )
        }
        Err(e) => io.fail(e),
    }
}

fn alias_set(io: &mut Io<'_>, provider: &str, alias: &str, model: &str) -> u8 {
    if let Err(code) = require_provider(io, provider) {
        return code;
    }
    match lexi_host::config::set_model_alias(provider, alias, model) {
        Ok(()) => io.result(
            format!("Set alias '{alias}' -> '{model}' for {provider}."),
            json!({ "provider": provider, "alias": alias, "model": model }),
        ),
        Err(e) => io.fail(e),
    }
}

fn alias_rm(io: &mut Io<'_>, provider: &str, alias: &str) -> u8 {
    if let Err(code) = require_provider(io, provider) {
        return code;
    }
    match lexi_host::config::delete_model_alias(provider, alias) {
        Ok(()) => io.result(
            format!("Removed alias '{alias}' from {provider}."),
            json!({ "provider": provider, "removed": alias }),
        ),
        Err(e) => io.fail(e),
    }
}

fn tag(io: &mut Io<'_>, provider: &str, model: &str, ops: &[String], clear: bool) -> u8 {
    if clear {
        return match lexi_host::models::clear_model_tags(provider, model) {
            Ok(had) => {
                let human = if had {
                    format!("Cleared your tags on {model}.")
                } else {
                    format!("No tags of yours on {model}.")
                };
                io.result(
                    human,
                    json!({ "provider": provider, "model": model, "cleared": had }),
                )
            }
            Err(e) => io.fail(e),
        };
    }
    match lexi_host::models::tag_model(provider, model, ops) {
        Ok(now) => io.result(
            format!("{model}: {}", now.join(" ")),
            json!({ "provider": provider, "model": model, "tags": now }),
        ),
        Err(e) => io.fail(e),
    }
}

/// `models tags`: the classification, tag by tag with its source.
fn tags(io: &mut Io<'_>, provider: &str, model: &str) -> u8 {
    let cls = match lexi_host::models::classify(provider, model) {
        Ok(c) => c,
        Err(e) => return io.fail(e),
    };
    let mut lines = vec![format!(
        "{model}: kind {} → api {}",
        scalar(cls.get("kind")),
        if cls["api"].is_null() {
            "(none)".to_string()
        } else {
            scalar(cls.get("api"))
        }
    )];
    for t in cls["tags"].as_array().into_iter().flatten() {
        lines.push(format!(
            "  {:<28} {}",
            scalar(t.get("tag")),
            scalar(t.get("source"))
        ));
    }
    if let Some(err) = cls.get("error").and_then(Value::as_str) {
        lines.push(format!("unsupported: {err}"));
    }
    io.result(lines.join("\n"), cls)
}

/// `tags`: the vocabulary.
pub(crate) fn vocabulary(io: &mut Io<'_>) -> u8 {
    let v = lexi_host::models::tag_vocabulary();
    let mut lines = Vec::new();
    for (ns, tags) in v["namespaces"].as_object().into_iter().flatten() {
        let names: Vec<&str> = tags
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        lines.push(format!("{ns}: {}", names.join(", ")));
    }
    lines.push("label:<name>  (your own labels, for filtering only)".into());
    lines.push(String::new());
    lines.push("Kinds imply input/output tags:".into());
    for (kind, io_) in v["kinds"].as_object().into_iter().flatten() {
        let list = |k: &str| {
            io_[k]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        };
        lines.push(format!(
            "  kind:{kind:<18} in:{}  out:{}",
            list("in"),
            list("out")
        ));
    }
    io.result(lines.join("\n"), v)
}
