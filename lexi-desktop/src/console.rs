//! The CLI tab: runs `lexi` command lines in-process, exactly as the terminal
//! shell does (`lexi_cli::execute_line`: aliases, `#N`, the shared history),
//! streaming what they print to the webview as events, one JSON string each:
//!
//! - `{"type": "out" | "err", "text"}` — output as it's written;
//! - `{"type": "input", "prompt", "secret"}` — the command is asking for
//!   something (an API key); the answer comes back through [`give_input`];
//! - `{"type": "done", "code", "exit"}` — finished, with its exit code
//!   (`exit`: the line was `exit`/`quit`, which the app ignores).
//!
//! `sh` / `!cmd` are off: it's an app, not a shell.

use std::collections::HashMap;
use std::io::Write;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

/// Commands waiting for an answer, by request id.
fn waiting() -> &'static Mutex<HashMap<String, mpsc::Sender<Option<String>>>> {
    static WAITING: OnceLock<Mutex<HashMap<String, mpsc::Sender<Option<String>>>>> =
        OnceLock::new();
    WAITING.get_or_init(Default::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Answer the question the command `request_id` is waiting on (`None`:
/// declined). Whether it was waiting.
pub fn give_input(request_id: &str, value: Option<String>) -> bool {
    match lock(waiting()).remove(request_id) {
        Some(tx) => tx.send(value).is_ok(),
        None => false,
    }
}

/// Writes a stream's text out as events, holding back a UTF-8 sequence
/// split across writes.
struct EventWriter<F: Fn(Value)> {
    kind: &'static str,
    emit: F,
    pending: Vec<u8>,
}

impl<F: Fn(Value)> Write for EventWriter<F> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        let upto = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => self.pending.len(), // not UTF-8 at all: send it lossily
        };
        if upto > 0 {
            let text = String::from_utf8_lossy(&self.pending[..upto]).into_owned();
            self.pending.drain(..upto);
            (self.emit)(json!({ "type": self.kind, "text": text }));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run one command line; `emit` gets each event (see the module docs), the
/// last being `done`. A question blocks until [`give_input`] answers it.
pub async fn run<F>(line: &str, request_id: &str, emit: F)
where
    F: Fn(Value) + Clone + Send + Sync,
{
    let mut out = EventWriter {
        kind: "out",
        emit: emit.clone(),
        pending: Vec::new(),
    };
    let mut err = EventWriter {
        kind: "err",
        emit: emit.clone(),
        pending: Vec::new(),
    };
    let ask_emit = emit.clone();
    let id = request_id.to_string();
    let mut ask = move |prompt: &str, secret: bool| {
        let (tx, rx) = mpsc::channel();
        lock(waiting()).insert(id.clone(), tx);
        ask_emit(json!({ "type": "input", "prompt": prompt, "secret": secret }));
        let answer = rx.recv().ok().flatten();
        lock(waiting()).remove(&id);
        answer
    };
    let outcome = {
        let mut io = lexi_cli::Io::new(&mut out, &mut err, false);
        io.input = Some(&mut ask);
        let opts = lexi_cli::RunOptions {
            color: false,
            allow_shell: false,
        };
        lexi_cli::execute_line(line, &mut io, opts).await
    };
    emit(json!({ "type": "done", "code": outcome.code, "exit": outcome.exit }));
}

/// The shell history, oldest first (for ↑ / ↓).
pub fn history() -> Vec<String> {
    lexi_cli::history_entries()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn collect() -> (Arc<Mutex<Vec<Value>>>, impl Fn(Value) + Clone + Send + Sync) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        (events, move |v| sink.lock().unwrap().push(v))
    }

    #[test]
    fn split_utf8_is_held_until_complete() {
        let (events, emit) = collect();
        let mut w = EventWriter {
            kind: "out",
            emit,
            pending: Vec::new(),
        };
        let bytes = "é!".as_bytes();
        w.write_all(&bytes[..1]).unwrap();
        assert!(events.lock().unwrap().is_empty());
        w.write_all(&bytes[1..]).unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            [json!({ "type": "out", "text": "é!" })]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_a_line_and_answers_a_question() {
        let _env = crate::backend::tests::ENV_LOCK.lock().await;
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("LEXI_HOME", home.path());
        // Never the real keychain.
        std::env::set_var("LEXI_KEY_STORE", "file");
        lexi_host::ensure_home().unwrap();
        let (events, emit) = collect();
        run("version", "c1", emit.clone()).await;
        let got = events.lock().unwrap().clone();
        let text: String = got
            .iter()
            .filter(|e| e["type"] == "out")
            .filter_map(|e| e["text"].as_str())
            .collect();
        assert!(text.starts_with("lexi "), "{got:?}");
        assert_eq!(got.last().unwrap()["type"], "done");
        assert_eq!(got.last().unwrap()["code"], 0);

        // A question waits for its answer; declining fails the command.
        let (events, emit) = collect();
        let answer = std::thread::spawn(|| {
            for _ in 0..200 {
                if give_input("c2", None) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            false
        });
        run("providers api-key set openai#work", "c2", emit).await;
        assert!(answer.join().unwrap(), "the command never asked");
        let got = events.lock().unwrap().clone();
        assert!(got
            .iter()
            .any(|e| e["type"] == "input" && e["secret"] == true));
        assert_eq!(got.last().unwrap()["code"], 1);
    }
}
