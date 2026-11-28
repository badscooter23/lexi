# Lexi CLI — Machine Interface v1

**Status:** v1 · 2026-11-16 · implemented by `lexi-cli-rs` (the Rust `lexi`). v1 only adds to v0 (see §7): every v0 field is still there with the same meaning.
**Audience:** programs, scripts and agents that drive `lexi` and read its output.
The Python library (`import lexi`) returns these same shapes as dicts.

Every `lexi` command has a human form and a machine form. The machine form is
selected with `--json` and is the stable contract described here. Human output
(tables, wording, colors) can change; the shapes below change only with a new
version of this document.

## 1. Invocation

- `--json` is global: `lexi --json models list openai`, `lexi models list openai --json`
  and (for `respond`) `lexi respond openai --text hi --json` are all equivalent.
- Command aliases from `~/.lexi-cli/aliases.json` expand in machine mode too
  (`lexi --json prov list`). Scripts that need to be independent of a user's
  aliases should use full command names (aliases never shadow them).
- `$LEXI_HOME` overrides the config directory (default `~/.lexi-cli`).

## 2. Streams and exit codes

| | Success | Command failure | Usage error |
|---|---|---|---|
| Exit code | `0` | `1` | `2` |
| stdout | the result (§3) — one JSON value on one line, or NDJSON events (§4) | empty (except streamed events and `api validate` / `jobs`, below) | empty |
| stderr | warnings only (human text) | `{"error": "<message>"}` | clap's usage text (human) |

Exceptions, where a failure still has a structured result on stdout:
`api validate` on an invalid definition prints `{"name", "valid": false, "errors"}`
and exits `1`; `jobs status` / `jobs wait` on a failed job print the operation
(§5) and exit `1`; a streamed `respond` that fails ends with a `provider_done`
event carrying `error` (§4) and exits `1`.

## 3. Result shapes

API keys are never included in any result except `providers api-key show`
(masked unless `--reveal`).

| Command | Result |
|---|---|
| `version` | `{"version", "implementation": "rust"}` |
| `config list` | the merged settings object (defaults overlaid by `config.json`) |
| `config list <key>` | `{"key", "value"}` |
| `config set` / `config rm` | `{"key", "value"}` / `{"removed"}` |
| `prompts list` | `[{"name", "active", "prompt", "max_tokens", "temperature", "role", …}]` — active (`$$`) first |
| `prompts list <name>` · `prompts set` | one prompt object as above |
| `prompts rm` · `prompts activate` | `{"removed"}` · `{"active"}` |
| `prompts <text…>` | the ad-hoc payload `{"prompt", "max_tokens", "temperature", "role"}` |
| `providers list` | `[{"name", "template", "url", "api", "apis", "default_model", "hosted_models", "hosted_model_fields", "key_source"}]`; `api` is the catch-all binding's API, `apis` every binding's; `key_source` ∈ `"file" \| "env" \| "config" \| null` |
| `providers status` | `{"provider", "status", "source"}`; `status` ∈ `"ready" \| "no api-key" \| "invalid api-key"` |
| `providers add` / `rename` / `rm` | `{"added"}` / `{"renamed", "to"}` / `{"removed"}` |
| `providers upgrade` | `[{"provider", "template", "changed", "before", "after", "dropped", "kept", "backup"?}]` — `backup` (the copy of the old file) only with `--yes` |
| `providers api list` | `[{"api", "for"?, "default_model"?, "url"?, "enabled", "source"}]`; `source` ∈ `"template" \| "user" \| "config"` |
| `providers api add` / `rm` | `{"provider", "api", "existed"}` / `{"provider", "removed"}` |
| `providers api-key set` / `show` / `remove` | `{"provider", "location", "store", "added", "used"}` / `{"provider", "api", "own", "source", "from", "key", "error", "sources", "save_to", "store", "env"}` / `{"removed": bool}`. `location` / `save_to` is where the saved key is kept: `"the macOS Keychain"` or its key file; `store` is `"keychain" \| "file"` for `set`, and `{"kind", "available", "locked"}` for `show` (`locked`: reading the saved key failed because the keychain is locked). `source` ∈ `"env" \| "file" \| "command" \| "saved" \| "config"`; each of `sources` is `{"kind", "from", "state", "note"?}`, `state` ∈ `"used" \| "missing" \| "error" \| "not run" \| "not checked"` |
| `providers api-key source list` / `add` / `rm` | `{"provider", "api", "own", "sources": [{"kind", "from", "value", "implicit"?}]}` (list and add) / `{"provider", "removed": bool}` |
| `models list` | `[{"id", "name", "description", "default", "default_for", "aliases", "metadata", "kind", "api", "tags"}]` — `default_for` the kinds it's your default for; `api` serves the model; `tags` its effective tags |
| `models tag` | `{"provider", "model", "tags": ["+tag" \| "-tag", …]}` (your ops on that id/glob) · with `--clear`: `{"provider", "model", "cleared": bool}` |
| `models tags` | `{"provider", "model", "kind", "api", "binding", "tags": [{"tag", "source"}], "in_catalog", "limits"?, "name"?, "error"?}` (`limits`/`name` from the cached catalog) — `api`/`binding` null and `error` set when no API serves the model; `source` ∈ `api:<name>`, `rule:<glob>`, `catalog`, `user`, `kind` |
| `tags` | `{"namespaces": {"in", "out", "kind", "feature", "status"}, "kinds": {"<kind>": {"in", "out"}}, "labels"}` |
| `models hosted` | the provider's model entries, as a flat array, each with Lexi's `"kind"`, `"api"` and `"tags"` added; `--tag T` keeps entries having every given tag; `--cached` reads the last fetch |
| `models default` | `{"provider", "default_model", "defaults": [{"kind", "model", "source"}]}` — `default_model` is the chat default; `source` ∈ `user` (yours), `binding`, `provider`; `model`/`source` null when there's none · with `--kind K`: `{"provider", "kind", "model", "source"}` · setting: `{"provider", "kind", "model"}` · `--clear`: `{"provider", "kind", "cleared": bool, "model", "source"}` (the default now in effect) |
| `models add` / `set` / `rm` | `{"provider", "model", "existed"}` / same / `{"provider", "removed"}` |
| `models alias list` | `[{"alias", "model"}]` |
| `models alias set` / `rm` | `{"provider", "alias", "model"}` / `{"provider", "removed"}` |
| `api list` | `[{"name", "description", "path", "text_path", "builtin"}]` |
| `api show` | the definition object |
| `api validate` | `{"name", "valid", "errors"}` |
| `api preview` | `{"name", "description", "valid", "errors", "flags", "presets", "body" \| "body_error"}` |
| `api create` / `edit` / `save` / `rm` | `{"created"}` / `{"updated"}` / `{"saved", "valid", "errors"}` / `{"removed"}` |
| `respond` (buffered) | `{"provider", "model", "api", "text", "artifacts", "metadata", "activity"?, "sources"?, "raw"?, "warnings"?}` — `api` is the API that served it; `artifacts` the files saved (§6); `sources` with `--cite`, `raw` with `--raw` |
| `respond --background --detach` | the operation (§5) plus `"provider"` |
| `generate` | the `respond` result (it is `respond --output image`) |
| `embed` | `{"provider", "model", "api", "count", "dimensions", "embeddings": [[number, …], …], "metadata"}` — one vector per input, in input order |
| `upload` | `{"provider", "file_id"}` |
| `jobs status` / `jobs wait` | the operation (§5) plus `"provider"` and, once succeeded, `"text"` |
| `alias list` | `{"<expansion>": ["<alias>", …]}` |
| `alias add` / `rm` / `reset` | `{"alias", "expansion"}` / `{"removed"}` / `{"reset": true}` |
| `history` | `[{"n", "command"}]` |
| `help …` | human text (not JSON) |

