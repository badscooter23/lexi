//! C3 admin writes: config, prompts, providers (+ API keys, status), models,
//! model aliases, API definitions.

mod common;

use common::{lexi, lexi_out, run, seeded_home, stderr, stdout};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok(home: &std::path::Path, args: &[&str]) -> String {
    let o = lexi_out(home, args);
    assert!(o.status.success(), "{args:?} failed: {}", stderr(&o));
    stdout(&o)
}

fn fails(home: &std::path::Path, args: &[&str]) -> String {
    let o = lexi_out(home, args);
    assert_eq!(
        o.status.code(),
        Some(1),
        "{args:?} should fail: {}",
        stdout(&o)
    );
    stderr(&o)
}

fn read_json(home: &std::path::Path, rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(home.join(rel)).unwrap()).unwrap()
}

#[test]
fn config_set_and_rm() {
    let home = seeded_home();
    assert_eq!(
        ok(home.path(), &["config", "set", "theme", "dark", "purple"]),
        "Set theme = dark purple\n"
    );
    assert_eq!(
        read_json(home.path(), "config.json"),
        json!({"theme": "dark purple"})
    );
    assert_eq!(
        ok(home.path(), &["config", "rm", "theme"]),
        "Removed theme\n"
    );
    assert!(fails(home.path(), &["config", "rm", "theme"]).contains("not found"));
    let o = lexi_out(home.path(), &["config", "set", "only-key"]);
    assert_eq!(o.status.code(), Some(2)); // value required: usage error
}

#[test]
fn prompts_set_name_rules_options_and_merge() {
    let home = seeded_home();
    // One bare word: the active prompt's text.
    ok(home.path(), &["prompts", "set", "solo"]);
    assert_eq!(read_json(home.path(), "prompts/$$.json")["prompt"], "solo");

    // Leading word + more: a named prompt; options, --set, and \n escapes.
    assert_eq!(
        ok(
            home.path(),
            &[
                "prompts",
                "set",
                "greet",
                "hi",
                "there\\nfriend",
                "-m",
                "99",
                "-t",
                "0.2",
                "-r",
                "system",
                "--set",
                "reasoning.effort=high"
            ]
        ),
        "Saved prompt 'greet'.\n"
    );
    assert_eq!(
        read_json(home.path(), "prompts/greet.json"),
        json!({"prompt": "hi there\nfriend", "max_tokens": 99, "temperature": 0.2, "role": "system", "reasoning": {"effort": "high"}})
    );

    // Updating merges over the saved prompt; --unset removes a path.
    ok(
        home.path(),
        &["prompts", "set", "greet", "--unset", "reasoning", "-m", "5"],
    );
    let greet = read_json(home.path(), "prompts/greet.json");
    assert_eq!(greet["prompt"], "hi there\nfriend");
    assert_eq!(greet["max_tokens"], 5);
    assert!(greet.get("reasoning").is_none());

    assert!(fails(home.path(), &["prompts", "set", "empty", "-m", "5"])
        .contains("requires prompt text"));
    assert!(fails(home.path(), &["prompts", "set", "x", "y", "-m", "lots"]).contains("integer"));

    // Activate copies into $$; rm deletes.
    assert_eq!(
        ok(home.path(), &["prompts", "use", "greet"]),
        "Activated prompt 'greet'.\n"
    );
    assert_eq!(read_json(home.path(), "prompts/$$.json")["max_tokens"], 5);
    assert_eq!(
        ok(home.path(), &["prompts", "rm", "greet"]),
        "Removed prompt 'greet'.\n"
    );
    assert!(fails(home.path(), &["prompts", "rm", "greet"]).contains("not found"));
    assert!(fails(home.path(), &["prompts", "set", "../evil", "x"]).contains("invalid prompt name"));
}

