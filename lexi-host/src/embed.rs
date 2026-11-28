//! Embeddings: texts in, vectors out (`lexi embed`).
//!
//! The model resolves like any other (`resolve_model` with `vector` output):
//! the provider's binding for `kind:embedding` models, e.g. OpenAI's or
//! NVIDIA's `/embeddings`. The engine composes the body (`input` binds the
//! texts) and reads the vectors (`response.embeddings`).

use serde_json::{json, Map, Value};

use crate::respond::endpoint_url;
use crate::{auth, config, HostError};

/// What `embed` sends beyond the provider: the texts plus the usual options.
#[derive(Debug, Clone, Default)]
pub struct EmbedRequest {
    pub inputs: Vec<String>,
    /// A model id or alias; default: the provider's embedding binding default.
    pub model: Option<String>,
    pub overrides: Map<String, Value>,
    pub set_paths: Vec<(String, Value)>,
    pub unset_paths: Vec<String>,
    pub skip_validation: bool,
}

/// Embed `req.inputs`: `{provider, model, api, count, dimensions, embeddings:
/// [[f32…]…], metadata}` — one vector per input, in input order.
pub async fn embed(provider: &str, req: &EmbedRequest) -> Result<Value, HostError> {
    if req.inputs.is_empty() || req.inputs.iter().all(|t| t.trim().is_empty()) {
        return Err(HostError::Invalid(
            "nothing to embed: give one or more texts".into(),
        ));
    }
    let resolved = crate::models::resolve_model(provider, req.model.as_deref(), Some("vector"))?;
    let eff = &resolved.effective;
    let api_key = config::resolve_api_key(provider, eff)?;

    let bindings = json!({ "inputs": req.inputs, "model": resolved.model });
    let mut overrides = req.overrides.clone();
    overrides
        .entry("model")
        .or_insert_with(|| Value::from(resolved.model.clone()));
    let body = lexi_core::compose_request_body(
        eff,
        &bindings,
        &Value::Object(overrides),
        &[],
        &req.set_paths,
        &req.unset_paths,
        req.skip_validation,
    )
    .map_err(HostError::Compose)?;

    let client = reqwest::Client::new();
    let raw: Value = crate::respond::within_limit(async {
        let resp = client
            .post(endpoint_url(eff))
            .headers(auth::build_headers(eff, &api_key))
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let err = HostError::HttpStatus {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            };
            return Err(crate::respond::explain_account_error(provider, eff, err));
        }
        Ok(resp.json().await?)
    })
    .await?;
    let vectors = lexi_core::extract_embeddings(&raw, eff.pointer("/response/embeddings"));
    let mut metadata = Map::new();
    if let Some(paths) = eff.pointer("/response/metadata").and_then(Value::as_object) {
        for (k, p) in paths {
            if let Some(v) = p.as_str().and_then(|p| lexi_core::get_path(&raw, p)) {
                if !v.is_null() {
                    metadata.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let dimensions = vectors
        .first()
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Ok(json!({
        "provider": provider,
        "model": resolved.model,
        "api": resolved.api,
        "count": vectors.len(),
        "dimensions": dimensions,
        "embeddings": vectors,
        "metadata": metadata,
    }))
}
