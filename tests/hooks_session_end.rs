use serde_json::json;
use std::io::Write;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::Duration;

mod support;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn command(fixture: &support::Fixture, source: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-session-hook"));
    command
        .args(["end", "--source", source])
        .env_clear()
        .env("HOME", fixture.home())
        .current_dir(fixture.repository());
    command
}
#[test]
fn undeclared_sources_do_not_wait_for_input_or_require_home() -> TestResult {
    for source in ["codex", "claude", "herdr"] {
        for declaration in [None, Some("")] {
            let fixture = support::Fixture::new()?;
            let mut command = command(&fixture, source);
            command.env_remove("HOME");
            if let Some(name) = declaration {
                command.env("AGENT_SESSION_HOOK_WEBHOOK", name);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while child.try_wait()?.is_none() {
                if std::time::Instant::now() >= deadline {
                    child.kill()?;
                    child.wait()?;
                    return Err("undeclared hook waited for input".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output()?;
            assert!(output.status.success());
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        }
    }
    Ok(())
}

#[test]
fn invalid_configuration_fails_without_outputting_values() -> TestResult {
    for configuration in [
        "private invalid configuration",
        "[webhooks.routine]\nsource='environment'\nurl_env='PRIVATE_URL'\ntoken_env='PRIVATE_TOKEN'",
        "[webhooks.other]\nsource='keychain'\nservice='private'\naccount='webhook'",
        "[webhooks.routine]\nsource='unknown-private-source'\nservice='private'\naccount='webhook'",
        "[webhooks.routine]\nsource='keychain'\nservice='private'\naccount='webhook'\nprivate='unexpected'",
        "[webhooks.routine]\nsource='keychain'\nservice='private'",
    ] {
        let fixture = support::Fixture::new()?;
        fixture.write_home(".config/agent-session-hook/webhooks.toml", configuration)?;
        let output = command(&fixture, "codex")
            .env("AGENT_SESSION_HOOK_WEBHOOK", "routine")
            .arg("invalid input")
            .output()?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr)?;
        assert!(error.starts_with("session-end:"));
        assert!(!error.contains("private"));
    }
    Ok(())
}

#[test]
fn no_declaration_skips_invalid_configuration_and_network() -> TestResult {
    let fixture = support::Fixture::new()?;
    fixture.write_home(
        ".config/agent-session-hook/webhooks.toml",
        "private invalid configuration",
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    for name in [None, Some("")] {
        let mut command = command(&fixture, "codex");
        command.arg("invalid JSON");
        if let Some(name) = name {
            command.env("AGENT_SESSION_HOOK_WEBHOOK", name);
        }
        let output = command.output()?;
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    assert_eq!(
        listener.accept().err().ok_or("unexpected webhook")?.kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}

#[test]
fn complete_stdin_input_is_parsed_without_waiting_for_eof() -> TestResult {
    let fixture = support::Fixture::new()?;
    fixture.write_home(
        ".config/agent-session-hook/webhooks.toml",
        "[webhooks.routine]\nsource='keychain'\nservice='fixture'\naccount='webhook'",
    )?;
    let mut child = command(&fixture, "claude")
        .env("AGENT_SESSION_HOOK_WEBHOOK", "routine")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.as_mut().ok_or("missing stdin")?.write_all(
        json!({"hook_event_name":"Stop","cwd":fixture.repository().join("missing")})
            .to_string()
            .as_bytes(),
    )?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while child.try_wait()?.is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("hook waited for EOF".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "session-end: worktree must be an existing absolute directory\n"
    );
    Ok(())
}
