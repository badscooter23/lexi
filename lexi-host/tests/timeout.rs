//! `request_timeout` (config.json): a buffered request must finish within it;
//! a streamed one may run as long as it keeps sending, and stops after that
//! long with nothing received. (Its own test binary: it sets `LEXI_HOME`.)

use std::time::{Duration, Instant};

use lexi_host::config;
use lexi_host::respond::{respond_stream_with_effective, respond_with_effective};
use lexi_host::HostError;
use serde_json::{json, Map, Value};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn set_timeout(v: Value) -> Result<Value, HostError> {
    let mut m = Map::new();
    m.insert("request_timeout".into(), v);
    config::set_config(&m)
}

#[tokio::test]
async fn requests_stop_waiting_after_request_timeout() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("LEXI_HOME", home.path());
    // Never the real keychain.
    std::env::set_var("LEXI_KEY_STORE", "file");

    // The setting is a whole number of seconds (text from the CLI works too).
    assert_eq!(set_timeout(json!("2")).unwrap()["request_timeout"], 2);
    assert!(set_timeout(json!("soon")).is_err());
    assert!(set_timeout(json!(-1)).is_err());
    assert_eq!(set_timeout(json!(1)).unwrap()["request_timeout"], 1);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(4))
                .set_body_json(json!({"content": [{"type": "text", "text": "late"}]})),
        )
        .mount(&server)
        .await;
    let mut eff = config::load_api_def("messages").unwrap();
    eff["url"] = Value::from(server.uri());
    let client = reqwest::Client::new();

    // Buffered: the whole wait is limited.
    let started = Instant::now();
    let err = respond_with_effective(&client, &eff, "k", "hi", "user", "m", &[])
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "stopped at the limit"
    );
    assert!(matches!(err, HostError::RequestTimeout(_)), "{err}");
    assert!(err.to_string().contains("within 1 seconds"), "{err}");

    // Streaming: nothing received for the limit (here, before the reply
    // starts) stops it.
    let started = Instant::now();
    let err = respond_stream_with_effective(&client, &eff, "k", "hi", "user", "m", &[], |_| {})
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "stopped at the limit"
    );
    assert!(
        err.to_string().contains("sent nothing for 1 seconds"),
        "{err}"
    );

    // 0 turns the limit off: the slow reply is waited for.
    set_timeout(json!(0)).unwrap();
    let res = respond_with_effective(&client, &eff, "k", "hi", "user", "m", &[])
        .await
        .expect("no limit");
    assert_eq!(res.text, "late");

    // One operation's own limit wins over config.json, either way.
    let started = Instant::now();
    let err = lexi_host::with_request_timeout(
        1,
        respond_with_effective(&client, &eff, "k", "hi", "user", "m", &[]),
    )
    .await
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        err.to_string()
            .contains("the timeout given for this request"),
        "{err}"
    );
    set_timeout(json!(1)).unwrap();
    let res = lexi_host::with_request_timeout(
        0,
        respond_with_effective(&client, &eff, "k", "hi", "user", "m", &[]),
    )
    .await
    .expect("no limit for this one");
    assert_eq!(res.text, "late");
    // … and only for that operation.
    let err = respond_with_effective(&client, &eff, "k", "hi", "user", "m", &[])
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("request_timeout in config.json"),
        "{err}"
    );

    // Unset, the default is 5 minutes: a reply well within it is fine.
    config::delete_config("request_timeout").unwrap();
    assert_eq!(config::DEFAULT_REQUEST_TIMEOUT, 300);
}