#[test]
fn prompts_adhoc_prints_without_saving() {
    let home = seeded_home();
    let v: Value = serde_json::from_str(&ok(
        home.path(),
        &["--json", "prompts", "hello", "-t", "0.1"],
    ))
    .unwrap();
    assert_eq!(
        v,
        json!({"prompt": "hello", "max_tokens": 256, "temperature": 0.1, "role": "user"})
    );
    assert!(std::fs::read_dir(home.path().join("prompts"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn providers_add_keys_rename_rm() {
    let home = seeded_home();
    let h = home.path();
    assert_eq!(
        ok(
            h,
            &[
                "providers",
                "add",
                "groq",
                "--url",
                "https://api.groq.com/openai/v1",
                "--default_model",
                "llama",
                "--api_key",
                "gsk-1234567890abcd",
                "--key_name",
                "main"
            ]
        ),
        "Added provider groq.\n"
    );
    let cfg = std::fs::read_to_string(h.join("providers/providers-config.yaml")).unwrap();
    assert!(cfg.contains("groq:") && cfg.contains("api: chat-completions"));
    assert!(
        !cfg.contains("gsk-"),
        "the key must not be written to the config"
    );
    assert!(cfg.contains("anthropic:"), "existing providers kept");
    assert_eq!(
        std::fs::read_to_string(h.join("providers/groq/default-model.yaml")).unwrap(),
        "default_model: llama\n"
    );
    assert!(fails(
        h,
        &[
            "providers",
            "add",
            "groq",
            "--url",
            "u",
            "--default_model",
            "m"
        ]
    )
    .contains("already exists"));
    assert!(fails(
        h,
        &[
            "providers",
            "add",
            "x",
            "--url",
            "u",
            "--default_model",
            "m",
            "--api",
            "nope"
        ]
    )
    .contains("unknown API"));

    // Keys: saved without the config listing them; masked by default, set
    // via stdin, remove — always by name.
    assert_eq!(
        ok(h, &["providers", "api-key", "show", "groq#main"]),
        "Looked for in this order:\n  1. providers/groq/keys/main (saved key) — used\nKey: gsk-**********abcd\n"
    );
    // Named none: every key.
    assert_eq!(
        ok(h, &["providers", "api-key", "show", "groq"]),
        "groq#main ★\n  Looked for in this order:\n    1. providers/groq/keys/main (saved key) — used\n  Key: gsk-**********abcd\n"
    );
    assert!(ok(
        h,
        &["providers", "api-key", "show", "groq#main", "--reveal"]
    )
    .ends_with("Key: gsk-1234567890abcd\n"));
    let o = lexi(h)
        .args(["providers", "api-key", "set", "groq#main"])
        .write_stdin("piped-key-value\n")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(ok(
        h,
        &[
            "providers",
            "api-key",
            "show",
            "groq",
            "--name",
            "main",
            "-r"
        ]
    )
    .ends_with("Key: piped-key-value\n"));
    // Changing a key without naming it is refused, before asking for it.
    assert!(
        fails(h, &["providers", "api-key", "set", "groq", "-v", "k"]).contains(
            "name the key: `lexi providers api-key set groq#NAME` (groq's keys: #main ★)"
        )
    );
    assert!(fails(h, &["providers", "api-key", "remove", "groq"]).contains("name the key"));
    assert_eq!(
        ok(h, &["providers", "api-key", "remove", "groq#main"]),
        "Removed the saved API key for groq#main.\n"
    );
    assert_eq!(
        ok(h, &["providers", "api-key", "remove", "groq#main"]),
        "No saved API key for groq#main.\n"
    );
    assert_eq!(
        ok(h, &["providers", "api-key", "show", "groq#main"]),
        "Looked for in this order:\n  1. providers/groq/keys/main (saved key) — missing: none saved\nNo API key found.\n"
    );
    // There's no --api any more: keys belong to the provider.
    assert!(
        lexi_out(
            h,
            &[
                "providers",
                "api-key",
                "set",
                "groq#main",
                "-v",
                "x",
                "--api",
                "chat-completions"
            ]
        )
        .status
        .code()
            == Some(2)
    );
    assert!(
        fails(h, &["providers", "api-key", "set", "nope#k", "-v", "k"])
            .contains("unknown provider")
    );

    ok(
        h,
        &["providers", "api-key", "set", "groq#main", "--value", "k2"],
    );
    assert_eq!(
        ok(h, &["providers", "rename", "groq", "groq2"]),
        "Renamed provider groq -> groq2.\n"
    );
    assert!(h.join("providers/groq2/keys/main").exists());
    assert!(!h.join("providers/groq").exists());
    assert_eq!(
        ok(h, &["providers", "rm", "groq2"]),
        "Removed provider groq2.\n"
    );
    assert!(!h.join("providers/groq2").exists());
    assert!(fails(h, &["providers", "rm", "groq2"]).contains("unknown provider"));
}

#[tokio::test]
async fn providers_status_checks_the_key_live() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(wiremock::matchers::header("authorization", "Bearer good"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let home = seeded_home();
    let h = home.path().to_path_buf();
    std::fs::write(
        h.join("providers/providers-config.yaml"),
        format!(
            "providers:\n  p:\n    url: {0}/v1\n    api: chat-completions\n  nokey:\n    url: {0}/v1\n    api: chat-completions\n",
            server.uri()
        ),
    )
    .unwrap();

    let status = |args: Vec<&'static str>| {
        let mut cmd = lexi(&h);
        cmd.args(args);
        run(cmd)
    };
    assert_eq!(
        stdout(&status(vec!["providers", "status", "nokey"]).await),
        "nokey: not ready (no key): add one with `lexi providers api-key set nokey#NAME`.\n"
    );

    // A key saved before keys had names is the unnamed key, `p#`.
    std::fs::create_dir_all(h.join("providers/p")).unwrap();
    std::fs::write(h.join("providers/p/api-key"), "good").unwrap();
    assert_eq!(
        stdout(&status(vec!["providers", "status", "p"]).await),
        "p#: ready (key from providers/p/api-key)\np: requests that name no key use p# ★.\n"
    );

    std::fs::write(h.join("providers/p/api-key"), "bad").unwrap();
    let o = status(vec!["providers", "status", "p#", "--raw"]).await;
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v,
        json!({"provider": "p", "key": "p#", "status": "invalid api-key", "source": "saved", "from": "providers/p/api-key", "verified": false, "reason": null})
    );
}

#[test]
fn key_sources_list_add_rm() {
    let home = seeded_home();
    let h = home.path();
    // A fresh install: no keys, and the provider's variable isn't one.
    assert_eq!(
        ok(h, &["providers", "api-key", "source", "list", "openai"]),
        "openai has no keys.\n$OPENAI_API_KEY isn't set (when it is, requests that name no key use it).\n"
    );
    ok(h, &["providers", "keys", "add", "openai#work"]);
    assert_eq!(
        ok(h, &["providers", "api-key", "source", "list", "openai#work"]),
        "Looked for in this order:\n  1. providers/openai/keys/work (saved key; last unless api_key places it)\n"
    );
    assert!(fails(
        h,
        &[
            "providers",
            "api-key",
            "source",
            "add",
            "openai",
            "--file",
            "~/keys/openai"
        ]
    )
    .contains("name the key"));
    assert!(ok(
        h,
        &[
            "providers",
            "api-key",
            "source",
            "add",
            "openai#work",
            "--file",
            "~/keys/openai",
            "--first"
        ]
    )
    .starts_with(
        "Updated openai#work's api_key.\nLooked for in this order:\n  1. ~/keys/openai\n"
    ));
    ok(
        h,
        &[
            "providers",
            "api-key",
            "source",
            "add",
            "openai",
            "--name",
            "work",
            "--saved",
            "--first",
        ],
    );
    let config = std::fs::read_to_string(h.join("providers/providers-config.yaml")).unwrap();
    assert!(config.contains("- saved\n"), "{config}");
    let v = lexi_out(
        h,
        &[
            "--json",
            "providers",
            "api-key",
            "source",
            "list",
            "openai#work",
        ],
    );
    let v: Value = serde_json::from_slice(&v.stdout).unwrap();
    let kinds: Vec<&str> = v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["saved", "file"]);

    assert_eq!(
        ok(
            h,
            &[
                "providers",
                "api-key",
                "source",
                "rm",
                "openai#work",
                "--file",
                "~/keys/openai"
            ]
        ),
        "Removed it from openai#work's api_key.\n"
    );
    assert_eq!(
        ok(
            h,
            &[
                "providers",
                "api-key",
                "source",
                "rm",
                "openai#work",
                "--file",
                "~/keys/openai"
            ]
        ),
        "openai#work's api_key doesn't list it.\n"
    );
    // Exactly one place (a usage error).
    let usage = |args: &[&str]| {
        let o = lexi_out(h, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        stderr(&o)
    };
    assert!(usage(&["providers", "api-key", "source", "add", "openai#work"]).contains("required"));
    assert!(usage(&[
        "providers",
        "api-key",
        "source",
        "add",
        "openai#work",
        "--env",
        "A",
        "--file",
        "b"
    ])
    .contains("cannot be used with"));
    assert!(fails(
        h,
        &[
            "providers",
            "api-key",
            "source",
            "add",
            "openai#work",
            "--command",
            "op 'read"
        ]
    )
    .contains("check its quotes"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_command_source_supplies_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer from-cmd",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;
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
    // `api_key` written in the config: the unnamed key, `p#`.
    ok(
        &h,
        &[
            "providers",
            "api-key",
            "source",
            "add",
            "p#",
            "--command",
            "sh -c 'echo from-cmd'",
        ],
    );
    // Showing doesn't run it; a live check does.
    assert!(ok(&h, &["providers", "api-key", "show", "p#"]).contains(
        "1. the command `sh -c echo from-cmd` — not run: runs when a request needs the key"
    ));
    let mut cmd = lexi(&h);
    cmd.args(["providers", "status", "p#"]);
    assert_eq!(
        stdout(&run(cmd).await),
        "p#: ready (key from the command `sh -c echo from-cmd`)\n"
    );
}

#[test]
fn models_add_set_rm_and_aliases() {
    let home = seeded_home();
    let h = home.path();
    std::fs::write(
        h.join("providers/openai/configured-models.yaml"),
        "models: {}\n",
    )
    .unwrap();
    assert_eq!(
        ok(
            h,
            &[
                "models",
                "add",
                "openai",
                "gpt-x",
                "--name",
                "GPT X",
                "--max_tokens",
                "10",
                "--default"
            ]
        ),
        "Added model gpt-x for openai.\n"
    );
    assert_eq!(
        ok(
            h,
            &[
                "models",
                "set",
                "openai",
                "gpt-x",
                "--description",
                "updated"
            ]
        ),
        "Updated model gpt-x for openai.\n"
    );
    let v: Value = serde_json::from_str(&ok(h, &["--json", "models", "list", "openai"])).unwrap();
    assert_eq!(v[0]["id"], "gpt-x");
    assert_eq!(v[0]["default"], true);
    assert_eq!(
        v[0]["metadata"],
        json!({"name": "GPT X", "max_tokens": 10, "description": "updated"})
    );

    assert_eq!(
        ok(h, &["models", "alias", "set", "openai", "gx", "gpt-x"]),
        "Set alias 'gx' -> 'gpt-x' for openai.\n"
    );
    assert!(ok(h, &["models", "alias", "list", "openai"]).contains("  gx -> gpt-x\n"));
    assert_eq!(
        ok(h, &["models", "alias", "rm", "openai", "gx"]),
        "Removed alias 'gx' from openai.\n"
    );
    fails(h, &["models", "alias", "rm", "openai", "gx"]);

    assert_eq!(
        ok(h, &["models", "rm", "openai", "gpt-x"]),
        "Removed model gpt-x from openai.\n"
    );
    fails(h, &["models", "rm", "openai", "gpt-x"]);
    assert!(fails(h, &["models", "add", "nope", "m"]).contains("unknown provider"));
}

#[test]
fn api_create_edit_rm() {
    let home = seeded_home();
    let h = home.path();
    assert_eq!(
        ok(
            h,
            &["api", "create", "mine", "--from", "messages", "--path", "/v9"]
        ),
        "API definition 'mine' created.\n"
    );
    assert!(fails(h, &["api", "create", "mine"]).contains("already exists"));
    assert!(fails(h, &["api", "create", "messages"]).contains("already exists"));
    assert_eq!(
        ok(
            h,
            &[
                "api",
                "edit",
                "mine",
                "--text-path",
                "content[0].text",
                "--set-meta",
                "stop:stop_reason"
            ]
        ),
        "API definition 'mine' updated.\n"
    );
    let def: Value = serde_json::from_str(&ok(h, &["--json", "api", "show", "mine"])).unwrap();
    assert_eq!(def["request"]["path"], "/v9");
    assert_eq!(
        def["response"]["text_extract"]["text_path"],
        "content[0].text"
    );
    assert_eq!(def["response"]["metadata"]["stop"], "stop_reason");

    assert!(fails(h, &["api", "edit", "messages", "--path", "/x"]).contains("shipped definition"));
    assert!(fails(h, &["api", "edit", "mine", "--set-meta", "nocolon"]).contains("KEY:PATH"));
    assert_eq!(
        ok(h, &["api", "rm", "mine"]),
        "API definition 'mine' removed.\n"
    );
    fails(h, &["api", "rm", "mine"]);
    fails(h, &["api", "rm", "messages"]);
}
