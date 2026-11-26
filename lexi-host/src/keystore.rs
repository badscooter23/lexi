//! Where API keys come from. A provider's `api_key` in providers-config.yaml
//! is an ordered list of sources — `{env: VAR}`, `{file: PATH}`,
//! `{command: [PROGRAM, ARG…]}` (or the older `${VAR}` / literal strings) —
//! and the first one that has a key is used. Only a missing key (an unset
//! variable, no file, an empty file, a command that prints nothing) moves on
//! to the next; a read error stops the lookup, so an unreadable file or a
//! failing command never silently switches you to a different key.
//!
//! The listed sources are never written: a key Lexi saves goes in its own
//! store (`saved`, a 0600 file under `$LEXI_HOME` — see
//! `docs/key-storage-design.md`), which is looked in after the listed sources,
//! or where the marker `saved` puts it.
//!
//! A development build briefly kept keys in the OS keychain
//! (`{keychain: NAME}`); `ensure_home` rewrites such entries to `saved`, and
//! one written by hand is a config error that says what to use instead.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// One place a key can come from: an entry of an `api_key` list. (No `Debug`:
/// a literal holds the key itself.)
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// An environment variable.
    Env(String),
    /// A file holding the key (yours: Lexi never writes it); relative paths
    /// are under `$LEXI_HOME`.
    File(String),
    /// A program that prints the key (a password manager, `sops -d`, …):
    /// its argv, run without a shell.
    Command(Vec<String>),
    /// The key written into providers-config.yaml itself.
    Literal(String),
    /// The key Lexi saved — its key file — looked in after the listed sources
    /// unless the marker `saved` puts it earlier. Parsed as `Saved("")`;
    /// [`with_saved`] fills in the file.
    Saved(String),
}

const SOURCE_FORMS: &str =
    "api_key entries are {env: VAR}, {file: PATH}, {command: [PROGRAM, ARG, …]}, saved or \"${VAR}\"";

/// The marker for the saved key's place in an `api_key` list.
pub(crate) const SAVED: &str = "saved";

impl Source {
    /// `env`, `file`, `command`, `saved` or `config` (a literal).
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Source::Env(_) => "env",
            Source::File(_) => "file",
            Source::Command(_) => "command",
            Source::Literal(_) => "config",
            Source::Saved(_) => "saved",
        }
    }

    /// Where it is, for messages: `$OPENAI_API_KEY`, a path (a saved key's is
    /// its key file), a command line.
    pub(crate) fn describe(&self) -> String {
        match self {
            Source::Env(v) => format!("${v}"),
            Source::File(p) => p.clone(),
            Source::Command(argv) => format!("the command `{}`", argv.join(" ")),
            Source::Literal(_) => "providers-config.yaml (the key itself)".into(),
            Source::Saved(p) if p.is_empty() => "the saved key".into(),
            Source::Saved(p) => p.clone(),
        }
    }

    /// As written in providers-config.yaml.
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Source::Env(v) => json!({ "env": v }),
            Source::File(p) => json!({ "file": p }),
            Source::Command(argv) => json!({ "command": argv }),
            Source::Literal(k) => Value::from(k.as_str()),
            Source::Saved(_) => Value::from(SAVED),
        }
    }

    /// The same place, ignoring where a saved key's file is.
    pub(crate) fn same_place(&self, other: &Source) -> bool {
        match (self, other) {
            (Source::Saved(_), Source::Saved(_)) => true,
            (a, b) => a == b,
        }
    }

    /// One `api_key` entry (or `None` for an empty one).
    pub(crate) fn parse(v: &Value) -> Result<Option<Source>, String> {
        match v {
            Value::Null => Ok(None),
            Value::String(s) if s.trim().is_empty() => Ok(None),
            Value::String(s) if s.trim() == SAVED => Ok(Some(Source::Saved(String::new()))),
            Value::String(s) => Ok(Some(
                match s.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
                    Some(var) => Source::Env(var.to_string()),
                    None => Source::Literal(s.clone()),
                },
            )),
            Value::Object(o) if o.len() == 1 && o.contains_key("command") => {
                let argv: Option<Vec<String>> = match &o["command"] {
                    Value::Array(items) => {
                        items.iter().map(|i| i.as_str().map(String::from)).collect()
                    }
                    _ => None,
                };
                match argv {
                    Some(argv) if argv.first().is_some_and(|p| !p.trim().is_empty()) => {
                        Ok(Some(Source::Command(argv)))
                    }
                    _ => Err(
                        "api_key command: needs a list — the program, then its arguments, e.g. {command: [op, read, \"op://Private/OpenAI/credential\"]}"
                            .into(),
                    ),
                }
            }
            Value::Object(o) if o.len() == 1 => {
                let (k, v) = o.iter().next().expect("one entry");
                let name = v
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("api_key {k}: needs a name or path ({SOURCE_FORMS})"))?
                    .to_string();
                match k.as_str() {
                    "env" => Ok(Some(Source::Env(name))),
                    "file" => Ok(Some(Source::File(name))),
                    "keychain" => Err(format!(
                        "api_key {{keychain: {name}}}: save the key again (the desktop app's Setup Wizard, or `lexi providers api-key set`) and list it as `saved`, or use {{file: PATH}} or {{env: VAR}}"
                    )),
                    other => Err(format!("unknown api_key source {other:?} ({SOURCE_FORMS})")),
                }
            }
            _ => Err(SOURCE_FORMS.to_string()),
        }
    }
}

