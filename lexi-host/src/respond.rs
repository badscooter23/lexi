//! The chat pipeline: compose (lexi-core) → POST → extract (lexi-core), in
//! buffered, streaming (SSE) and background (submit/poll) forms.
//!
//! Two layers:
//! - **Full chat** ([`ChatRequest`] → [`prepare_chat`] → [`run_chat`]): a saved
//!   prompt payload plus everything the CLI's `respond` takes — model (aliases
//!   resolved), images, `--params` overrides, tool presets, `--set`/`--unset`
//!   paths. Model resolution and per-model config overrides follow the Python
//!   CLI. `prepare_chat` does no network I/O, so the composed body is
//!   inspectable.
//! - **Simple text chat** (`respond_text` / `respond_stream_text` /
//!   `respond_background_text`, and their `*_with_effective` forms): prompt +
//!   role, used by the desktop app and the mock-server tests.

use futures_util::StreamExt;
use serde_json::{json, Map, Value};

use crate::image::ResolvedImage;
use crate::{auth, config, HostError};

/// The result of a chat: the extracted assistant text + the raw response.
/// For streaming, `raw` is the stream's final response event (`final_event`,
/// else `done_event`) when the API sends one, otherwise `null`.
#[derive(Debug, Clone)]
pub struct RespondResult {
    pub text: String,
    pub raw: Value,
}

/// A streaming callback event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvent<'a> {
    /// A fragment of assistant text.
    Delta(&'a str),
    /// A status label for provider-side work (e.g. "searching the web…"),
    /// from the stream config's `activity_events`.
    Activity(&'a str),
}

pub(crate) fn endpoint_url(effective_cfg: &Value) -> String {
    let base = effective_cfg
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("");
    let path = effective_cfg
        .get("request")
        .and_then(|r| r.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("");
    format!("{base}{path}")
}

/// Compose a request body from a prompt payload (`{prompt, role, max_tokens,
/// temperature, …}`) via the engine: the payload's text (plus any images)
/// becomes a canonical message lowered through the API's `content_mapping`;
/// the payload's other fields stay available as bindings. Precedence (low →
/// high): schema default → payload → `overrides` → tool presets → `set_paths`
/// → `unset_paths`.
#[allow(clippy::too_many_arguments)]
pub fn compose_chat_body(
    effective_cfg: &Value,
    payload: &Value,
    images: &[ResolvedImage],
    overrides: &Value,
    tool_presets: &[String],
    set_paths: &[(String, Value)],
    unset_paths: &[String],
    skip_validation: bool,
) -> Result<Value, HostError> {
    let text = payload.get("prompt").and_then(Value::as_str).unwrap_or("");
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let mut content = vec![json!({ "type": "text", "text": text })];
    for img in images {
        content.push(img.part_value());
    }
    let message = json!({ "role": role, "content": content });
    let msg = lexi_core::message_from_dict(&message);
    let lowered = lexi_core::lower_message(&msg, effective_cfg.get("content_mapping"));

    let mut bindings = payload.as_object().cloned().unwrap_or_default();
    if let Value::Object(l) = lowered {
        bindings.extend(l);
    }
    lexi_core::compose_request_body(
        effective_cfg,
        &Value::Object(bindings),
        overrides,
        tool_presets,
        set_paths,
        unset_paths,
        skip_validation,
    )
    .map_err(HostError::Compose)
}

/// Simple-chat body: just `prompt` + `role` (+ images) and `overrides`.
pub(crate) fn compose_body(
    effective_cfg: &Value,
    prompt: &str,
    role: &str,
    images: &[ResolvedImage],
    overrides: &Value,
) -> Result<Value, HostError> {
    compose_chat_body(
        effective_cfg,
        &json!({ "prompt": prompt, "role": role }),
        images,
        overrides,
        &[],
        &[],
        &[],
        false,
    )
}

fn text_extract(effective_cfg: &Value) -> Option<&Value> {
    effective_cfg
        .get("response")
        .and_then(|r| r.get("text_extract"))
}

/// A request that failed because the key's account can't pay — no credit,
/// no quota left — said in plain words, with the key marked `no_credit` in
/// its last test (so the key list says so). The provider's template says how
/// to tell (`billing_errors: [{match, status?, message}]`: `match` a
/// case-insensitive part of the error body, `message` what to do). Any other
/// error comes back as it is.
pub(crate) fn explain_account_error(provider: &str, eff: &Value, err: HostError) -> HostError {
    let HostError::HttpStatus { status, body } = &err else {
        return err;
    };
    let rules = eff.get("billing_errors").and_then(Value::as_array);
    let lower = body.to_lowercase();
    let Some(rule) = rules.into_iter().flatten().find(|r| {
        let m = r.get("match").and_then(Value::as_str).unwrap_or("");
        let s = r.get("status").and_then(Value::as_u64);
        !m.is_empty()
            && lower.contains(&m.to_lowercase())
            && s.is_none_or(|s| s == u64::from(*status))
    }) else {
        return err;
    };
    let message = rule
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the account for this key can't pay for requests");
    // Which key it was (the same choice the request made), to mark it.
    let who = match config::lookup_chosen_key(provider, eff, false) {
        Ok((lookup, label)) => {
            if let Some(k) = lookup.key.as_deref() {
                config::record_key_check(provider, &label, "no_credit", k);
            }
            if label.starts_with('$') {
                label
            } else {
                format!("{provider}{label}")
            }
        }
        Err(_) => provider.to_string(),
    };
    // The provider's own words, when its error is the usual JSON.
    let said = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| body.chars().take(200).collect());
    HostError::Account(format!("{who}: {message}. (The provider said: {said})"))
}

async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, HostError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(HostError::HttpStatus {
        status: status.as_u16(),
        body,
    })
}