`metadata` holds the API definition's `response.metadata` fields that the
response carried (typically `model`, `input_tokens`, `output_tokens`,
`total_tokens`, `stop_reason`).

## 4. Streaming events (NDJSON)

`respond --stream --json` writes one JSON object per line as the response
arrives. The event names and fields are the same ones the desktop app's chat
stream uses.

```json
{"event": "provider_start", "provider": "openai"}
{"event": "activity", "provider": "openai", "message": "🔍 searching the web"}
{"event": "delta", "provider": "openai", "text": "Hel"}
{"event": "delta", "provider": "openai", "text": "lo"}
{"event": "artifact", "provider": "openai", "path": "fox-1.png", "type": "image", "media_type": "image/png", "revised_prompt": null}
{"event": "provider_done", "provider": "openai", "model": "gpt-4o", "api": "responses", "text": "Hello", "artifacts": [...], "metadata": {}}
```

- `activity` events are optional and API-dependent (`response.stream.activity_events`).
- `artifact` events (one per saved file) come after the last delta, before `provider_done`.
- `provider_done` carries the same fields as a buffered result (§3) — `text` is
  the concatenated deltas; `metadata`, `sources` and `raw` come from the
  response object in the stream's final event (`response.stream.final_event` /
  `final_path`) when the API sends one.
- On failure the last event is `{"event": "provider_done", "provider", "error"}`
  and the exit code is `1`.

## 5. Operations (background jobs)

A background job is reported as the engine's canonical operation:

```json
{"id": "resp_123", "status": "in_progress", "state": "running", "done": false}
```

`state` ∈ `pending | running | succeeded | failed | unknown` (provider statuses
are mapped by the API definition's `operation.status_map`); `done` is true for
`succeeded` and `failed`. Submit with `respond <provider> … --background --detach`,
then `jobs status <provider> <id>` (one check) or `jobs wait <provider> <id>
[--interval S] [--timeout S]`.

## 6. Files

Generated media is written to disk and reported by path (`respond --out DIR`,
default the current directory); file contents never appear on stdout. Each
artifact is `{"path", "type", "media_type", "revised_prompt"}`; files are named
`<prompt-words>-<n>.<ext>` with the first unused `n`, so nothing is overwritten.

## 7. Changes from v0

All additive — no v0 field was removed or changed meaning.

- `respond`: `api`, `artifacts` (§6) and `warnings`? in the result; `artifact`
  stream events; `generate` returns the `respond` result.
- `embed`: new.
- `models list`: `kind`, `api`, `tags`. `models hosted`: `kind`, `api`, `tags`
  on each entry; `--tag`, `--cached`. `models tag`, `models tags`, `tags`: new.
- `providers list`: `template`, `apis`. `providers upgrade`,
  `providers api list|add|rm`: new.
- Errors for unsupported models/outputs and invalid tags use the same
  `{"error"}` shape (§2).
