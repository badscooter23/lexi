//! Where saved keys are kept: `key_store` / `LEXI_KEY_STORE`, and (on macOS,
//! with `LEXI_TEST_KEYCHAIN` set) the keychain end to end against a
//! throw-away keychain.

mod common;

use common::{lexi, lexi_out, seeded_home, stderr, stdout};
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

#[cfg(target_os = "macos")]
mod keychain {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SECURITY: &str = "/usr/bin/security";

    fn security(args: &[&str]) -> std::process::Output {
        Command::new(SECURITY).args(args).output().unwrap()
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

    /// A keychain in a temp dir, unlocked, left out of the search list
    /// (creating it adds it; the list is put back), deleted afterwards.
    struct TestKeychain {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    impl TestKeychain {
        fn new() -> TestKeychain {
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
            TestKeychain { _dir: dir, path }
        }

        fn has(&self, account: &str) -> bool {
            security(&[
                "find-generic-password",
                "-s",
                "Lexi",
                "-a",
                account,
                self.path.to_str().unwrap(),
            ])
            .status
            .success()
        }
    }

    impl Drop for TestKeychain {
        fn drop(&mut self) {
            let _ = security(&["delete-keychain", self.path.to_str().unwrap()]);
        }
    }

    fn account(provider: &str, home: &Path) -> String {
        format!("{provider}@{}", home.canonicalize().unwrap().display())
    }

    #[tokio::test]
    async fn saved_keys_live_in_the_keychain() {
        if std::env::var_os("LEXI_TEST_KEYCHAIN")
            .filter(|v| !v.is_empty())
            .is_none()
        {
            eprintln!("skipped: set LEXI_TEST_KEYCHAIN=1 to test against a throw-away keychain");
            return;
        }
        let kc = TestKeychain::new();
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
        let in_kc = |args: &[&str]| {
            let mut cmd = lexi(&h);
            cmd.env("LEXI_KEY_STORE", "system")
                .env("LEXI_KEYCHAIN", &kc.path)
                .args(args);
            cmd
        };

        // The first run moves the key file in, and says so once.
        let first = in_kc(&["version"]).output().unwrap();
        assert!(
            stderr(&first).contains("Moved p's API key into the macOS Keychain."),
            "{}",
            stderr(&first)
        );
        assert!(!key_file.exists());
        assert!(kc.has(&account("p", &h)));
        assert!(!stderr(&in_kc(&["version"]).output().unwrap()).contains("Moved"));

        let status = common::run(in_kc(&["providers", "status", "p"])).await;
        assert_eq!(stdout(&status), "p: ready (key from the macOS Keychain)\n");
        let info = json(
            &in_kc(&["--json", "providers", "api-key", "show", "p"])
                .output()
                .unwrap(),
        );
        assert_eq!(info["store"]["kind"], "keychain");
        assert_eq!(info["save_to"], "the macOS Keychain");
        assert_eq!(info["sources"][0]["from"], "the macOS Keychain");
        assert_eq!(info["sources"][0]["state"], "used");

        // Saving replaces it there; no file appears.
        let set = in_kc(&["providers", "api-key", "set", "p", "--value", "sk-new"])
            .output()
            .unwrap();
        assert_eq!(
            stdout(&set),
            "Saved the API key for p in the macOS Keychain.\n",
            "{}",
            stderr(&set)
        );
        assert!(!key_file.exists());
        let status = common::run(in_kc(&["providers", "status", "p"])).await;
        assert!(
            stdout(&status).starts_with("p: ready"),
            "{}",
            stderr(&status)
        );

        // Rename takes the key along; remove deletes it.
        assert!(in_kc(&["providers", "rename", "p", "q"])
            .output()
            .unwrap()
            .status
            .success());
        assert!(!kc.has(&account("p", &h)));
        let shown = json(
            &in_kc(&["--json", "providers", "api-key", "show", "q", "--reveal"])
                .output()
                .unwrap(),
        );
        assert_eq!(shown["key"], "sk-new");
        assert!(in_kc(&["providers", "rm", "q"])
            .output()
            .unwrap()
            .status
            .success());
        assert!(!kc.has(&account("q", &h)));
    }
}