/// Run one provider request — sending it and reading the whole reply —
/// within `request_timeout` (config.json), if there is one.
pub(crate) async fn within_limit<T>(
    fut: impl std::future::Future<Output = Result<T, HostError>>,
) -> Result<T, HostError> {
    match config::request_timeout() {
        None => fut.await,
        Some(limit) => tokio::time::timeout(limit, fut).await.unwrap_or_else(|_| {
            Err(HostError::RequestTimeout(format!(
                "no answer from the provider within {} seconds, so Lexi stopped waiting ({}; 0 = no limit)",
                limit.as_secs(),
                config::timeout_source()
            )))
        }),
    }
}

/// A stream that went quiet for longer than `request_timeout`.
fn silent_for(limit: std::time::Duration) -> HostError {
    HostError::RequestTimeout(format!(
        "the provider sent nothing for {} seconds, so Lexi stopped the stream ({}; 0 = no limit)",
        limit.as_secs(),
        config::timeout_source()
    ))
}

/// POST a composed body and extract the text from the JSON reply.
async fn post_buffered(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    body: &Value,
) -> Result<RespondResult, HostError> {
    let raw: Value = within_limit(async {
        let resp = client
            .post(endpoint_url(effective_cfg))
            .headers(auth::build_headers(effective_cfg, api_key))
            .json(body)
            .send()
            .await?;
        Ok(check_status(resp).await?.json().await?)
    })
    .await?;
    let text = lexi_core::extract_text(&raw, text_extract(effective_cfg));
    Ok(RespondResult { text, raw })
}

