//! Where saved keys are kept: `key_store` / `LEXI_KEY_STORE`, and (with
//! `LEXI_TEST_KEY_STORE` set) the OS key store end to end.

mod common;

use common::{lexi, lexi_out, seeded_home, stderr};
use serde_json::Value;

fn json(o: &std::process::Output) -> Value {
    assert!(o.status.success(), "{}", stderr(o));
    serde_json::from_slice(&o.stdout).unwrap()
}

/// The guard: tests keep saved keys in files under their own home, never the
/// developer's keychain (the harness sets `LEXI_KEY_STORE=file`).
#[test]
fn tests_never_use_the_real_keychain() {
    let home = seeded_home();
    let info = json(&lexi_out(
        home.path(),
        &["--json", "providers", "api-key", "show", "openai"],
    ));
    assert_eq!(info["store"]["kind"], "file", "{info}");
    assert_eq!(info["save_to"], "providers/openai/api-key");
}

#[test]
fn the_key_store_setting_is_checked() {
    let home = seeded_home();
    let h = home.path();
    let o = lexi_out(h, &["config", "set", "key_store", "keychain"]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("key_store is auto, system, file"),
        "{}",
        stderr(&o)
    );
    assert!(lexi_out(h, &["config", "set", "key_store", "File"])
        .status
        .success());
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(h.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg["key_store"], "file");

    let o = lexi(h)
        .env("LEXI_KEY_STORE", "vault")
        .args(["providers", "api-key", "show", "openai"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("LEXI_KEY_STORE is \"vault\": use auto, system, file"),
        "{}",
        stderr(&o)
    );
}

/// The OS store end to end, run only with `LEXI_TEST_KEY_STORE` set (CI):
/// macOS against a throw-away keychain; Windows Credential Manager and the
/// Secret Service (Linux: a private D-Bus session with gnome-keyring) under
/// accounts made unique by the temp home, deleted by the test.
#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
mod os_store {
    use super::*;
    use crate::common::stdout;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[cfg(target_os = "macos")]
    const KIND: &str = "keychain";
    #[cfg(target_os = "macos")]
    const NAME: &str = "the macOS Keychain";
    #[cfg(windows)]
    const KIND: &str = "credential-manager";
    #[cfg(windows)]
    const NAME: &str = "Windows Credential Manager";
    #[cfg(target_os = "linux")]
    const KIND: &str = "secret-service";
    #[cfg(target_os = "linux")]
    const NAME: &str = "your desktop keyring";

    /// macOS: a keychain in a temp dir, unlocked, left out of the search list
    /// (creating it adds it; the list is put back), deleted afterwards.
    #[cfg(target_os = "macos")]
    mod test_store {
        use std::path::{Path, PathBuf};
        use std::process::Command;

        fn security(args: &[&str]) -> std::process::Output {
            Command::new("/usr/bin/security")
                .args(args)
                .output()
                .unwrap()
        }

        fn must(args: &[&str]) -> String {
            let out = security(args);
            assert!(
                out.status.success(),
                "security {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).into_owned()
        }

        pub struct TestStore {
            _dir: tempfile::TempDir,
            path: PathBuf,
        }

        impl TestStore {
            pub fn new() -> TestStore {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("lexi cli test.keychain-db");
                let p = path.to_str().unwrap();
                let before = must(&["list-keychains", "-d", "user"]);
                must(&["create-keychain", "-p", "pw", p]);
                let listed: Vec<String> = before
                    .lines()
                    .map(|l| l.trim().trim_matches('"').to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                let mut restore = vec!["list-keychains", "-d", "user", "-s"];
                restore.extend(listed.iter().map(String::as_str));
                must(&restore);
                must(&["unlock-keychain", "-p", "pw", p]);
                TestStore { _dir: dir, path }
            }

            pub fn env(&self, cmd: &mut assert_cmd::Command) {
                cmd.env("LEXI_KEYCHAIN", &self.path);
            }

            /// Whether `provider`'s item is in the keychain.
            pub fn has(&self, provider: &str, home: &Path) -> Option<bool> {
                let account = format!("{provider}@{}", home.canonicalize().unwrap().display());
                Some(
                    security(&[
                        "find-generic-password",
                        "-s",
                        "Lexi",
                        "-a",
                        &account,
                        self.path.to_str().unwrap(),
                    ])
                    .status
                    .success(),
                )
            }
        }

        impl Drop for TestStore {
            fn drop(&mut self) {
                let _ = security(&["delete-keychain", self.path.to_str().unwrap()]);
            }
        }
    }

    /// Windows and Linux: the real store (the test's accounts are its own).
    #[cfg(not(target_os = "macos"))]
    mod test_store {
        pub struct TestStore;

        impl TestStore {
            pub fn new() -> TestStore {
                TestStore
            }

            pub fn env(&self, _cmd: &mut assert_cmd::Command) {}

            /// Not looked at directly here: the CLI's answers show it.
            pub fn has(&self, _provider: &str, _home: &std::path::Path) -> Option<bool> {
                None
            }
        }
    }

    #[tokio::test]
    async fn saved_keys_live_in_the_os_store() {
        if std::env::var_os("LEXI_TEST_KEY_STORE")
            .filter(|v| !v.is_empty())
            .is_none()
        {
            eprintln!("skipped: set LEXI_TEST_KEY_STORE=1 to test against the OS key store");
            return;
        }
        let store = test_store::TestStore::new();
        let server = MockServer::start().await;
        for key in ["sk-moved", "sk-new"] {
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .and(header("authorization", format!("Bearer {key}").as_str()))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})),
                )
                .mount(&server)
                .await;
        }
        let home = seeded_home();
        let h = home.path().to_path_buf();
        std::fs::write(
            h.join("providers/providers-config.yaml"),
            format!(
                "providers:\n  p:\n    url: {}/v1\n    api: chat-completions\n    api_key: []\n",
                server.uri()
            ),
        )
        .unwrap();
        let key_file = h.join("providers/p/api-key");
        std::fs::create_dir_all(key_file.parent().unwrap()).unwrap();
        std::fs::write(&key_file, "sk-moved\n").unwrap();
        let in_store = |args: &[&str]| {
            let mut cmd = lexi(&h);
            cmd.env("LEXI_KEY_STORE", "system").args(args);
            store.env(&mut cmd);
            cmd
        };

        // The first run moves the key file in, and says so once.
        let first = in_store(&["version"]).output().unwrap();
        assert!(
            stderr(&first).contains(&format!("Moved p's API key into {NAME}.")),
            "{}",
            stderr(&first)
        );
        assert!(!key_file.exists());
        assert_ne!(store.has("p", &h), Some(false));
        assert!(!stderr(&in_store(&["version"]).output().unwrap()).contains("Moved"));

        let status = common::run(in_store(&["providers", "status", "p"])).await;
        assert_eq!(
            stdout(&status),
            format!("p: ready (key from {NAME})\n"),
            "{}",
            stderr(&status)
        );
        let info = json(
            &in_store(&["--json", "providers", "api-key", "show", "p"])
                .output()
                .unwrap(),
        );
        assert_eq!(info["store"]["kind"], KIND);
        assert_eq!(info["save_to"], NAME);
        assert_eq!(info["sources"][0]["from"], NAME);
        assert_eq!(info["sources"][0]["state"], "used");

        // Saving replaces it there; no file appears.
        let set = in_store(&["providers", "api-key", "set", "p", "--value", "sk-new"])
            .output()
            .unwrap();
        assert_eq!(
            stdout(&set),
            format!("Saved the API key for p in {NAME}.\n"),
            "{}",
            stderr(&set)
        );
        assert!(!key_file.exists());
        let status = common::run(in_store(&["providers", "status", "p"])).await;
        assert!(
            stdout(&status).starts_with("p: ready"),
            "{}",
            stderr(&status)
        );

        // Rename takes the key along; remove deletes it.
        assert!(in_store(&["providers", "rename", "p", "q"])
            .output()
            .unwrap()
            .status
            .success());
        assert_ne!(store.has("p", &h), Some(true));
        let shown = json(
            &in_store(&["--json", "providers", "api-key", "show", "q", "--reveal"])
                .output()
                .unwrap(),
        );
        assert_eq!(shown["key"], "sk-new");
        let removed = in_store(&["providers", "rm", "q"]).output().unwrap();
        assert!(removed.status.success(), "{}", stderr(&removed));
        assert_ne!(store.has("q", &h), Some(true));
    }
}
