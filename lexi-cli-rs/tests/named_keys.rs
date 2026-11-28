//! A provider's keys (`docs/key-storage-design.md` §11, §12) — `providers
//! keys …`, `PROVIDER#KEY` (`PROVIDER#` a key from before names), `--key`,
//! `default_key`.

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
    // `api_key: []` in the config: a key from before names, written `p#`.
    ok(
        h,
        &["providers", "api-key", "set", "p#", "--value", "sk-main"],
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
    // With no default_key, the unnamed key is the default (as before).
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
    let both = say(&["respond", "p#work", "hi", "--key", "other"]).await;
    assert!(
        stderr(&both).contains("name different keys"),
        "{}",
        stderr(&both)
    );
    // `default` isn't a key name.
    let old = say(&["respond", "p#default", "hi"]).await;
    assert!(
        stderr(&old).contains("`openai#`") || stderr(&old).contains("nothing after the #"),
        "{}",
        stderr(&old)
    );

    // The default key is used when a request names none; `p#` names the unnamed one.
    ok(h, &["providers", "keys", "default", "p", "work"]);
    assert_eq!(
        stdout(&say(&["respond", "p", "hi"]).await).trim(),
        "billed to work"
    );
    assert_eq!(
        stdout(&say(&["respond", "p#", "hi"]).await).trim(),
        "billed to main"
    );

    // Status checks the key it's given — or, named none, every key.
    let st = say(&["providers", "status", "p", "--key", ""]).await;
    assert_eq!(stdout(&st), "p#: ready (key from providers/p/api-key)\n");
    let st = say(&["providers", "status", "p#work"]).await;
    assert_eq!(
        stdout(&st),
        "p#work: ready (key from providers/p/keys/work)\n"
    );
    let st = say(&["providers", "status", "p"]).await;
    assert_eq!(
        stdout(&st),
        "p#: ready (key from providers/p/api-key)\np#work: ready (key from providers/p/keys/work)\np: requests that name no key use p#work ★.\n"
    );

    let unknown = say(&["respond", "p#nope", "hi"]).await;
    assert!(
        stderr(&unknown).contains("p has no key named \"nope\" (its keys: #, #work)"),
        "{}",
        stderr(&unknown)
    );
}

#[test]
fn keys_are_listed_added_renamed_and_removed() {
    let home = seeded_home();
    let h = home.path();
    assert_eq!(
        ok(h, &["providers", "keys", "add", "openai", "work"]),
        "Added the key openai#work. Save its value with: lexi providers api-key set openai#work\n"
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
        .map(|k| k["label"].as_str().unwrap())
        .collect();
    // A fresh install has no unnamed key, and the variable isn't a key.
    assert_eq!(names, ["#work", "#personal"]);
    assert!(list["default_key"].is_null());
    assert_eq!(list["reason"], "no default key");
    let shown = json_of(h, &["providers", "api-key", "show", "openai#personal"]);
    assert_eq!(shown["key_name"], "personal");
    assert_eq!(shown["save_to"], "providers/openai/keys/personal");
    assert_eq!(shown["key"], "****");

    ok(h, &["providers", "keys", "default", "openai#personal"]);
    assert_eq!(
        ok(h, &["providers", "keys", "default", "openai"]),
        "openai's default key is openai#personal: requests that name no key use it.\n"
    );
    assert!(ok(h, &["providers", "keys", "list", "openai"])
        .starts_with("openai's keys (★ = used when a request names none):\n  #work      no key\n★ #personal  providers/openai/keys/personal\n$OPENAI_API_KEY isn't set"));
    ok(
        h,
        &["providers", "keys", "rename", "openai#personal", "home"],
    );
    assert!(h.join("providers/openai/keys/home").exists());
    assert_eq!(
        json_of(h, &["providers", "keys", "list", "openai"])["default_key"],
        "home"
    );
    ok(
        h,
        &["providers", "keys", "rename", "openai", "home", "personal"],
    );
    // The default can be removed: with one key left, that's the default.
    assert!(ok(h, &["providers", "keys", "rm", "openai#personal"])
        .starts_with("Removed the key openai#personal"));
    assert!(!h.join("providers/openai/keys/personal").exists());
    assert_eq!(
        json_of(h, &["providers", "keys", "list", "openai"])["default_key"],
        "work"
    );
    // And the only key: the provider has none.
    assert_eq!(
        ok(h, &["providers", "keys", "rm", "openai", "work"]),
        "Removed the key openai#work and its saved key. openai has no key now.\n"
    );

    // Commands that change the provider itself take it alone.
    assert!(fails(h, &["providers", "rm", "openai#work"]).contains("leave out #work"));
    assert!(fails(h, &["models", "list", "openai#work"]).contains("leave out #work"));
    assert!(
        fails(h, &["providers", "keys", "add", "openai", "default"]).contains("isn't a key name")
    );
    assert!(fails(h, &["providers", "keys", "add", "openai#"]).contains("needs a name"));
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
}