/// POST a composed body and consume the SSE reply: deltas and activity labels
/// go to `on_event`; the final response event (if any) becomes `raw`.
/// `request_timeout` limits each silence — waiting for the reply to start, or
/// for its next piece — not the stream's length; deltas already passed to
/// `on_event` stay delivered when it runs out.
async fn post_stream<F: FnMut(StreamEvent)>(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    body: &Value,
    mut on_event: F,
) -> Result<RespondResult, HostError> {
    let limit = config::request_timeout();
    let send = client
        .post(endpoint_url(effective_cfg))
        .headers(auth::build_headers(effective_cfg, api_key))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(body)
        .send();
    let resp = match limit {
        Some(d) => tokio::time::timeout(d, send)
            .await
            .map_err(|_| silent_for(d))??,
        None => send.await?,
    };
    let resp = check_status(resp).await?;

    let stream_cfg = effective_cfg
        .get("response")
        .and_then(|r| r.get("stream"))
        .cloned()
        .unwrap_or(Value::Null);
    let final_event = stream_cfg
        .get("final_event")
        .or_else(|| stream_cfg.get("done_event"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut buffer = String::new();
    let mut accumulated = String::new();
    let mut final_response = Value::Null;
    let mut bytes = resp.bytes_stream();

    'outer: loop {
        let next = match limit {
            Some(d) => tokio::time::timeout(d, bytes.next())
                .await
                .map_err(|_| silent_for(d))?,
            None => bytes.next().await,
        };
        let Some(chunk) = next else { break };
        buffer.push_str(&String::from_utf8_lossy(&chunk?));
        // An SSE event block ends at a blank line.
        while let Some(idx) = buffer.find("\n\n") {
            let block: String = buffer.drain(..idx + 2).collect();
            let parsed = lexi_core::parse_sse_block(&block);
            let event = parsed["event"].as_str();
            let data = parsed["data"].as_str().unwrap_or("");
            if final_event.is_some() && event == final_event.as_deref() {
                if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(data) {
                    // `final_path` points at the response object inside the
                    // event (OpenAI wraps it: `{type, response: {...}}`).
                    final_response = stream_cfg
                        .get("final_path")
                        .and_then(Value::as_str)
                        .and_then(|p| lexi_core::get_path(&v, p))
                        .filter(|r| r.is_object())
                        .cloned()
                        .unwrap_or(v);
                }
            }
            let cls = lexi_core::classify_event(event, data, &stream_cfg);
            match cls["kind"].as_str() {
                Some("delta") => {
                    let frag = cls["text"].as_str().unwrap_or("");
                    accumulated.push_str(frag);
                    on_event(StreamEvent::Delta(frag));
                }
                Some("activity") => {
                    if let Some(label) = cls["label"].as_str() {
                        on_event(StreamEvent::Activity(label));
                    }
                }
                Some("done") => break 'outer,
                _ => {}
            }
        }
    }

    Ok(RespondResult {
        text: accumulated,
        raw: final_response,
    })
}

/// A background operation's state — `{id, status, state, done}` from the
/// engine's `read_operation` — plus the extracted result once it succeeded.
#[derive(Debug, Clone)]
pub struct OperationStatus {
    pub operation: Value,
    pub result: Option<RespondResult>,
}

impl OperationStatus {
    pub fn done(&self) -> bool {
        self.operation["done"].as_bool().unwrap_or(false)
    }
    pub fn failed(&self) -> bool {
        self.operation["state"].as_str() == Some("failed")
    }

    /// The machine-interface form: the operation plus `provider` and, once it
    /// succeeded, `text`.
    pub fn to_value(&self, provider: &str) -> Value {
        let mut out = self.operation.clone();
        out["provider"] = Value::from(provider);
        if let Some(res) = &self.result {
            out["text"] = Value::from(res.text.clone());
        }
        out
    }
}

fn operation_cfg(effective_cfg: &Value) -> Result<Value, HostError> {
    effective_cfg.get("operation").cloned().ok_or_else(|| {
        HostError::Config("this API defines no `operation` (background) lifecycle".into())
    })
}

/// Classify a response as an operation, extracting the result when it has
/// succeeded.
fn operation_status_of(effective_cfg: &Value, op_cfg: &Value, response: Value) -> OperationStatus {
    let operation = lexi_core::read_operation(&response, Some(op_cfg));
    let result = (operation["state"].as_str() == Some("succeeded")).then(|| RespondResult {
        text: lexi_core::extract_text(&response, text_extract(effective_cfg)),
        raw: response,
    });
    OperationStatus { operation, result }
}

/// Submit a composed background body; returns the operation as first reported.
async fn submit_background(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    body: &Value,
) -> Result<OperationStatus, HostError> {
    let op_cfg = operation_cfg(effective_cfg)?;
    let response: Value = within_limit(async {
        let resp = client
            .post(endpoint_url(effective_cfg))
            .headers(auth::build_headers(effective_cfg, api_key))
            .json(body)
            .send()
            .await?;
        Ok(check_status(resp).await?.json().await?)
    })
    .await?;
    Ok(operation_status_of(effective_cfg, &op_cfg, response))
}

/// Poll an operation once (`operation.poll`).
async fn poll_operation(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    id: &Value,
) -> Result<OperationStatus, HostError> {
    let op_cfg = operation_cfg(effective_cfg)?;
    let base_url = effective_cfg
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("");
    let poll_url = format!("{base_url}{}", lexi_core::poll_path(&op_cfg, id));
    let response: Value = within_limit(async {
        let resp = client
            .get(&poll_url)
            .headers(auth::build_headers(effective_cfg, api_key))
            .send()
            .await?;
        Ok(check_status(resp).await?.json().await?)
    })
    .await?;
    Ok(operation_status_of(effective_cfg, &op_cfg, response))
}

/// Poll until the operation is terminal. A failed operation is an error.
async fn wait_for(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    mut status: OperationStatus,
    poll_interval: std::time::Duration,
    max_polls: usize,
) -> Result<OperationStatus, HostError> {
    let mut polls = 0;
    while !status.done() {
        if polls >= max_polls {
            return Err(HostError::OperationTimeout);
        }
        tokio::time::sleep(poll_interval).await;
        let id = status.operation.get("id").cloned().unwrap_or(Value::Null);
        status = poll_operation(client, effective_cfg, api_key, &id).await?;
        polls += 1;
    }
    if status.failed() {
        return Err(HostError::OperationFailed(
            status.operation["status"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
        ));
    }
    Ok(status)
}

/// Submit a background body, poll to completion, and return the result.
async fn post_background(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    body: &Value,
    poll_interval: std::time::Duration,
    max_polls: usize,
) -> Result<RespondResult, HostError> {
    let submitted = submit_background(client, effective_cfg, api_key, body).await?;
    let done = wait_for(
        client,
        effective_cfg,
        api_key,
        submitted,
        poll_interval,
        max_polls,
    )
    .await?;
    done.result
        .ok_or_else(|| HostError::OperationFailed("finished without a result".into()))
}

// ---------------------------------------------------------------------------
// Simple text chat over an already-resolved effective config (testable
// against a mock server; no config I/O).
// ---------------------------------------------------------------------------

/// Buffered text chat.
pub async fn respond_with_effective(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    prompt: &str,
    role: &str,
    model: &str,
    images: &[ResolvedImage],
) -> Result<RespondResult, HostError> {
    let body = compose_body(
        effective_cfg,
        prompt,
        role,
        images,
        &json!({ "model": model }),
    )?;
    post_buffered(client, effective_cfg, api_key, &body).await
}

/// Streaming text chat: `on_delta` receives each text fragment as it arrives.
#[allow(clippy::too_many_arguments)]
pub async fn respond_stream_with_effective<F: FnMut(&str)>(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    prompt: &str,
    role: &str,
    model: &str,
    images: &[ResolvedImage],
    mut on_delta: F,
) -> Result<RespondResult, HostError> {
    let body = compose_body(
        effective_cfg,
        prompt,
        role,
        images,
        &json!({ "model": model, "stream": true }),
    )?;
    post_stream(client, effective_cfg, api_key, &body, |ev| {
        if let StreamEvent::Delta(t) = ev {
            on_delta(t);
        }
    })
    .await
}

/// Background (submit/poll/fetch) text chat.
#[allow(clippy::too_many_arguments)]
pub async fn respond_background_with_effective(
    client: &reqwest::Client,
    effective_cfg: &Value,
    api_key: &str,
    prompt: &str,
    role: &str,
    model: &str,
    poll_interval: std::time::Duration,
    max_polls: usize,
) -> Result<RespondResult, HostError> {
    let body = compose_body(
        effective_cfg,
        prompt,
        role,
        &[],
        &json!({ "model": model, "background": true }),
    )?;
    post_background(
        client,
        effective_cfg,
        api_key,
        &body,
        poll_interval,
        max_polls,
    )
    .await
}

// ---------------------------------------------------------------------------
// Provider + model resolution (shared by both layers).
// ---------------------------------------------------------------------------

/// Resolve `provider` and its model for a text chat via
/// [`crate::models::resolve_model`]: the effective config of the binding that
/// serves the model (per-model `auth`/`request`/`response` overrides merged
/// in), the API key, and the model id.
fn resolve_provider_model(
    provider: &str,
    model_override: Option<&str>,
) -> Result<(Value, String, String), HostError> {
    let resolved = crate::models::resolve_model(provider, model_override, Some("text"))?;
    // The model's / API's own api_key, else the provider's.
    let api_key = config::resolve_api_key(provider, &resolved.effective)?;
    Ok((resolved.effective, api_key, resolved.model))
}

// ---------------------------------------------------------------------------
// Simple text chat with config resolution (the desktop app's entry points).
// ---------------------------------------------------------------------------

/// Load config, resolve the provider, and run a buffered chat (optionally with
/// image parts).
pub async fn respond_text(
    provider: &str,
    prompt: &str,
    role: &str,
    model_override: Option<&str>,
    images: &[ResolvedImage],
) -> Result<RespondResult, HostError> {
    let (eff, api_key, model) = resolve_provider_model(provider, model_override)?;
    let client = reqwest::Client::new();
    respond_with_effective(&client, &eff, &api_key, prompt, role, &model, images)
        .await
        .map_err(|e| explain_account_error(provider, &eff, e))
}

/// Load config, resolve the provider, and run a streaming chat (optionally with
/// image parts).
pub async fn respond_stream_text<F: FnMut(&str)>(
    provider: &str,
    prompt: &str,
    role: &str,
    model_override: Option<&str>,
    images: &[ResolvedImage],
    on_delta: F,
) -> Result<RespondResult, HostError> {
    let (eff, api_key, model) = resolve_provider_model(provider, model_override)?;
    let client = reqwest::Client::new();
    respond_stream_with_effective(
        &client, &eff, &api_key, prompt, role, &model, images, on_delta,
    )
    .await
    .map_err(|e| explain_account_error(provider, &eff, e))
}

/// Load config, resolve the provider, and run a background (submit/poll/fetch)
/// text chat. Polls every second for up to ~10 minutes.
pub async fn respond_background_text(
    provider: &str,
    prompt: &str,
    role: &str,
    model_override: Option<&str>,
) -> Result<RespondResult, HostError> {
    let (eff, api_key, model) = resolve_provider_model(provider, model_override)?;
    let client = reqwest::Client::new();
    respond_background_with_effective(
        &client,
        &eff,
        &api_key,
        prompt,
        role,
        &model,
        std::time::Duration::from_secs(1),
        600,
    )
    .await
}

// ---------------------------------------------------------------------------
// Full chat (the CLI's `respond`).
// ---------------------------------------------------------------------------

/// Everything a full chat can carry beyond the provider name.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    /// The prompt payload: `{prompt, role?, max_tokens?, temperature?, …}`.
    pub prompt: Value,
    /// Model override (an alias or configured id).
    pub model: Option<String>,
    pub images: Vec<ResolvedImage>,
    /// Body overrides (e.g. from a `--params` file).
    pub overrides: Map<String, Value>,
    pub tool_presets: Vec<String>,
    pub set_paths: Vec<(String, Value)>,
    pub unset_paths: Vec<String>,
    /// Compose without validating the body against the schema.
    pub skip_validation: bool,
    /// The output kind asked for (`text`, `image`, …; default `text`): with no
    /// `model`, picks the binding that produces it and its default model.
    pub output: Option<String>,
}

/// How to run a chat. The composed body decides in the end: a body with
/// `stream: true` (e.g. via `--set stream=true`) streams regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatMode {
    Buffered,
    Stream,
    Background,
}

