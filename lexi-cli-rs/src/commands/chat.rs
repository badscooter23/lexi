//! Media and misc commands: `generate`, `upload`, `version`.

use serde_json::json;

use crate::output::Io;

/// `generate` — kept as a shortcut for `respond <provider> TEXT --output image`
/// (same pipeline, same result).
pub(crate) async fn generate(
    io: &mut Io<'_>,
    provider: String,
    label: String,
    prompt: String,
    model: Option<String>,
    out: std::path::PathBuf,
) -> u8 {
    let mut args = vec![
        "--text".to_string(),
        prompt,
        "--output".into(),
        "image".into(),
        "--out".into(),
        out.display().to_string(),
    ];
    if let Some(m) = model {
        args.extend(["--model".into(), m]);
    }
    super::respond::respond(io, &provider, &label, &args).await
}

/// Upload a file (with the key the caller chose: file ids belong to an
/// account, so later requests that attach it should use the same key).
pub(crate) async fn upload(
    io: &mut Io<'_>,
    provider: String,
    label: String,
    file: std::path::PathBuf,
) -> u8 {
    match lexi_host::upload_file(&provider, &file).await {
        Ok(id) => io.result(&id, json!({ "provider": label, "file_id": id })),
        Err(e) => io.fail(e),
    }
}

pub(crate) fn version(io: &mut Io<'_>) -> u8 {
    let version = env!("CARGO_PKG_VERSION");
    io.result(
        format!("lexi {version} (rust)"),
        json!({ "version": version, "implementation": "rust" }),
    )
}
