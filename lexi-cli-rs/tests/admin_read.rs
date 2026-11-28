//! C2 read-only admin commands: config, prompts, providers, models, api.

mod common;

use common::{lexi, lexi_out, run, seeded_home, stderr, stdout};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn json_out(home: &std::path::Path, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let o = lexi_out(home, &full);
    assert!(o.status.success(), "{args:?}: {}", stderr(&o));
    serde_json::from_slice(&o.stdout).unwrap()
}

fn write(home: &std::path::Path, rel: &str, text: &str) {
    let p = home.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn config_list_overlays_saved_values_on_defaults() {
    let home = seeded_home();
    write(
        home.path(),
        "config.json",
        r#"{"prompt": "lexi", "theme": "dark"}"#,
    );

    let out = stdout(&lexi_out(home.path(), &["config", "list"]));
    assert!(out.contains("cli_name=\"lexi\"\n"), "{out}");
    assert!(out.contains("prompt=\"lexi\"\n"));
    assert!(out.contains("theme=\"dark\"\n"));

    assert_eq!(
        stdout(&lexi_out(home.path(), &["config", "ls", "theme", "-v"])),
        "dark\n"
    );
    assert_eq!(
        json_out(home.path(), &["config", "list", "theme"]),
        json!({"key": "theme", "value": "dark"})
    );
    let all = json_out(home.path(), &["config", "list"]);
    assert_eq!(all["edit_mode"], "vi");

    let o = lexi_out(home.path(), &["config", "list", "nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("\"nope\" not found"));
}

#[test]
fn prompts_list_puts_the_active_prompt_first() {
    let home = seeded_home();
    let body = |p: &str| {
        format!(r#"{{"prompt": "{p}", "max_tokens": 256, "temperature": 0.7, "role": "user"}}"#)
    };
    write(home.path(), "prompts/zeta.json", &body("last"));
    write(home.path(), "prompts/alpha.json", &body("first"));
    write(home.path(), "prompts/$$.json", &body("active one"));

    assert_eq!(
        stdout(&lexi_out(home.path(), &["prompts", "list"])),
        "* $$: active one\nalpha: first\nzeta: last\n"
    );
    let detailed = stdout(&lexi_out(home.path(), &["prompts", "list", "alpha", "-d"]));
    assert_eq!(
        detailed,
        "alpha: first\n    max_tokens=256 temperature=0.7 role=user\n"
    );
    let table = stdout(&lexi_out(home.path(), &["prompts", "list", "--table"]));
    assert!(table.starts_with("Prompts\n") && table.contains("active one"));

    let all = json_out(home.path(), &["prompts", "list"]);
    assert_eq!(all[0]["name"], "$$");
    assert_eq!(all[0]["active"], true);
    assert_eq!(all[1]["prompt"], "first");

    let o = lexi_out(home.path(), &["prompts", "list", "missing"]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn prompts_list_empty() {
    let home = seeded_home();
    assert_eq!(
        stdout(&lexi_out(home.path(), &["prompts", "list"])),
        "No prompts saved.\n"
    );
}

#[test]
fn providers_list_reports_key_sources_without_revealing_keys() {
    let home = seeded_home();
    write(
        home.path(),
        "providers/providers-config.yaml",
        "providers:\n  a:\n    url: https://a.example/v1\n    api: messages\n    api_key: sk-literal\n  \
         b:\n    url: https://b.example/v1\n    api: responses\n    api_key: ${LEXI_TEST_B_KEY}\n  \
         c:\n    url: https://c.example/v1\n    api: chat-completions\n  \
         d:\n    url: https://d.example/v1\n    api: chat-completions\n",
    );
    write(home.path(), "providers/c/api-key", "sk-file\n");

    let o = lexi(home.path())
        .args(["providers", "list"])
        .env("LEXI_TEST_B_KEY", "sk-env")
        .output()
        .unwrap();
    let out = stdout(&o);
    assert!(
        out.contains("a: url=https://a.example/v1 api=messages"),
        "{out}"
    );
    assert!(out
        .lines()
        .any(|l| l.starts_with("a:") && l.ends_with("api_key=[config]")));
    assert!(out
        .lines()
        .any(|l| l.starts_with("b:") && l.ends_with("api_key=[env]")));
    assert!(out
        .lines()
        .any(|l| l.starts_with("c:") && l.ends_with("api_key=[saved]")));
    assert!(out
        .lines()
        .any(|l| l.starts_with("d:") && l.ends_with("api_key=[missing]")));
    assert!(!out.contains("sk-"), "keys must never be printed: {out}");

    let v = json_out(home.path(), &["providers", "list"]);
    assert_eq!(v[0]["name"], "a");
    assert_eq!(v[0]["key_source"], "config");
    assert_eq!(v[1]["key_source"], Value::Null); // env var not set here
    assert!(!v.to_string().contains("sk-"));

    let raw = stdout(&lexi_out(home.path(), &["providers", "list", "--raw"]));
    assert!(raw.starts_with("providers:\n  a:"));
}

#[test]
fn models_list_default_and_aliases_from_seeded_files() {
    let home = seeded_home();
    write(
        home.path(),
        "providers/openai/configured-models.yaml",
        "models:\n  gpt-a:\n    name: GPT A\n    description: first\n    max_tokens: 100\n  gpt-b:\n    name: GPT B\n    description: second\n",
    );
    write(
        home.path(),
        "providers/openai/default-model.yaml",
        "default_model: gpt-a\n",
    );
    write(
        home.path(),
        "providers/openai/model-aliases.yaml",
        "aliases:\n  fast: gpt-b\n",
    );

    let out = stdout(&lexi_out(home.path(), &["models", "list", "openai"]));
    assert_eq!(
        out,
        "Models for openai:\n  gpt-a: GPT A - first (default chat)\n  gpt-b: GPT B - second (alias: fast)\nAliases:\n  fast -> gpt-b\n"
    );
    let details = stdout(&lexi_out(home.path(), &["models", "list", "openai", "-d"]));
    assert!(details.contains("  gpt-a: GPT A - first (default chat)\n    max_tokens: 100\n"));

    let v = json_out(home.path(), &["models", "list", "openai"]);
    assert_eq!(v[0]["id"], "gpt-a");
    assert_eq!(v[0]["default"], true);
    assert_eq!(v[1]["aliases"], json!(["fast"]));
    assert_eq!(v[0]["metadata"]["max_tokens"], 100);

    assert_eq!(
        stdout(&lexi_out(home.path(), &["models", "default", "openai"])),
        "Default models for openai:\n  chat: gpt-a (yours)\n  image-generation: gpt-image-2.5-flare (the provider's)\n  embedding: text-embedding-3-small (the provider's)\n"
    );
    assert_eq!(
        stdout(&lexi_out(
            home.path(),
            &["models", "default", "openai", "gpt-b"]
        )),
        "gpt-b is now the default chat model for openai.\n"
    );
    let v = json_out(home.path(), &["models", "default", "openai"]);
    assert_eq!(v["default_model"], "gpt-b");
    assert_eq!(
        v["defaults"][0],
        json!({"kind": "chat", "model": "gpt-b", "source": "user"})
    );
    assert_eq!(
        v["defaults"][1],
        json!({"kind": "image-generation", "model": "gpt-image-2.5-flare", "source": "binding"})
    );
    let o = lexi_out(home.path(), &["models", "default", "openai", "nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("\"nope\" is not a configured model"));

    // A model is the default for its own kind; the chat default is kept.
    write(
        home.path(),
        "providers/openai/configured-models.yaml",
        "models:\n  gpt-a:\n    name: GPT A\n    description: first\n    max_tokens: 100\n  gpt-b:\n    name: GPT B\n    description: second\n  gpt-image-9:\n    name: Img\n",
    );
    assert_eq!(
        stdout(&lexi_out(
            home.path(),
            &["models", "default", "openai", "gpt-image-9"]
        )),
        "gpt-image-9 is now the default image-generation model for openai.\n"
    );
    assert_eq!(
        json_out(
            home.path(),
            &["models", "default", "openai", "--kind", "image-generation"]
        ),
        json!({"provider": "openai", "kind": "image-generation", "model": "gpt-image-9", "source": "user"})
    );
    assert_eq!(
        json_out(
            home.path(),
            &["models", "default", "openai", "--kind", "chat"]
        )["model"],
        "gpt-b"
    );
    let file =
        std::fs::read_to_string(home.path().join("providers/openai/default-model.yaml")).unwrap();
    assert!(file.starts_with("default_model: gpt-b\n"), "{file}");
    let v = json_out(home.path(), &["models", "list", "openai"]);
    assert_eq!(v[2]["default_for"], json!(["image-generation"]));

    // --kind must match the model's kind.
    let o = lexi_out(
        home.path(),
        &[
            "models",
            "default",
            "openai",
            "gpt-a",
            "--kind",
            "embedding",
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("gpt-a is a chat model, not embedding"),
        "{}",
        stderr(&o)
    );
    let o = lexi_out(
        home.path(),
        &["models", "default", "openai", "--kind", "pictures"],
    );
    assert_eq!(o.status.code(), Some(1));

    // Clearing goes back to the provider's default.
    assert_eq!(
        stdout(&lexi_out(
            home.path(),
            &["models", "default", "openai", "--clear", "--kind", "image-generation"]
        )),
        "Cleared your default image-generation model for openai; it's gpt-image-2.5-flare (the provider's) now.\n"
    );
    let o = lexi_out(home.path(), &["models", "default", "openai", "--clear"]);
    assert_eq!(o.status.code(), Some(2), "--clear needs --kind");

    assert_eq!(
        stdout(&lexi_out(
            home.path(),
            &["models", "alias", "list", "openai"]
        )),
        "  fast -> gpt-b\n"
    );
    assert_eq!(
        json_out(home.path(), &["models", "alias", "ls", "openai"]),
        json!([{"alias": "fast", "model": "gpt-b"}])
    );

    let o = lexi_out(home.path(), &["models", "list", "nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("unknown provider"));
}

#[tokio::test]
async fn models_hosted_lists_the_live_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer k"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "m1", "owned_by": "x", "extra": 1}, {"id": "m2", "owned_by": "y"}]
        })))
        .mount(&server)
        .await;
    let home = seeded_home();
    write(
        home.path(),
        "providers/providers-config.yaml",
        &format!(
            "providers:\n  p:\n    url: {}/v1\n    api: chat-completions\n    api_key: k\n    hosted_model_fields: [id, owned_by]\n",
            server.uri()
        ),
    );

    let mut cmd = lexi(home.path());
    cmd.args(["models", "hosted", "p"]);
    let o = run(cmd).await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "Hosted models for p:\n  id=m1, owned_by=x, kind=chat, api=chat-completions\n  \
         id=m2, owned_by=y, kind=chat, api=chat-completions\n"
    );

    let mut cmd = lexi(home.path());
    cmd.args(["--json", "models", "hosted", "p"]);
    let v: Value = serde_json::from_slice(&run(cmd).await.stdout).unwrap();
    assert_eq!(v[0]["extra"], 1);
}

#[test]
fn api_list_show_and_validate() {
    let home = seeded_home();
    let list = stdout(&lexi_out(home.path(), &["api", "list"]));
    for name in [
        "chat-completions",
        "image-generations",
        "messages",
        "responses",
    ] {
        assert!(list.contains(name), "{list}");
    }
    let v = json_out(home.path(), &["api", "list"]);
    let messages = v
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "messages")
        .unwrap();
    assert_eq!(messages["builtin"], true);
    assert_eq!(messages["path"], "/messages");
    assert_eq!(messages["text_path"], "items:content");

    let show = stdout(&lexi_out(home.path(), &["api", "show", "messages"]));
    assert!(show.starts_with("messages\n"));
    assert!(show.contains("  Request: POST /messages\n"));
    assert!(show.contains("max_tokens (integer) [required]"));
    let def = json_out(home.path(), &["api", "show", "messages"]);
    assert_eq!(def["request"]["path"], "/messages");

    assert_eq!(
        stdout(&lexi_out(home.path(), &["api", "validate", "messages"])),
        "API definition 'messages' is valid.\n"
    );

    // A custom, broken definition: exit 1, errors listed (stdout JSON in --json).
    write(
        home.path(),
        "apis/broken.yaml",
        "name: broken\nrequest: {}\n",
    );
    let o = lexi_out(home.path(), &["api", "validate", "broken"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Validation errors for 'broken':\n  - "));
    let o = lexi_out(home.path(), &["--json", "api", "validate", "broken"]);
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["valid"], false);
    assert!(!v["errors"].as_array().unwrap().is_empty());

    let o = lexi_out(home.path(), &["api", "show", "nope"]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn models_hosted_schema_shows_how_the_listing_is_fetched() {
    let home = seeded_home();
    let h = home.path();
    // A shipped provider: its template's catalog.
    let anthropic = json_out(h, &["models", "hosted", "anthropic", "--schema"]);
    assert_eq!(anthropic["url"], "https://api.anthropic.com/v1/models");
    assert_eq!(anthropic["source"], "template");
    assert_eq!(anthropic["template"], "anthropic");
    assert_eq!(anthropic["catalog"]["paginate"]["after_param"], "after_id");

    // Your own catalog settings (over a template's), the older
    // hosted_models form, and none at all.
    write(
        h,
        "providers/providers-config.yaml",
        "providers:\n  mine:\n    template: openai\n    catalog: {path: /v2/list, items_path: result}\n  old:\n    url: https://old.example/v1\n    api: chat-completions\n    hosted_models: /catalog\n  plain:\n    url: https://plain.example/v1/\n    api: chat-completions\n",
    );
    let mine = json_out(h, &["models", "hosted", "mine", "--schema"]);
    assert_eq!(mine["source"], "config");
    assert_eq!(mine["url"], "https://api.openai.com/v1/v2/list");
    assert_eq!(mine["catalog"]["items_path"], "result");
    // A catalog block replaces nothing it doesn't name: the template's
    // fields still apply.
    assert_eq!(mine["catalog"]["fields"][0], "id");
    let old = json_out(h, &["models", "hosted", "old", "--schema"]);
    assert_eq!(old["source"], "config");
    assert_eq!(old["catalog"], json!({ "path": "/catalog" }));
    assert_eq!(old["url"], "https://old.example/v1/catalog");
    let plain = json_out(h, &["models", "hosted", "plain", "--schema"]);
    assert_eq!(plain["source"], "default");
    assert_eq!(plain["catalog"], json!({}));
    assert_eq!(plain["url"], "https://plain.example/v1/models");

    let text = stdout(&lexi_out(h, &["models", "hosted", "mine", "--schema"]));
    assert!(text.starts_with(
        "mine's hosted models: GET https://api.openai.com/v1/v2/list\nCatalog settings, from providers-config.yaml (over the openai template):\n"
    ), "{text}");
    assert!(text.contains("\n  items_path: result\n"), "{text}");
    assert!(
        stdout(&lexi_out(h, &["models", "hosted", "plain", "--schema"]))
            .contains("No catalog settings: the models are read from the response's `data`")
    );
    // It never fetches, so it can't be combined with what a fetch does.
    assert_eq!(
        lexi_out(h, &["models", "hosted", "mine", "--schema", "--cached"])
            .status
            .code(),
        Some(2)
    );
}
