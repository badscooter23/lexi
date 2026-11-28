//! Python bindings for the `lexi-host` runtime: send prompts (buffered,
//! streaming, background), detached jobs, image generation and uploads.
//!
//! These are the private `lexi._lexi._*` functions; the public, documented
//! signatures are the Python wrappers in `python/lexi/`. Each call:
//! - seeds `~/.lexi-cli` (or `$LEXI_HOME`) with missing defaults, like the CLI;
//! - runs on one shared tokio runtime with the GIL released, checking for
//!   Ctrl-C while it waits;
//! - returns the CLI's `--json` shapes (`docs/spec/cli-machine-v1.md`), built by
//!   the same `lexi-host` code, and raises `lexi.LexiError` on failure.

use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use lexi_host::{ChatMode, ChatRequest, HostError, PreparedChat, StreamEvent};
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{from_py, to_py};

pyo3::create_exception!(
    lexi,
    LexiError,
    PyException,
    "A Lexi operation failed (unknown provider, missing API key, provider error, …)."
);

fn lexi_err(e: impl std::fmt::Display) -> PyErr {
    LexiError::new_err(e.to_string())
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("lexi-runtime")
            .build()
            .expect("failed to start the lexi runtime")
    })
}

/// How often a blocked call wakes to check for Ctrl-C.
const SIGNAL_CHECK: Duration = Duration::from_millis(100);

/// Run `fut` on the shared runtime and wait for it without holding the GIL.
/// A pending Python signal (Ctrl-C) cancels the task and is raised.
fn block_on<T, F>(py: Python<'_>, fut: F) -> PyResult<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T, HostError>> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let task = runtime().spawn(async move {
        let _ = tx.send(fut.await);
    });
    let mut rx = rx;
    loop {
        // Receiver is Send but not Sync: move it through the GIL-free closure.
        let (got, back) = py.allow_threads(move || (rx.recv_timeout(SIGNAL_CHECK), rx));
        rx = back;
        match got {
            Ok(res) => return res.map_err(lexi_err),
            Err(RecvTimeoutError::Timeout) => {
                if let Err(e) = py.check_signals() {
                    task.abort();
                    return Err(e);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(lexi_err("the lexi runtime task ended unexpectedly"))
            }
        }
    }
}

fn ensure_home() -> PyResult<()> {
    lexi_host::ensure_home().map(|_| ()).map_err(lexi_err)
}

// --- chat options -----------------------------------------------------------

/// The keyword options shared by `respond` / `stream` / `submit` / `prepare`
/// (the Python wrappers drop `None`s before passing them here).
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ChatOpts {
    text: Option<String>,
    prompt: Option<String>,
    role: Option<String>,
    model: Option<String>,
    images: Vec<String>,
    params: Map<String, Value>,
    tools: Vec<String>,
    set: Value,
    unset: Vec<String>,
    validate: Option<bool>,
    cite: bool,
    raw: bool,
    output: Option<String>,
    out_dir: Option<String>,
    inputs: Vec<String>,
    /// This operation's time limit in seconds (0 = none).
    timeout: Option<u64>,
}

/// Run `fut` with `timeout` as its time limit for provider requests, if given.
async fn timed<F: Future>(timeout: Option<u64>, fut: F) -> F::Output {
    match timeout {
        Some(secs) => lexi_host::with_request_timeout(secs, fut).await,
        None => fut.await,
    }
}

/// `set` as `{path: value}` or `[(path, value), …]`.
fn set_paths(v: &Value) -> PyResult<Vec<(String, Value)>> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Object(m) => Ok(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        Value::Array(pairs) => pairs
            .iter()
            .map(|p| match p.as_array().map(Vec::as_slice) {
                Some([Value::String(k), v]) => Ok((k.clone(), v.clone())),
                _ => Err(PyValueError::new_err(
                    "set: expected a dict or (path, value) pairs",
                )),
            })
            .collect(),
        _ => Err(PyValueError::new_err(
            "set: expected a dict or (path, value) pairs",
        )),
    }
}

