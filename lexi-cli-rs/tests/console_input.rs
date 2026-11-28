//! In-process hosts with no terminal (the desktop app's CLI console) answer a
//! command's questions through `Io::input`: the key a command asks for is
//! saved, but never printed and never written to the history.

mod common;

use lexi_cli::{execute_line, Io, RunOptions};

/// Run `line` in-process with `answer` as the reply to any question; returns
/// (exit code, stdout, stderr, the prompts asked).
async fn run(line: &str, answer: Option<&str>) -> (u8, String, String, Vec<(String, bool)>) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut asked = Vec::new();
    let answer = answer.map(String::from);
    let mut ask = |prompt: &str, secret: bool| {
        asked.push((prompt.to_string(), secret));
        answer.clone()
    };
    let code = {
        let mut io = Io::new(&mut out, &mut err, false);
        io.input = Some(&mut ask);
        let opts = RunOptions {
            color: false,
            allow_shell: false,
        };
        execute_line(line, &mut io, opts).await.code
    };
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
        asked,
    )
}

// One test: LEXI_HOME is process-wide.
#[tokio::test]
async fn questions_go_to_the_host_and_secrets_stay_out_of_history() {
    let home = common::seeded_home();
    std::env::set_var("LEXI_HOME", home.path());
    // Never the real keychain.
    std::env::set_var("LEXI_KEY_STORE", "file");

    let (code, out, err, asked) =
        run("providers api-key set openai#work", Some("sk-console-123")).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(asked, [("Enter API key: ".to_string(), true)]);
    assert!(!out.contains("sk-console-123") && !err.contains("sk-console-123"));
    let saved = std::fs::read_to_string(home.path().join("providers/openai/keys/work")).unwrap();
    assert_eq!(saved.trim(), "sk-console-123");
    let history = lexi_cli::history_entries();
    assert_eq!(
        history.last().map(String::as_str),
        Some("providers api-key set openai#work")
    );
    assert!(!std::fs::read_to_string(home.path().join("history"))
        .unwrap()
        .contains("sk-console"));

    // Declining fails the command and changes nothing.
    let (code, _, err, _) = run("providers api-key set anthropic#work", None).await;
    assert_eq!(code, 1);
    assert!(err.contains("nothing was entered"), "{err}");
    assert!(!home.path().join("providers/anthropic/keys/work").exists());

    // `sh` / `!` are off for in-process hosts.
    let (code, _, err, asked) = run("!echo hi", None).await;
    assert_ne!(code, 0);
    assert!(asked.is_empty());
    assert!(!err.is_empty());
}
