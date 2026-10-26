use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod http_client;
mod secrets;
mod session_end;
mod webhook_format;
mod worktree;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    End(session_end::SessionEndArgs),
}

#[derive(Debug)]
struct Error(&'static str);

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "session-end: {}", self.0)
    }
}

impl std::error::Error for Error {}

fn failure(message: &'static str) -> Error {
    Error(message)
}

fn main() -> ExitCode {
    let Command::End(args) = Cli::parse().command;
    match session_end::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