/// An `api_key` value (a source, or a list of them) as its sources, in order.
pub(crate) fn parse_sources(v: Option<&Value>) -> Result<Vec<Source>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| Source::parse(i).transpose())
            .collect(),
        Some(one) => Ok(Source::parse(one)?.into_iter().collect()),
    }
}

/// Sources as an `api_key` value (always a list).
pub(crate) fn sources_value(sources: &[Source]) -> Value {
    Value::Array(sources.iter().map(Source::to_value).collect())
}

/// The sources looked in: the listed ones with the saved key (its file
/// `saved`) where the marker `saved` is, else last. With no saved key (a
/// model's own `api_key`), the marker is dropped.
pub(crate) fn with_saved(listed: &[Source], saved: Option<&str>) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    let mut placed = false;
    for s in listed {
        match s {
            Source::Saved(_) => {
                if let (Some(file), false) = (saved, placed) {
                    out.push(Source::Saved(file.to_string()));
                    placed = true;
                }
            }
            other => out.push(other.clone()),
        }
    }
    if let (Some(file), false) = (saved, placed) {
        out.push(Source::Saved(file.to_string()));
    }
    out
}

/// A file source's path: `~/…` is under your home folder, a relative path is
/// under `$LEXI_HOME`.
pub(crate) fn file_path(home: &Path, raw: &str) -> PathBuf {
    if raw == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        home.join(p)
    }
}

/// What looking at one source found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// It had the key that's used.
    Used,
    /// No key there (`not set`, `no file`, `empty`) — the next source is tried.
    Missing(&'static str),
    /// Reading it failed; the lookup stops here.
    Failed(String),
    /// A command that wasn't run (looking without running commands — a
    /// listing, the startup screen): it decides the lookup, key unknown.
    NotRun,
    /// An earlier source decided the lookup.
    NotChecked,
}

impl Outcome {
    /// `used`, `missing`, `error`, `not run` or `not checked`.
    pub(crate) fn state(&self) -> &'static str {
        match self {
            Outcome::Used => "used",
            Outcome::Missing(_) => "missing",
            Outcome::Failed(_) => "error",
            Outcome::NotRun => "not run",
            Outcome::NotChecked => "not checked",
        }
    }

    pub(crate) fn note(&self) -> Option<String> {
        match self {
            Outcome::Missing(n) => Some((*n).to_string()),
            Outcome::Failed(e) => Some(e.clone()),
            Outcome::NotRun => Some("runs when a request needs the key".into()),
            _ => None,
        }
    }

    /// Whether this source settled the lookup (later ones aren't looked at).
    fn decides(&self) -> bool {
        matches!(self, Outcome::Used | Outcome::Failed(_) | Outcome::NotRun)
    }
}