fn parse_opts(opts: &Bound<'_, PyAny>) -> PyResult<ChatOpts> {
    serde_json::from_value(from_py(opts)?).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Resolve the prompt and compose the chat (no network I/O).
fn prepare(provider: &str, opts: &ChatOpts, mode: ChatMode) -> PyResult<PreparedChat> {
    ensure_home()?;
    let mut payload = match (&opts.text, &opts.prompt) {
        (Some(_), Some(_)) => {
            return Err(PyValueError::new_err(
                "pass either text or prompt (a saved prompt's name), not both",
            ))
        }
        (Some(t), None) => json!({ "prompt": t, "role": "user" }),
        (None, name) => lexi_host::config::resolve_prompt(name.as_deref()).map_err(lexi_err)?,
    };
    if let Some(role) = &opts.role {
        payload["role"] = Value::from(role.as_str());
    }
    let req = ChatRequest {
        prompt: payload,
        model: opts.model.clone(),
        images: lexi_host::resolve_images(&opts.images).map_err(lexi_err)?,
        overrides: opts.params.clone(),
        tool_presets: opts.tools.clone(),
        set_paths: set_paths(&opts.set)?,
        unset_paths: opts.unset.clone(),
        skip_validation: !opts.validate.unwrap_or(true),
        output: opts.output.clone(),
    };
    lexi_host::prepare_chat(provider, &req, mode).map_err(lexi_err)
}

fn mode_from(name: &str) -> PyResult<ChatMode> {
    match name {
        "buffered" => Ok(ChatMode::Buffered),
        "stream" => Ok(ChatMode::Stream),
        "background" => Ok(ChatMode::Background),
        other => Err(PyValueError::new_err(format!(
            "unknown chat mode {other:?}"
        ))),
    }
}

// --- chat -------------------------------------------------------------------

#[pyfunction]
fn _prepare<'py>(
    py: Python<'py>,
    provider: &str,
    opts: &Bound<'py, PyAny>,
    mode: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let chat = prepare(provider, &parse_opts(opts)?, mode_from(mode)?)?;
    to_py(
        py,
        &json!({ "provider": chat.provider, "model": chat.model, "body": chat.body }),
    )
}

#[pyfunction]
fn _respond<'py>(
    py: Python<'py>,
    provider: &str,
    opts: &Bound<'py, PyAny>,
    background: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let opts = parse_opts(opts)?;
    let mode = if background {
        ChatMode::Background
    } else {
        ChatMode::Buffered
    };
    let chat = prepare(provider, &opts, mode)?;
    let (cite, raw) = (opts.cite, opts.raw);
    let out_dir = PathBuf::from(opts.out_dir.as_deref().unwrap_or("."));
    let out = block_on(
        py,
        timed(opts.timeout, async move {
            let res = lexi_host::run_chat(&chat, |_| {}).await?;
            let saved = lexi_host::generate::save_artifacts(&chat, &res, &out_dir).await?;
            Ok(chat.result_value(&res, cite, raw, &saved))
        }),
    )?;
    to_py(py, &out)
}

#[pyfunction]
fn _submit<'py>(
    py: Python<'py>,
    provider: &str,
    opts: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let chat = prepare(provider, &parse_opts(opts)?, ChatMode::Background)?;
    let out = block_on(py, async move {
        let st = lexi_host::submit_chat(&chat).await?;
        Ok(st.to_value(&chat.provider))
    })?;
    to_py(py, &out)
}

enum Event {
    Delta(String),
    Activity(String),
    Artifact(Value),
    Done(Value),
    Failed(String),
}