/// A resolved, composed chat ready to send.
#[derive(Debug, Clone)]
pub struct PreparedChat {
    pub provider: String,
    pub model: String,
    /// The API definition serving the model.
    pub api: String,
    pub effective: Value,
    pub body: Value,
    /// The prompt text (names saved artifacts).
    pub prompt_text: String,
    /// Things worth telling the user that don't stop the request (e.g. the
    /// model is deprecated).
    pub warnings: Vec<String>,
    api_key: String,
}

impl PreparedChat {
    /// Whether [`run_chat`] will stream (the body asks for `stream: true`).
    pub fn streams(&self) -> bool {
        self.body.get("stream") == Some(&Value::Bool(true))
    }

    /// What a finished chat reports beyond its text, read from the raw
    /// response through the API definition's `response` block.
    pub fn details(&self, res: &RespondResult) -> ChatDetails {
        let resp = self.effective.get("response");
        let mut metadata = Map::new();
        if let Some(paths) = resp
            .and_then(|r| r.get("metadata"))
            .and_then(Value::as_object)
        {
            for (k, p) in paths {
                if let Some(v) = p.as_str().and_then(|p| lexi_core::get_path(&res.raw, p)) {
                    if !v.is_null() {
                        metadata.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        let activity =
            lexi_core::summarize_activity(&res.raw, resp.and_then(|r| r.get("activity")));
        let sources = match resp.and_then(|r| r.get("annotations")) {
            Some(ann_cfg) => {
                let items =
                    lexi_core::find_text_items(&res.raw, resp.and_then(|r| r.get("text_extract")));
                let anns = lexi_core::extract_annotations(&res.raw, Some(ann_cfg), &items);
                lexi_core::render_annotations(&anns, Some(ann_cfg))
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            None => Vec::new(),
        };
        ChatDetails {
            metadata,
            activity,
            sources,
        }
    }

    /// The machine-interface result of a finished chat
    /// (`docs/spec/cli-machine-v1.md` §3): `{provider, model, api, text,
    /// artifacts, metadata, activity?, sources?, raw?}` — `artifacts` are the
    /// files [`crate::generate::save_artifacts`] wrote, `sources` when `cite`,
    /// `raw` when `raw`.
    pub fn result_value(
        &self,
        res: &RespondResult,
        cite: bool,
        raw: bool,
        artifacts: &[crate::generate::SavedArtifact],
    ) -> Value {
        let d = self.details(res);
        let mut out = json!({
            "provider": self.provider,
            "model": self.model,
            "api": self.api,
            "text": res.text,
            "artifacts": artifacts.iter().map(|a| a.to_value()).collect::<Vec<_>>(),
            "metadata": d.metadata,
        });
        if !d.activity.is_empty() {
            out["activity"] = Value::from(d.activity);
        }
        if cite {
            out["sources"] = json!(d.sources);
        }
        if raw {
            out["raw"] = res.raw.clone();
        }
        if !self.warnings.is_empty() {
            out["warnings"] = json!(self.warnings);
        }
        out
    }
}

/// See [`PreparedChat::details`].
#[derive(Debug, Clone, Default)]
pub struct ChatDetails {
    /// `response.metadata` fields the response carried (model, token counts, …).
    pub metadata: Map<String, Value>,
    /// A one-line summary of provider-side work (`response.activity`), or "".
    pub activity: String,
    /// Rendered citations (`response.annotations`), one per entry.
    pub sources: Vec<String>,
}

/// Resolve the provider and model and compose the request body. No network I/O.
pub fn prepare_chat(
    provider: &str,
    req: &ChatRequest,
    mode: ChatMode,
) -> Result<PreparedChat, HostError> {
    let text = req
        .prompt
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("");
    if text.trim().is_empty() {
        return Err(HostError::Config("prompt text is empty".into()));
    }
    let resolved =
        crate::models::resolve_model(provider, req.model.as_deref(), req.output.as_deref())?;
    let (eff, model, api) = (
        resolved.effective.clone(),
        resolved.model.clone(),
        resolved.api.clone(),
    );
    let api_key = config::resolve_api_key(provider, &eff)?;
    // `respond` returns text and/or artifacts; vectors are `embed`'s job.
    let outputs: Vec<&str> = eff
        .pointer("/capabilities/output")
        .and_then(Value::as_array)
        .map(|o| o.iter().filter_map(Value::as_str).collect())
        .unwrap_or_else(|| vec!["text"]);
    if !outputs.iter().any(|o| matches!(*o, "text" | "image")) {
        return Err(HostError::Unsupported(format!(
            "{model} is a kind:{} model (it produces {}); use embed instead",
            resolved.kind.as_deref().unwrap_or("unknown"),
            outputs.join(", ")
        )));
    }
    let what = if api.is_empty() {
        "this provider's API".to_string()
    } else {
        format!("the {api} API")
    };
    match mode {
        ChatMode::Stream if eff.pointer("/response/stream").is_none() => {
            return Err(HostError::Unsupported(format!("{what} doesn't stream")));
        }
        ChatMode::Background if eff.get("operation").is_none() => {
            return Err(HostError::Unsupported(format!(
                "{what} has no background (async) lifecycle"
            )));
        }
        _ => {}
    }
    let mut overrides = req.overrides.clone();
    // The model goes in the body when the API's schema has a `model` field
    // (some APIs put it in the URL instead).
    if eff
        .pointer("/request/body_schema/properties/model")
        .is_some()
        || eff.pointer("/request/body_schema").is_none()
    {
        overrides
            .entry("model")
            .or_insert_with(|| Value::from(model.clone()));
    }
    match mode {
        ChatMode::Buffered => {}
        ChatMode::Stream => {
            overrides.insert("stream".into(), Value::Bool(true));
        }
        ChatMode::Background => {
            overrides.insert("background".into(), Value::Bool(true));
        }
    }
    let body = compose_chat_body(
        &eff,
        &req.prompt,
        &req.images,
        &Value::Object(overrides),
        &req.tool_presets,
        &req.set_paths,
        &req.unset_paths,
        req.skip_validation,
    )?;
    // A default output-token count (from the schema or a saved prompt) above
    // the model's limit is lowered to it; one the caller set explicitly
    // (flag, --set, --params) is checked — and refused — below.
    let mut body = body;
    if let Some(limit) = resolved
        .limits
        .get("max_output_tokens")
        .and_then(Value::as_u64)
    {
        for key in crate::models::OUTPUT_TOKEN_KEYS {
            let explicit =
                req.overrides.contains_key(*key) || req.set_paths.iter().any(|(p, _)| p == key);
            let over = body
                .get(*key)
                .and_then(Value::as_u64)
                .is_some_and(|n| n > limit);
            if over && !explicit {
                body[*key] = Value::from(limit);
            }
        }
    }
    // Check the request against the model's tags and limits.
    let (errors, warnings) = resolved.check_request(&body, !req.images.is_empty());
    if !req.skip_validation && !errors.is_empty() {
        return Err(HostError::Unsupported(errors.join("\n")));
    }
    Ok(PreparedChat {
        provider: provider.to_string(),
        model,
        api,
        effective: eff,
        body,
        prompt_text: text.to_string(),
        warnings,
        api_key,
    })
}

/// Send a prepared chat. Streams when the body asks for it (events go to
/// `on_event`), runs the background lifecycle when it asks for that, and is
/// buffered otherwise.
pub async fn run_chat<F: FnMut(StreamEvent)>(
    chat: &PreparedChat,
    on_event: F,
) -> Result<RespondResult, HostError> {
    run_chat_unexplained(chat, on_event)
        .await
        .map_err(|e| explain_account_error(&chat.provider, &chat.effective, e))
}

async fn run_chat_unexplained<F: FnMut(StreamEvent)>(
    chat: &PreparedChat,
    on_event: F,
) -> Result<RespondResult, HostError> {
    let client = reqwest::Client::new();
    if chat.streams() {
        post_stream(
            &client,
            &chat.effective,
            &chat.api_key,
            &chat.body,
            on_event,
        )
        .await
    } else if chat.body.get("background") == Some(&Value::Bool(true)) {
        post_background(
            &client,
            &chat.effective,
            &chat.api_key,
            &chat.body,
            std::time::Duration::from_secs(1),
            600,
        )
        .await
    } else {
        post_buffered(&client, &chat.effective, &chat.api_key, &chat.body).await
    }
}

// ---------------------------------------------------------------------------
// Detached background jobs (`respond --background --detach`, `jobs`).
// ---------------------------------------------------------------------------

/// Submit a prepared background chat without waiting; returns the operation
/// (its `id` is what [`operation_status`] / [`wait_operation`] take).
pub async fn submit_chat(chat: &PreparedChat) -> Result<OperationStatus, HostError> {
    let client = reqwest::Client::new();
    submit_background(&client, &chat.effective, &chat.api_key, &chat.body)
        .await
        .map_err(|e| explain_account_error(&chat.provider, &chat.effective, e))
}

fn resolve_provider(provider: &str) -> Result<(Value, String), HostError> {
    let providers = config::load_providers()?;
    let provider_cfg = providers
        .get("providers")
        .and_then(|p| p.get(provider))
        .cloned()
        .ok_or_else(|| HostError::UnknownProvider(provider.to_string()))?;
    let eff = config::effective_config(provider, &provider_cfg)?;
    let api_key = config::resolve_api_key(provider, &eff)?;
    Ok((eff, api_key))
}

/// Check a background operation once.
pub async fn operation_status(provider: &str, id: &str) -> Result<OperationStatus, HostError> {
    let (eff, api_key) = resolve_provider(provider)?;
    let client = reqwest::Client::new();
    poll_operation(&client, &eff, &api_key, &Value::from(id)).await
}

/// Poll a background operation until it finishes (a failure is an error).
pub async fn wait_operation(
    provider: &str,
    id: &str,
    poll_interval: std::time::Duration,
    max_polls: usize,
) -> Result<OperationStatus, HostError> {
    let (eff, api_key) = resolve_provider(provider)?;
    let client = reqwest::Client::new();
    let first = poll_operation(&client, &eff, &api_key, &Value::from(id)).await?;
    wait_for(&client, &eff, &api_key, first, poll_interval, max_polls).await
}
