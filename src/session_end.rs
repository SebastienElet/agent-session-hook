use crate::{Error, failure};
use clap::{Args, ValueEnum};
use serde::Deserialize;
use std::io::Read;
use std::path::PathBuf;

#[derive(Clone, Copy, ValueEnum)]
pub enum Source {
    Codex,
    Claude,
    Herdr,
}

#[derive(Args)]
pub struct SessionEndArgs {
    #[arg(long, value_enum)]
    pub source: Source,
    #[arg(long)]
    pub event: Option<String>,
    #[arg(long)]
    pub worktree: Option<PathBuf>,
    pub notification: Option<String>,
}

#[derive(Deserialize)]
struct NativeEvent {
    cwd: PathBuf,
    hook_event_name: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

pub(crate) fn run(args: SessionEndArgs) -> Result<(), Error> {
    let Some(name) = std::env::var_os("AGENT_SESSION_HOOK_WEBHOOK") else {
        return Ok(());
    };
    if name.is_empty() {
        return Ok(());
    }
    let name = name
        .to_str()
        .ok_or_else(|| failure("invalid webhook name"))?;
    let source = crate::secrets::configured_source(name)?;
    deliver(args, std::io::stdin(), || source.resolve())
}

fn deliver(
    args: SessionEndArgs,
    input: impl Read,
    credentials: impl FnOnce() -> Result<crate::secrets::Credentials, Error>,
) -> Result<(), Error> {
    let payload = payload(args, input)?;
    let credentials = credentials()?;
    let request = payload.encode(&credentials.token)?;
    crate::http_client::send(&credentials.url, &request)
}

fn payload(
    args: SessionEndArgs,
    input: impl Read,
) -> Result<crate::webhook_format::Payload, Error> {
    let (worktree, event) = match args.source {
        Source::Herdr => {
            if args.notification.is_some() {
                return Err(failure("unexpected herdr input"));
            }
            let event = match args.event.as_deref() {
                Some("done") => "done",
                Some("exited") => "exited",
                _ => return Err(failure("herdr requires done or exited")),
            };
            (
                args.worktree
                    .ok_or_else(|| failure("herdr requires a worktree"))?,
                event,
            )
        }
        Source::Codex | Source::Claude => {
            if args.event.is_some() || args.worktree.is_some() {
                return Err(failure("native hooks require their original input"));
            }
            let native: NativeEvent = if let Some(input) = args.notification {
                if !matches!(args.source, Source::Codex) {
                    return Err(failure("Claude requires stdin input"));
                }
                if input.len() > 1_048_576 {
                    return Err(failure("hook input is too large"));
                }
                serde_json::from_str(&input).map_err(|_| failure("invalid hook input"))?
            } else {
                NativeEvent::deserialize(&mut serde_json::Deserializer::from_reader(
                    input.take(1_048_576),
                ))
                .map_err(|_| failure("invalid hook input"))?
            };
            let event = match (
                args.source,
                native.hook_event_name.as_deref(),
                native.kind.as_deref(),
            ) {
                (Source::Codex | Source::Claude, Some("Stop"), None)
                | (Source::Codex, None, Some("agent-turn-complete")) => "Stop",
                (Source::Claude, Some("Notification"), None) => "Notification",
                _ => return Err(failure("unsupported hook event")),
            };
            (native.cwd, event)
        }
    };
    crate::webhook_format::Payload::new(crate::worktree::resolve(worktree)?, event)
}

#[cfg(test)]
mod tests;