/// An iterator over a streamed response's events — `activity`, `delta`, then
/// `provider_done` (the NDJSON events of `lexi respond --stream --json`).
/// A failure raises `LexiError` instead of a `provider_done` with `error`.
#[pyclass(module = "lexi")]
struct ChatStream {
    provider: String,
    rx: Mutex<Option<Receiver<Event>>>,
    started: bool,
    task: tokio::task::AbortHandle,
}

impl Drop for ChatStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[pymethods]
impl ChatStream {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        if !self.started {
            self.started = true;
            return to_py(
                py,
                &json!({ "event": "provider_start", "provider": self.provider }),
            )
            .map(Some);
        }
        let mut guard = self.rx.lock().unwrap();
        let Some(mut rx) = guard.take() else {
            return Ok(None);
        };
        let event = loop {
            let (got, back) = py.allow_threads(move || (rx.recv_timeout(SIGNAL_CHECK), rx));
            rx = back;
            match got {
                Ok(ev) => break ev,
                Err(RecvTimeoutError::Timeout) => {
                    if let Err(e) = py.check_signals() {
                        self.task.abort();
                        return Err(e);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(lexi_err("the stream ended unexpectedly"));
                }
            }
        };
        let p = &self.provider;
        let value = match event {
            Event::Delta(text) => json!({ "event": "delta", "provider": p, "text": text }),
            Event::Activity(msg) => json!({ "event": "activity", "provider": p, "message": msg }),
            Event::Artifact(mut a) => {
                a["event"] = Value::from("artifact");
                a["provider"] = Value::from(p.as_str());
                a
            }
            Event::Done(mut done) => {
                done["event"] = Value::from("provider_done");
                done
            }
            Event::Failed(e) => return Err(lexi_err(e)),
        };
        if matches!(
            value["event"].as_str(),
            Some("delta" | "activity" | "artifact")
        ) {
            *guard = Some(rx); // more to come
        }
        to_py(py, &value).map(Some)
    }

    /// Stop the stream (also happens when the iterator is garbage-collected).
    fn close(&mut self) {
        self.task.abort();
        *self.rx.lock().unwrap() = None;
    }
}

#[pyfunction]
fn _stream(provider: &str, opts: &Bound<'_, PyAny>) -> PyResult<ChatStream> {
    let opts = parse_opts(opts)?;
    let chat = prepare(provider, &opts, ChatMode::Stream)?;
    let (cite, raw) = (opts.cite, opts.raw);
    let out_dir = PathBuf::from(opts.out_dir.as_deref().unwrap_or("."));
    let (tx, rx) = mpsc::channel();
    let timeout = opts.timeout;
    let task = runtime().spawn(timed(timeout, async move {
        let res = lexi_host::run_chat(&chat, |ev| {
            let _ = tx.send(match ev {
                StreamEvent::Delta(t) => Event::Delta(t.to_string()),
                StreamEvent::Activity("") => return,
                StreamEvent::Activity(l) => Event::Activity(l.to_string()),
            });
        })
        .await;
        let res = match res {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Event::Failed(e.to_string()));
                return;
            }
        };
        match lexi_host::generate::save_artifacts(&chat, &res, &out_dir).await {
            Ok(saved) => {
                for a in &saved {
                    let _ = tx.send(Event::Artifact(a.to_value()));
                }
                let _ = tx.send(Event::Done(chat.result_value(&res, cite, raw, &saved)));
            }
            Err(e) => {
                let _ = tx.send(Event::Failed(e.to_string()));
            }
        }
    }));
    Ok(ChatStream {
        provider: provider.to_string(),
        rx: Mutex::new(Some(rx)),
        started: false,
        task: task.abort_handle(),
    })
}

// --- jobs -------------------------------------------------------------------

#[pyfunction]
fn _job_status<'py>(py: Python<'py>, provider: String, id: String) -> PyResult<Bound<'py, PyAny>> {
    ensure_home()?;
    let out = block_on(py, async move {
        let st = lexi_host::operation_status(&provider, &id).await?;
        Ok(st.to_value(&provider))
    })?;
    to_py(py, &out)
}

