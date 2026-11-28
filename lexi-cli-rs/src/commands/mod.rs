//! Command handlers: each turns parsed arguments into `lexi-host` calls and
//! reports through [`Io`]. One module per command group.

mod api;
mod chat;
pub(crate) mod config;
mod embed;
mod jobs;
mod models;
mod prompts;
mod providers;
mod respond;
pub(crate) mod shell_cmds;

use crate::cli::Cmd;
use crate::output::{Io, RunOptions};

/// Take `--timeout SECS` (or `--timeout=SECS`) out of a free-form argument
/// list: the time limit for this command's provider requests.
fn take_timeout(args: &mut Vec<String>) -> Result<Option<u64>, String> {
    let Some(pos) = args
        .iter()
        .position(|a| a == "--timeout" || a.starts_with("--timeout="))
    else {
        return Ok(None);
    };
    let flag = args.remove(pos);
    let value = match flag.strip_prefix("--timeout=") {
        Some(v) => v.to_string(),
        None if pos < args.len() => args.remove(pos),
        None => return Err("--timeout needs a number of seconds (0 = no limit)".into()),
    };
    value.trim().parse().map(Some).map_err(|_| {
        format!("--timeout is a whole number of seconds (0 = no limit), not {value:?}")
    })
}

/// Take `--key NAME` (or `--key=NAME`) out of a free-form argument list:
/// the provider's named key this command's requests use.
fn take_key(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let Some(pos) = args
        .iter()
        .position(|a| a == "--key" || a.starts_with("--key="))
    else {
        return Ok(None);
    };
    let flag = args.remove(pos);
    match flag.strip_prefix("--key=") {
        Some(v) => Ok(Some(v.to_string())),
        None if pos < args.len() => Ok(Some(args.remove(pos))),
        None => Err("--key needs the name of one of the provider's keys".into()),
    }
}

/// A request's provider argument and `--key`: the provider, the key (if
/// any) and the label to show for them (`openai#work`).
pub(crate) fn request_key(
    io: &mut Io<'_>,
    provider: &str,
    key: Option<&str>,
) -> Result<(String, Option<String>, String), u8> {
    let (name, key) = lexi_host::config::split_key_arg(provider, key).map_err(|e| io.fail(e))?;
    let label = match &key {
        Some(k) => format!("{name}#{k}"),
        None => name.clone(),
    };
    Ok((name, key, label))
}

/// Refuse `provider#key` where a command changes the provider itself.
pub(crate) fn plain_provider(io: &mut Io<'_>, provider: &str) -> Result<(), u8> {
    match provider.split_once('#') {
        Some((p, k)) => Err(io.fail(format!(
            "this command works on the provider itself: leave out #{k} (use {p})"
        ))),
        None => Ok(()),
    }
}

/// Run `fut` with `timeout` as its time limit, if one was given.
async fn timed<F: std::future::Future<Output = u8>>(timeout: Option<u64>, fut: F) -> u8 {
    match timeout {
        Some(secs) => lexi_host::with_request_timeout(secs, fut).await,
        None => fut.await,
    }
}

pub(crate) async fn dispatch(cmd: Cmd, io: &mut Io<'_>, opts: RunOptions) -> u8 {
    match cmd {
        Cmd::Respond { provider, mut args } => match (take_timeout(&mut args), take_key(&mut args))
        {
            (Ok(t), Ok(key)) => match request_key(io, &provider, key.as_deref()) {
                Ok((p, key, label)) => {
                    let run = respond::respond(io, &p, &label, &args);
                    lexi_host::config::with_key(key, timed(t, run)).await
                }
                Err(code) => code,
            },
            (Err(e), _) | (_, Err(e)) => io.fail(e),
        },
        Cmd::Generate {
            provider,
            prompt,
            model,
            out,
            timeout,
            key,
        } => match request_key(io, &provider, key.as_deref()) {
            Ok((p, key, label)) => {
                let run = chat::generate(io, p, label, prompt, model, out);
                lexi_host::config::with_key(key, timed(timeout, run)).await
            }
            Err(code) => code,
        },
        Cmd::Embed { provider, mut args } => match (take_timeout(&mut args), take_key(&mut args)) {
            (Ok(t), Ok(key)) => match request_key(io, &provider, key.as_deref()) {
                Ok((p, key, _)) => {
                    lexi_host::config::with_key(key, timed(t, embed::embed(io, &p, &args))).await
                }
                Err(code) => code,
            },
            (Err(e), _) | (_, Err(e)) => io.fail(e),
        },
        Cmd::Upload {
            provider,
            file,
            key,
        } => match request_key(io, &provider, key.as_deref()) {
            Ok((p, key, label)) => {
                lexi_host::config::with_key(key, chat::upload(io, p, label, file)).await
            }
            Err(code) => code,
        },
        Cmd::Jobs { cmd } => jobs::run(io, cmd).await,
        Cmd::Tags => models::vocabulary(io),
        Cmd::Version => chat::version(io),
        Cmd::Help { topic } => shell_cmds::help(io, topic, opts).await,
        Cmd::Alias { cmd } => shell_cmds::alias(io, cmd),
        Cmd::History { count, clear } => shell_cmds::history(io, count, clear),
        Cmd::Sh { command } => shell_cmds::sh(io, &command.join(" "), opts),
        Cmd::Exit => crate::output::EXIT_OK,
        Cmd::Config { cmd } => config::run(io, cmd),
        Cmd::Prompts { cmd } => prompts::run(io, cmd),
        Cmd::Providers { cmd } => providers::run(io, cmd).await,
        Cmd::Models { cmd } => models::run(io, cmd).await,
        Cmd::Api { cmd } => api::run(io, cmd),
    }
}

/// Read a file under the Lexi home for `--raw` display.
pub(crate) fn read_home_file(rel: &str) -> std::io::Result<String> {
    std::fs::read_to_string(lexi_host::config::lexi_home().join(rel))
}