/// The result of looking for a key through a list of sources.
pub(crate) struct Lookup {
    pub(crate) key: Option<String>,
    pub(crate) checked: Vec<(Source, Outcome)>,
}

impl Lookup {
    /// The source the key came from.
    pub(crate) fn used(&self) -> Option<&Source> {
        self.checked
            .iter()
            .find(|(_, o)| *o == Outcome::Used)
            .map(|(s, _)| s)
    }

    /// The source the key comes from: the one used, else a command that
    /// wasn't run (it will be when a request needs the key).
    pub(crate) fn source(&self) -> Option<&Source> {
        self.used().or_else(|| {
            self.checked
                .iter()
                .find(|(_, o)| *o == Outcome::NotRun)
                .map(|(s, _)| s)
        })
    }

    /// The source whose read failed, and why.
    pub(crate) fn failure(&self) -> Option<(&Source, &str)> {
        self.checked.iter().find_map(|(s, o)| match o {
            Outcome::Failed(e) => Some((s, e.as_str())),
            _ => None,
        })
    }

    /// `$OPENAI_API_KEY (not set), providers/openai/api-key (no file)`.
    pub(crate) fn summary(&self) -> String {
        self.checked
            .iter()
            .map(|(s, o)| match o.note() {
                Some(n) => format!("{} ({n})", s.describe()),
                None => s.describe(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Each source as `{kind, from, state, note?}`.
    pub(crate) fn to_value(&self) -> Value {
        Value::Array(
            self.checked
                .iter()
                .map(|(s, o)| {
                    let mut v =
                        json!({ "kind": s.kind(), "from": s.describe(), "state": o.state() });
                    if let Some(n) = o.note() {
                        v["note"] = Value::from(n);
                    }
                    v
                })
                .collect(),
        )
    }
}

/// How a lookup treats commands.
#[derive(Clone, Debug, Default)]
pub(crate) struct LookupOpts {
    /// Run `command:` sources (else they're `not run`, and decide the lookup).
    pub(crate) run_commands: bool,
    /// Config files a `command:` may come from: a command isn't run when one
    /// of them can be changed by other users.
    pub(crate) guard: Vec<PathBuf>,
}

/// Look through `sources` in order for a key, running commands.
#[cfg(test)]
pub(crate) fn lookup(home: &Path, sources: &[Source]) -> Lookup {
    lookup_with(
        home,
        sources,
        &LookupOpts {
            run_commands: true,
            guard: Vec::new(),
        },
    )
}

/// Look through `sources` in order for a key.
pub(crate) fn lookup_with(home: &Path, sources: &[Source], opts: &LookupOpts) -> Lookup {
    let mut key = None;
    let mut checked: Vec<(Source, Outcome)> = Vec::new();
    for source in sources {
        if checked.iter().any(|(_, o)| o.decides()) {
            checked.push((source.clone(), Outcome::NotChecked));
            continue;
        }
        let outcome = match source {
            Source::Env(var) => match std::env::var(var) {
                Ok(v) if !v.trim().is_empty() => {
                    key = Some(v.trim().to_string());
                    Outcome::Used
                }
                _ => Outcome::Missing("not set"),
            },
            Source::File(raw) | Source::Saved(raw) => {
                let path = file_path(home, raw);
                if raw.is_empty() || !path.exists() {
                    Outcome::Missing(if matches!(source, Source::Saved(_)) {
                        "none saved"
                    } else {
                        "no file"
                    })
                } else {
                    match std::fs::read_to_string(&path) {
                        Ok(v) if !v.trim().is_empty() => {
                            key = Some(v.trim().to_string());
                            Outcome::Used
                        }
                        Ok(_) => Outcome::Missing("empty"),
                        Err(e) => Outcome::Failed(e.to_string()),
                    }
                }
            }
            Source::Command(argv) if !opts.run_commands => match cached(argv) {
                Some(k) => {
                    key = Some(k);
                    Outcome::Used
                }
                None => Outcome::NotRun,
            },
            Source::Command(argv) => match run_command(home, argv, &opts.guard) {
                Ok(Some(k)) => {
                    key = Some(k);
                    Outcome::Used
                }
                Ok(None) => Outcome::Missing("printed nothing"),
                Err(e) => Outcome::Failed(e),
            },
            Source::Literal(k) => {
                key = Some(k.trim().to_string());
                Outcome::Used
            }
        };
        checked.push((source.clone(), outcome));
    }
    Lookup { key, checked }
}

/// How long a `command:` source may take — long enough for a Touch ID or
/// unlock prompt.
const COMMAND_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(60)
};

/// Keys commands printed, by argv, for the life of the process (so a Touch
/// ID prompt comes once, not once per request).
fn command_cache() -> &'static Mutex<HashMap<Vec<String>, String>> {
    static CACHE: OnceLock<Mutex<HashMap<Vec<String>, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(argv: &[String]) -> Option<String> {
    command_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(argv).cloned())
}

/// Forget the keys commands printed (after a key setting changes, or to
/// check a key afresh).
pub(crate) fn forget_commands() {
    if let Ok(mut c) = command_cache().lock() {
        c.clear();
    }
}

/// Why a command from these config files mustn't run: one can be changed by
/// other users (so they could make Lexi run anything as you).
fn unsafe_config(guard: &[PathBuf]) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in guard {
            if let Ok(meta) = std::fs::metadata(f) {
                if meta.permissions().mode() & 0o022 != 0 {
                    return Some(format!(
                        "Lexi won't run commands from {}: other users can change it (run `chmod go-w {}`)",
                        f.display(),
                        f.display()
                    ));
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = guard;
    None
}

/// Run a `command:` source: its trimmed output is the key (`None` when it
/// prints nothing); a failure to start, a non-zero exit or a timeout is an
/// error that shows what it wrote to stderr (never its output).
fn run_command(home: &Path, argv: &[String], guard: &[PathBuf]) -> Result<Option<String>, String> {
    if let Some(k) = cached(argv) {
        return Ok(Some(k));
    }
    if let Some(why) = unsafe_config(guard) {
        return Err(why);
    }
    let run = || run_command_now(home, argv);
    // Waiting blocks; on a multi-threaded runtime, let its other tasks move
    // to another thread meanwhile.
    let out = match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(run)
        }
        _ => run(),
    }?;
    if let Some(k) = &out {
        if let Ok(mut c) = command_cache().lock() {
            c.insert(argv.to_vec(), k.clone());
        }
    }
    Ok(out)
}

fn run_command_now(home: &Path, argv: &[String]) -> Result<Option<String>, String> {
    use std::process::{Command, Stdio};
    let program = &argv[0];
    let args: Vec<PathBuf> = argv[1..]
        .iter()
        .map(|a| match a.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
            None => PathBuf::from(a),
        })
        .collect();
    let mut child = Command::new(file_path_for_program(program))
        .args(&args)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run `{program}`: {e}"))?;
    let read = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            buf
        })
    };
    let stdout = read(Box::new(child.stdout.take().expect("piped")));
    let stderr = read(Box::new(child.stderr.take().expect("piped")));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= COMMAND_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`{program}` didn't finish within {} seconds",
                    COMMAND_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("`{program}`: {e}")),
        }
    };
    let out = String::from_utf8_lossy(&stdout.join().unwrap_or_default()).to_string();
    if !status.success() {
        let err = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).to_string();
        let err: Vec<&str> = err
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .take(3)
            .collect();
        let what = match status.code() {
            Some(c) => format!("exited with status {c}"),
            None => "was stopped".to_string(),
        };
        return Err(if err.is_empty() {
            format!("`{program}` {what}")
        } else {
            format!("`{program}` {what}: {}", err.join(" "))
        });
    }
    let key = out.trim();
    Ok((!key.is_empty()).then(|| key.to_string()))
}

/// A command's program: `~/…` is under your home folder; anything else as
/// written (a bare name is looked up on `PATH`).
fn file_path_for_program(program: &str) -> PathBuf {
    match program.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(program),
    }
}