#[pyfunction]
fn _job_wait<'py>(
    py: Python<'py>,
    provider: String,
    id: String,
    interval: f64,
    timeout: f64,
) -> PyResult<Bound<'py, PyAny>> {
    ensure_home()?;
    if !interval.is_finite() || interval <= 0.0 || !timeout.is_finite() || timeout < 0.0 {
        return Err(PyValueError::new_err(
            "interval must be > 0 and timeout >= 0",
        ));
    }
    let polls = ((timeout / interval).ceil() as usize).max(1);
    let out = block_on(py, async move {
        let st =
            lexi_host::wait_operation(&provider, &id, Duration::from_secs_f64(interval), polls)
                .await?;
        Ok(st.to_value(&provider))
    })?;
    to_py(py, &out)
}

// --- media ------------------------------------------------------------------

#[pyfunction]
fn _embed<'py>(
    py: Python<'py>,
    provider: String,
    opts: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    ensure_home()?;
    let o = parse_opts(opts)?;
    let req = lexi_host::EmbedRequest {
        inputs: o.inputs.clone(),
        model: o.model.clone(),
        overrides: o.params.clone(),
        set_paths: set_paths(&o.set)?,
        unset_paths: o.unset.clone(),
        skip_validation: !o.validate.unwrap_or(true),
    };
    let out = block_on(
        py,
        timed(
            o.timeout,
            async move { lexi_host::embed(&provider, &req).await },
        ),
    )?;
    to_py(py, &out)
}

#[pyfunction]
fn _upload<'py>(py: Python<'py>, provider: String, path: PathBuf) -> PyResult<Bound<'py, PyAny>> {
    ensure_home()?;
    let file_id = block_on(py, {
        let provider = provider.clone();
        async move { lexi_host::upload_file(&provider, &path).await }
    })?;
    to_py(py, &json!({ "provider": provider, "file_id": file_id }))
}

// --- admin --------------------------------------------------------------------

/// Typed access to an admin call's keyword arguments.
struct Args(Map<String, Value>);

impl Args {
    fn opt_str(&self, k: &str) -> Option<&str> {
        self.0.get(k).and_then(Value::as_str)
    }
    fn str(&self, k: &str) -> PyResult<&str> {
        self.opt_str(k)
            .ok_or_else(|| PyValueError::new_err(format!("{k} is required")))
    }
    fn string(&self, k: &str) -> PyResult<String> {
        self.str(k).map(str::to_string)
    }
    fn flag(&self, k: &str) -> bool {
        self.0.get(k).and_then(Value::as_bool).unwrap_or(false)
    }
    fn map(&self, k: &str) -> Map<String, Value> {
        self.0
            .get(k)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }
}

fn require_provider(name: &str) -> Result<Value, HostError> {
    lexi_host::admin::provider_config(name)
}

