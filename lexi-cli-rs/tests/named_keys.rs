//! Named keys: several accounts per provider (`docs/key-storage-design.md`
//! §11) — `providers keys …`, `PROVIDER#KEY`, `--key`, `default_key`.

mod common;

use common::{lexi, lexi_out, run, seeded_home, stderr, stdout};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok(home: &std::path::Path, args: &[&str]) -> String {
    let o = lexi_out(home, args);
    assert!(o.status.success(), "{args:?} failed: {}", stderr(&o));
    stdout(&o)
}

fn fails(home: &std::path::Path, args: &[&str]) -> String {
    let o = lexi_out(home, args);
    assert!(!o.status.success(), "{args:?} should fail: {}", stdout(&o));
    stderr(&o)
}

fn json_of(home: &std::path::Path, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    serde_json::from_str(&ok(home, &full)).unwrap()
}

/// A chat-completions provider `p` whose server answers each account's key
/// with that account's name (and 401s anything else).
async fn accounts() -> (MockServer, tempfile::TempDir) {
    let server = MockServer::start().await;
    for (key, who) in [("sk-main", "main"), ("sk-work", "work")] {
        let bearer = format!("Bearer {key}");
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", bearer.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "role": "assistant", "content": format!("billed to {who}") } }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", bearer.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let home = seeded_home();
    std::fs::write(
        home.path().join("providers/providers-config.yaml"),
        format!(
            "providers:\n  p:\n    url: {}/v1\n    api: chat-completions\n    api_key: []\n    default_model: m\n",
            server.uri()
        ),
    )
    .unwrap();
    (server, home)
}

#[tokio::test]
async fn a_request_is_billed_to_the_key_it_names() {
    let (_server, home) = accounts().await;
    let h = home.path();
    ok(
        h,
        &["providers", "api-key", "set", "p", "--value", "sk-main"],
    );
    assert_eq!(
        ok(h, &["providers", "api-key", "set", "p#work", "--value", "sk-work"]),
        "Saved the API key for p#work in providers/p/keys/work. Added the key to providers-config.yaml.\n"
    );
    let say = |args: &[&str]| {
        let mut cmd = lexi(h);
        cmd.args(args);
        run(cmd)
    };
    assert_eq!(
        stdout(&say(&["respond", "p", "hi"]).await).trim(),
        "billed to main"
    );
    assert_eq!(
        stdout(&say(&["respond", "p#work", "hi"]).await).trim(),
        "billed to work"
    );
    assert_eq!(
        stdout(&say(&["respond", "p", "hi", "--key", "work"]).await).trim(),
        "billed to work"
    );
    // Both ways, agreeing, is fine; disagreeing isn't.
    assert!(say(&["respond", "p#work", "hi", "--key", "work"])
        .await
        .status
        .success());
    let both = say(&["respond", "p#work", "hi", "--key", "default"]).await;
    assert!(
        stderr(&both).contains("name different keys"),
        "{}",
        stderr(&both)
    );

    // The default key is used when a request names none; #default is the unnamed one.
    ok(h, &["providers", "keys", "default", "p", "work"]);
    assert_eq!(
        stdout(&say(&["respond", "p", "hi"]).await).trim(),
        "billed to work"
    );
    assert_eq!(
        stdout(&say(&["respond", "p#default", "hi"]).await).trim(),
        "billed to main"
    );

    // Status checks the key it's given.
    let st = say(&["providers", "status", "p", "--key", "default"]).await;
    assert_eq!(
        stdout(&st),
        "p#default: ready (key from providers/p/api-key)\n"
    );
    let st = say(&["providers", "status", "p#work"]).await;
    assert_eq!(
        stdout(&st),
        "p#work: ready (key from providers/p/keys/work)\n"
    );

    let unknown = say(&["respond", "p#nope", "hi"]).await;
    assert!(
        stderr(&unknown).contains("p has no key named \"nope\" (its keys: default, work)"),
        "{}",
        stderr(&unknown)
    );
}

#[test]
fn named_keys_are_listed_added_renamed_and_removed() {
    let home = seeded_home();
    let h = home.path();
    assert_eq!(
        ok(h, &["providers", "keys", "add", "openai", "work"]),
        "Added the key openai#work. Save it with: lexi providers api-key set openai#work\n"
    );
    ok(
        h,
        &[
            "providers",
            "api-key",
            "source",
            "add",
            "openai#work",
            "--env",
            "WORK_KEY",
        ],
    );
    ok(
        h,
        &[
            "providers",
            "api-key",
            "set",
            "openai",
            "--name",
            "personal",
            "--value",
            "sk-p",
        ],
    );
    let list = json_of(h, &["providers", "keys", "list", "openai"]);
    let names: Vec<&str> = list["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["default", "work", "personal"]);
    assert_eq!(list["default_key"], "default");
    let shown = json_of(h, &["providers", "api-key", "show", "openai#personal"]);
    assert_eq!(shown["key_name"], "personal");
    assert_eq!(shown["save_to"], "providers/openai/keys/personal");
    assert_eq!(shown["key"], "****");

    ok(h, &["providers", "keys", "default", "openai", "personal"]);
    assert_eq!(
        ok(h, &["providers", "keys", "default", "openai"]),
        "openai's requests use the key personal (openai#personal).\n"
    );
    ok(
        h,
        &["providers", "keys", "rename", "openai", "personal", "home"],
    );
    assert!(h.join("providers/openai/keys/home").exists());
    assert_eq!(
        json_of(h, &["providers", "keys", "list", "openai"])["default_key"],
        "home"
    );
    ok(h, &["providers", "keys", "rm", "openai", "home"]);
    assert!(!h.join("providers/openai/keys/home").exists());
    assert_eq!(
        json_of(h, &["providers", "keys", "list", "openai"])["default_key"],
        "default"
    );

    // Commands that change the provider itself take it alone.
    assert!(fails(h, &["providers", "rm", "openai#work"]).contains("leave out #work"));
    assert!(fails(h, &["models", "list", "openai#work"]).contains("leave out #work"));
    assert!(fails(h, &["providers", "keys", "add", "openai", "default"]).contains("unnamed key"));
    assert!(fails(
        h,
        &[
            "providers",
            "add",
            "a#b",
            "--url",
            "u",
            "--default-model",
            "m"
        ]
    )
    .contains("invalid provider name"));
    assert!(fails(
        h,
        &[
            "providers",
            "api-key",
            "set",
            "openai#work",
            "--api",
            "responses",
            "--value",
            "x"
        ]
    )
    .contains("covers every API"));
}
