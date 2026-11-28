//! Shared helpers for the `lexi` binary tests.
#![allow(dead_code)]

use std::path::Path;

use assert_cmd::Command;

/// The `lexi` binary, pointed at `home` as its `LEXI_HOME`, keeping saved
/// keys in files there (never the real keychain).
pub fn lexi(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lexi").unwrap();
    cmd.env("LEXI_HOME", home).env("LEXI_KEY_STORE", "file");
    cmd
}

/// A fresh home seeded with the shipped defaults (by running `lexi version`).
pub fn seeded_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    lexi(dir.path()).arg("version").assert().success();
    dir
}

/// A home whose only provider, `mock`, speaks the Messages API at `url`.
pub fn home_with_mock_provider(url: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("providers/providers-config.yaml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg,
        format!(
            "providers:\n  mock:\n    url: {url}\n    api_key: test-key\n    api: messages\n    default_model: claude-x\n"
        ),
    )
    .unwrap();
    dir
}

/// Run the (blocking) binary off the async test runtime.
pub async fn run(cmd: Command) -> std::process::Output {
    let mut cmd = cmd;
    tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap()
}

pub fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Run `lexi <args>` against `home` synchronously.
pub fn lexi_out(home: &Path, args: &[&str]) -> std::process::Output {
    lexi(home).args(args).output().unwrap()
}
