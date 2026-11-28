//! `lexi-host` — the Lexi runtime (I/O layer) over the pure `lexi-core` engine.
//!
//! `lexi-core` makes every pure decision (compose the request body, extract the
//! response); this crate adds the side effects it deliberately omits: loading
//! providers/keys/config, building auth headers, and HTTP transport. It is the
//! shared runtime beneath the CLI and (later) the Tauri GUI.

pub mod admin;
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod embed;
pub mod generate;
pub mod hosted;
pub mod image;
mod key_store;
mod keystore;
pub mod models;
pub mod respond;
pub mod upload;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HostError {
    #[error("unknown provider {0:?}")]
    UnknownProvider(String),
    #[error("provider {0:?} has no `api` reference (or inline `request` block)")]
    MissingApi(String),
    #[error("unknown API definition {0:?}")]
    UnknownApi(String),
    /// No source in a provider's `api_key` list has a key (the message names
    /// the provider and where Lexi looked).
    #[error("{0}")]
    MissingApiKey(String),
    #[error("config error: {0}")]
    Config(String),
    /// A saved prompt that doesn't exist (the message is the whole error).
    #[error("{0}")]
    Prompt(String),
    #[error("request composition failed: {0}")]
    Compose(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    /// Invalid input, e.g. a tag not in the vocabulary (the message says why).
    #[error("{0}")]
    Invalid(String),
    /// No configured API serves this model / output (the message says why).
    #[error("{0}")]
    Unsupported(String),
    #[error("background operation failed: {0}")]
    OperationFailed(String),
    #[error("background operation did not finish within the poll budget")]
    OperationTimeout,
    /// A provider request ran past `request_timeout` (the message says
    /// whether it was a whole request or a silent stream).
    #[error("{0}")]
    RequestTimeout(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub use bootstrap::{ensure_home, ensure_home_in, take_notices};
pub use config::with_request_timeout;
pub use embed::{embed, EmbedRequest};
pub use generate::{generate_images_text, generate_images_with_effective, SavedArtifact};
pub use hosted::{
    cached_catalog, check_api_key, fetch_hosted_models, fetch_hosted_models_with, KeyStatus,
};

/// A config scalar as query-string text (`1000`, `"abc"` → `abc`).
pub(crate) fn config_scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
pub use image::{resolve_image, resolve_images, ResolvedImage};
pub use models::{resolve_model, ResolvedModel};
pub use respond::{
    compose_chat_body, operation_status, prepare_chat, respond_background_text,
    respond_background_with_effective, respond_stream_text, respond_stream_with_effective,
    respond_text, respond_with_effective, run_chat, submit_chat, wait_operation, ChatDetails,
    ChatMode, ChatRequest, OperationStatus, PreparedChat, RespondResult, StreamEvent,
};
pub use upload::{upload_file, upload_file_with_effective};
