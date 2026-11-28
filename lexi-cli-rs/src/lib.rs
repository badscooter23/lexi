//! Lexi CLI library — the command tree, parser, dispatcher, shell and output
//! behind the `lexi` binary, over `lexi-host` (which is itself over the pure
//! `lexi-core` engine). It reads and writes the same `~/.lexi-cli/` as the
//! desktop app.
//!
//! [`run`] takes argv plus output writers instead of touching the process's
//! stdio, and [`execute_line`] runs one shell line, so in-process hosts (tests,
//! the desktop app's CLI console) run exactly what a terminal user types and
//! capture what it prints.

mod aliases;
pub mod cli;
mod coerce;
mod commands;
mod history;
pub mod output;
mod shell;

use std::ffi::OsString;

use clap::Parser;

pub use cli::Cli;
pub use output::InputFn;
pub use output::{Io, RunOptions, Writer, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
pub use shell::{execute_line, LineOutcome};

/// The shell history (`~/.lexi-cli/history`), oldest first — what ↑ walks
/// through in the terminal shell and the desktop app's CLI console.
pub fn history_entries() -> Vec<String> {
    history::load()
}

/// Parse `args` (argv, including the program name) and run the command,
/// writing to `out`/`err`. With no command, runs the interactive shell.
/// Returns the process exit code: [`EXIT_OK`], [`EXIT_FAILURE`] (the command
/// failed) or [`EXIT_USAGE`] (bad arguments).
pub async fn run<I, T>(args: I, out: Writer<'_>, err: Writer<'_>) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    run_with(args, out, err, RunOptions::default()).await
}

/// [`run`] with explicit [`RunOptions`].
pub async fn run_with<I, T>(args: I, out: Writer<'_>, err: Writer<'_>, opts: RunOptions) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut io = Io::new(out, err, false);
    io.color = opts.color;
    if let Err(e) = lexi_host::ensure_home() {
        io.warn(format_args!("could not prepare the Lexi home: {e}"));
    }
    for notice in lexi_host::take_notices() {
        io.notice(notice);
    }
    let mut argv: Vec<String> = args
        .into_iter()
        .map(|a| a.into().to_string_lossy().into_owned())
        .collect();
    if argv.is_empty() {
        argv.push("lexi".into());
    }
    let rest = match shell::expand_aliases(argv.split_off(1)) {
        Ok(r) => r,
        Err(e) => return io.fail(e),
    };
    argv.extend(rest);
    // No command: the interactive shell.
    if let Ok(Cli { cmd: None, json }) = Cli::try_parse_from(&argv) {
        io.json = json;
        return shell::repl(&mut io, opts).await;
    }
    run_parsed(argv, &mut io, opts).await
}

/// Parse a full argv and dispatch it (no shell, no alias expansion). Clap's
/// own output (`--help`, usage errors) goes to `io`.
pub(crate) async fn run_parsed(argv: Vec<String>, io: &mut Io<'_>, opts: RunOptions) -> u8 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            // --help / --version are "errors" to clap but successes to us.
            let text = e.render().to_string();
            if e.use_stderr() {
                let _ = io.err.write_all(text.as_bytes());
                let _ = io.err.flush();
                return EXIT_USAGE;
            }
            let _ = io.out.write_all(text.as_bytes());
            let _ = io.out.flush();
            return EXIT_OK;
        }
    };
    let Some(cmd) = cli.cmd else {
        let _ = writeln!(io.err, "error: no command given (try `help`)");
        return EXIT_USAGE;
    };
    let json = cli.json || io.json;
    let saved = std::mem::replace(&mut io.json, json);
    let code = commands::dispatch(cmd, io, opts).await;
    io.json = saved;
    code
}