/// The admin operations, by `group.verb` (the Python wrappers in
/// `python/lexi/{providers,models,prompts,config,apis}.py`). Results are the
/// CLI's `--json` shapes.
#[pyfunction]
fn _admin<'py>(py: Python<'py>, op: &str, args: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    use lexi_host::{admin, config as cfg};
    ensure_home()?;
    let a = match from_py(args)? {
        Value::Object(m) => Args(m),
        Value::Null => Args(Map::new()),
        _ => return Err(PyValueError::new_err("admin arguments must be a dict")),
    };
    let e = lexi_err;
    let out: Value = match op {
        // providers
        "providers.list" => json!(admin::provider_summaries().map_err(e)?),
        "providers.status" => {
            let name = a.string("name")?;
            block_on(py, async move {
                let st = lexi_host::check_api_key(&name).await?;
                Ok(
                    json!({ "provider": name, "status": st.status, "source": st.source, "from": st.from }),
                )
            })?
        }
        "providers.add" => {
            let name = a.str("name")?;
            admin::create_provider(
                name,
                a.str("url")?,
                a.str("default_model")?,
                a.opt_str("api_key"),
                a.opt_str("api").unwrap_or("chat-completions"),
                a.opt_str("hosted_models").unwrap_or("/models"),
            )
            .map_err(e)?;
            json!({ "added": name })
        }
        "providers.rename" => {
            let (old, new) = (a.str("old")?, a.str("new")?);
            cfg::rename_provider(old, new).map_err(e)?;
            json!({ "renamed": old, "to": new })
        }
        "providers.remove" => {
            let name = a.str("name")?;
            cfg::remove_provider(name).map_err(e)?;
            json!({ "removed": name })
        }
        "providers.set_key" => {
            let name = a.str("name")?;
            require_provider(name).map_err(e)?;
            let mut saved = cfg::set_api_key(name, a.opt_str("api"), a.str("key")?)
                .map_err(e)?
                .to_value();
            saved["provider"] = json!(name);
            saved
        }
        "providers.show_key" => {
            admin::api_key_info(a.str("name")?, a.opt_str("api"), a.flag("reveal")).map_err(e)?
        }
        "providers.key_sources" => {
            cfg::key_source_list(a.str("name")?, a.opt_str("api")).map_err(e)?
        }
        "providers.add_key_source" => {
            let source = a.0.get("source").cloned().unwrap_or(Value::Null);
            cfg::add_key_source(a.str("name")?, a.opt_str("api"), &source, a.flag("first"))
                .map_err(e)?
        }
        "providers.remove_key_source" => {
            let name = a.str("name")?;
            let source = a.0.get("source").cloned().unwrap_or(Value::Null);
            let removed = cfg::remove_key_source(name, a.opt_str("api"), &source).map_err(e)?;
            json!({ "provider": name, "removed": removed })
        }
        "providers.upgrade" => {
            cfg::upgrade_provider(a.str("name")?, a.opt_str("template"), a.flag("apply"))
                .map_err(e)?
        }
        "providers.apis" => json!(cfg::provider_apis(a.str("name")?).map_err(e)?),
        "providers.add_api" => {
            let name = a.str("name")?;
            let mut b = Map::new();
            b.insert("api".into(), Value::from(a.str("api")?));
            for (k, key) in [
                ("for", "for"),
                ("default_model", "default_model"),
                ("url", "url"),
            ] {
                if let Some(v) = a.0.get(k).filter(|v| !v.is_null()) {
                    b.insert(key.into(), v.clone());
                }
            }
            let existed = cfg::add_provider_api(name, &b).map_err(e)?;
            json!({ "provider": name, "api": b["api"], "existed": existed })
        }
        "providers.remove_api" => {
            let (name, api) = (a.str("name")?, a.str("api")?);
            cfg::remove_provider_api(name, api).map_err(e)?;
            json!({ "provider": name, "removed": api })
        }
        "providers.remove_key" => {
            let name = a.str("name")?;
            require_provider(name).map_err(e)?;
            json!({ "removed": cfg::remove_api_key(name, a.opt_str("api")).map_err(e)? })
        }

        // models
        "models.list" => {
            let p = a.str("provider")?;
            require_provider(p).map_err(e)?;
            json!(lexi_host::models::model_summaries(p).map_err(e)?)
        }
        "models.tag" => {
            let (p, m) = (a.str("provider")?, a.str("model")?);
            if a.flag("clear") {
                let had = lexi_host::models::clear_model_tags(p, m).map_err(e)?;
                json!({ "provider": p, "model": m, "cleared": had })
            } else {
                let ops: Vec<String> =
                    a.0.get("ops")
                        .and_then(Value::as_array)
                        .map(|v| {
                            v.iter()
                                .filter_map(Value::as_str)
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default();
                let now = lexi_host::models::tag_model(p, m, &ops).map_err(e)?;
                json!({ "provider": p, "model": m, "tags": now })
            }
        }
        "models.tags" => {
            lexi_host::models::classify(a.str("provider")?, a.str("model")?).map_err(e)?
        }
        "models.resolve" => {
            let r = lexi_host::models::resolve_model(
                a.str("provider")?,
                a.opt_str("model"),
                a.opt_str("output"),
            )
            .map_err(e)?;
            json!({ "provider": r.provider, "model": r.model, "api": r.api, "kind": r.kind, "tags": r.tags })
        }
        "tags.vocabulary" => lexi_host::models::tag_vocabulary(),
        "models.hosted" => {
            let p = a.string("provider")?;
            let cached = a.flag("cached");
            let tags: Vec<String> =
                a.0.get("tags")
                    .and_then(Value::as_array)
                    .map(|v| {
                        v.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
            Value::Array(block_on(py, async move {
                lexi_host::models::hosted_summaries(&p, cached, &tags).await
            })?)
        }
        "models.default" => {
            lexi_host::models::defaults(a.str("provider")?, a.opt_str("kind")).map_err(e)?
        }
        "models.set_default" => {
            let (p, kind) = (a.str("provider")?, a.opt_str("kind"));
            match a.opt_str("model") {
                Some(m) => lexi_host::models::set_default(p, m, kind).map_err(e)?,
                None => lexi_host::models::clear_default(p, kind.unwrap_or("chat")).map_err(e)?,
            }
        }
        "models.clear_default" => {
            lexi_host::models::clear_default(a.str("provider")?, a.str("kind")?).map_err(e)?
        }
        "models.add" => {
            let (p, m) = (a.str("provider")?, a.str("model")?);
            require_provider(p).map_err(e)?;
            let existed =
                cfg::upsert_model(p, m, &a.map("fields"), a.flag("default")).map_err(e)?;
            json!({ "provider": p, "model": m, "existed": existed })
        }
        "models.remove" => {
            let (p, m) = (a.str("provider")?, a.str("model")?);
            require_provider(p).map_err(e)?;
            cfg::delete_model(p, m).map_err(e)?;
            json!({ "provider": p, "removed": m })
        }
        "models.aliases" => {
            let p = a.str("provider")?;
            require_provider(p).map_err(e)?;
            let mut aliases: Vec<_> = cfg::model_aliases(p).map_err(e)?.into_iter().collect();
            aliases.sort_by(|x, y| x.0.cmp(&y.0));
            Value::Array(
                aliases
                    .into_iter()
                    .map(|(alias, model)| json!({ "alias": alias, "model": model }))
                    .collect(),
            )
        }
        "models.set_alias" => {
            let (p, al, m) = (a.str("provider")?, a.str("alias")?, a.str("model")?);
            require_provider(p).map_err(e)?;
            cfg::set_model_alias(p, al, m).map_err(e)?;
            json!({ "provider": p, "alias": al, "model": m })
        }
        "models.remove_alias" => {
            let (p, al) = (a.str("provider")?, a.str("alias")?);
            require_provider(p).map_err(e)?;
            cfg::delete_model_alias(p, al).map_err(e)?;
            json!({ "provider": p, "removed": al })
        }

        // prompts
        "prompts.list" => json!(admin::prompt_summaries().map_err(e)?),
        "prompts.get" => {
            let name = a.str("name")?;
            admin::prompt_summaries()
                .map_err(e)?
                .into_iter()
                .find(|p| p["name"] == name)
                .ok_or_else(|| lexi_err(format!("prompt {name:?} not found")))?
        }
        "prompts.save" => admin::update_prompt(a.str("name")?, &a.map("fields")).map_err(e)?,
        "prompts.remove" => {
            let name = a.str("name")?;
            cfg::delete_prompt(name).map_err(e)?;
            json!({ "removed": name })
        }
        "prompts.activate" => {
            let name = a.str("name")?;
            cfg::set_active_prompt(name).map_err(e)?;
            json!({ "active": name })
        }

        // config (app settings)
        "config.list" => Value::Object(admin::settings().map_err(e)?),
        "config.get" => {
            let key = a.str("key")?;
            let v = admin::settings()
                .map_err(e)?
                .get(key)
                .cloned()
                .ok_or_else(|| lexi_err(format!("config key {key:?} not found")))?;
            json!({ "key": key, "value": v })
        }
        "config.set" => {
            let key = a.str("key")?;
            let value = a.0.get("value").cloned().unwrap_or(Value::Null);
            let mut update = Map::new();
            update.insert(key.to_string(), value.clone());
            cfg::set_config(&update).map_err(e)?;
            json!({ "key": key, "value": value })
        }
        "config.remove" => {
            let key = a.str("key")?;
            if !cfg::delete_config(key).map_err(e)? {
                return Err(lexi_err(format!("config key {key:?} not found")));
            }
            json!({ "removed": key })
        }

        // API definitions
        "apis.list" => json!(admin::api_summaries().map_err(e)?),
        "apis.show" => cfg::load_api_def(a.str("name")?).map_err(e)?,
        "apis.validate" => {
            let name = a.str("name")?;
            let (valid, errors) = cfg::validate_api_def(name).map_err(e)?;
            json!({ "name": name, "valid": valid, "errors": errors })
        }
        "apis.preview" => cfg::preview_api_def(
            a.opt_str("name"),
            a.opt_str("yaml"),
            a.opt_str("prompt").unwrap_or("Sample prompt"),
            a.opt_str("model").unwrap_or("sample-model"),
            a.opt_str("role").unwrap_or("user"),
        )
        .map_err(e)?,
        "apis.create" => {
            let name = a.str("name")?;
            if cfg::load_api_def(name).is_ok() {
                return Err(lexi_err(format!(
                    "API definition {name:?} already exists; use apis.edit to modify it"
                )));
            }
            cfg::create_api_def(
                name,
                a.opt_str("template"),
                a.opt_str("path"),
                a.opt_str("text_path"),
            )
            .map_err(e)?;
            json!({ "created": name })
        }
        "apis.edit" => {
            let name = a.str("name")?;
            let meta: Vec<(String, String)> = a
                .map("metadata")
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect();
            cfg::edit_api_def(name, a.opt_str("path"), a.opt_str("text_path"), &meta).map_err(e)?;
            json!({ "updated": name })
        }
        "apis.save" => {
            let name = a.str("name")?;
            if cfg::embedded_api_names().contains(&name) {
                return Err(lexi_err(format!(
                    "{name:?} is a shipped definition and can't be replaced; save under another name"
                )));
            }
            let errors = cfg::save_api_raw(name, a.str("yaml")?).map_err(e)?;
            json!({ "saved": name, "valid": errors.is_empty(), "errors": errors })
        }
        "apis.remove" => {
            let name = a.str("name")?;
            cfg::delete_api_def(name).map_err(e)?;
            json!({ "removed": name })
        }
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown admin operation {other:?}"
            )))
        }
    };
    to_py(py, &out)
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("LexiError", m.py().get_type_bound::<LexiError>())?;
    m.add_class::<ChatStream>()?;
    m.add_function(wrap_pyfunction!(_prepare, m)?)?;
    m.add_function(wrap_pyfunction!(_respond, m)?)?;
    m.add_function(wrap_pyfunction!(_submit, m)?)?;
    m.add_function(wrap_pyfunction!(_stream, m)?)?;
    m.add_function(wrap_pyfunction!(_job_status, m)?)?;
    m.add_function(wrap_pyfunction!(_job_wait, m)?)?;
    m.add_function(wrap_pyfunction!(_upload, m)?)?;
    m.add_function(wrap_pyfunction!(_embed, m)?)?;
    m.add_function(wrap_pyfunction!(_admin, m)?)?;
    Ok(())
}
