use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod support;

type TestError = Box<dyn std::error::Error + Send + Sync>;
type Received = (String, Value);
type Exchange = (std::process::Output, Option<Received>);
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const TOKEN: &str = "fixture-private-token";

fn command(fixture: &support::Fixture, source: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-session-hook"));
    command
        .args(["end", "--source", source])
        .env_clear()
        .env("HOME", fixture.home())
        .current_dir(fixture.repository());
    command
}

fn configure(
    fixture: &support::Fixture,
    url: &str,
    source: &str,
) -> Result<Command, Box<dyn std::error::Error + Send + Sync>> {
    fixture.write_home(".config/agent-session-hook/webhooks.toml", "[webhooks.routine]\nsource = 'environment'\nurl_env = 'TEST_WEBHOOK_URL'\ntoken_env = 'TEST_WEBHOOK_TOKEN'\n")?;
    let mut command = command(fixture, source);
    command
        .env("AGENT_SESSION_HOOK_WEBHOOK", "routine")
        .env("TEST_WEBHOOK_URL", url)
        .env("TEST_WEBHOOK_TOKEN", TOKEN);
    Ok(command)
}

fn request(
    stream: TcpStream,
    status: &str,
) -> Result<(String, Value), Box<dyn std::error::Error + Send + Sync>> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream);
    let mut headers = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err("incomplete HTTP headers".into());
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse::<usize>()?;
        }
        headers.push_str(&line);
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    if status == "silent" {
        let mut remaining = Vec::new();
        reader.read_to_end(&mut remaining)?;
        return Ok((headers, serde_json::from_slice(&body)?));
    }
    write!(
        reader.get_mut(),
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    Ok((headers, serde_json::from_slice(&body)?))
}

#[test]
fn codex_notify_posts_only_worktree_event_and_time() -> TestResult {
    let fixture = support::Fixture::new()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/private-fixture-url", listener.local_addr()?);
    let before = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut child = configure(&fixture, &url, "codex")?
        .arg(json!({"type":"agent-turn-complete", "cwd":fixture.repository(), "last-assistant-message":"private session", "input-messages":["private prompt"]}).to_string())
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let received = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(request(stream, "204 No Content")?),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child.try_wait()?.is_some() || std::time::Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (headers, body) = received.ok_or("webhook was not received")?;
    assert!(headers.starts_with("POST /private-fixture-url HTTP/1.1\r\n"));
    assert!(
        headers
            .to_ascii_lowercase()
            .contains(&format!("authorization: Bearer {TOKEN}").to_ascii_lowercase())
    );
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: application/json\r\n")
    );
    let timestamp = body
        .get("time")
        .and_then(Value::as_u64)
        .ok_or("missing timestamp")?;
    assert!(
        timestamp >= before && timestamp <= SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    );
    assert_eq!(
        body,
        json!({"worktree":fixture.repository().canonicalize()?,"event":"Stop","time":timestamp})
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn no_declaration_skips_input_configuration_and_network() -> TestResult {
    let fixture = support::Fixture::new()?;
    fixture.write_home(
        ".config/agent-session-hook/webhooks.toml",
        "malformed and private",
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    for declaration in [None, Some("")] {
        let mut command = command(&fixture, "codex");
        command
            .arg("not JSON")
            .env(
                "TEST_WEBHOOK_URL",
                format!("http://{}", listener.local_addr()?),
            )
            .env("TEST_WEBHOOK_TOKEN", TOKEN);
        if let Some(name) = declaration {
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

fn exchange(
    listener: &TcpListener,
    command: &mut Command,
    input: Option<&Value>,
    status: &str,
) -> Result<Exchange, TestError> {
    listener.set_nonblocking(true)?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("stdin unavailable")?;
    if let Some(input) = input {
        stdin.write_all(input.to_string().as_bytes())?;
    }
    drop(stdin);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let received = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(request(stream, status)?),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child.try_wait()?.is_some() {
                    break None;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill()?;
                    return Err("hook did not terminate".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let output = child.wait_with_output()?;
    assert_eq!(
        listener.accept().err().ok_or("duplicate webhook")?.kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok((output, received))
}

#[test]
fn native_hooks_and_herdr_lifecycle_post_minimal_events() -> TestResult {
    for (source, event) in [
        ("codex", "Stop"),
        ("claude", "Stop"),
        ("claude", "Notification"),
        ("herdr", "done"),
        ("herdr", "exited"),
    ] {
        let fixture = support::Fixture::new()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/callback", listener.local_addr()?);
        let mut command = configure(&fixture, &url, source)?;
        let input = if source == "herdr" {
            command
                .args(["--event", event, "--worktree"])
                .arg(fixture.repository());
            None
        } else {
            Some(
                json!({"hook_event_name":event, "cwd":fixture.repository(), "message":"private notification", "last_assistant_message":"private session", "transcript_path":"/private/transcript", "notification_type":"permission_prompt"}),
            )
        };
        let (output, received) =
            exchange(&listener, &mut command, input.as_ref(), "204 No Content")?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        let (_, body) = received.ok_or("missing webhook")?;
        let timestamp = body
            .get("time")
            .and_then(Value::as_u64)
            .ok_or("missing timestamp")?;
        assert_eq!(
            body,
            json!({"worktree":fixture.repository().canonicalize()?,"event":event,"time":timestamp})
        );
    }
    Ok(())
}

#[test]
fn invalid_input_configuration_and_credentials_never_send_or_leak() -> TestResult {
    for case in [
        "unknown-name",
        "invalid-name",
        "missing-url",
        "missing-token",
        "empty-token",
        "header-injection",
        "insecure-url",
        "invalid-url",
        "fragment",
        "userinfo",
        "malformed-config",
        "unknown-field",
        "malformed-input",
        "unsupported-event",
        "relative-worktree",
        "missing-worktree",
        "wrong-shape",
        "oversized-input",
    ] {
        let fixture = support::Fixture::new()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/private-fixture-url", listener.local_addr()?);
        let mut command = configure(&fixture, &url, "codex")?;
        let mut input = json!({"hook_event_name":"Stop","cwd":fixture.repository()}).to_string();
        match case {
            "unknown-name" => { command.env("AGENT_SESSION_HOOK_WEBHOOK", "unknown"); }
            "invalid-name" => { command.env("AGENT_SESSION_HOOK_WEBHOOK", "../private"); }
            "missing-url" => { command.env_remove("TEST_WEBHOOK_URL"); }
            "missing-token" => { command.env_remove("TEST_WEBHOOK_TOKEN"); }
            "empty-token" => { command.env("TEST_WEBHOOK_TOKEN", ""); }
            "header-injection" => { command.env("TEST_WEBHOOK_TOKEN", "private\r\nInjected: credential"); }
            "insecure-url" => { command.env("TEST_WEBHOOK_URL", "http://example.invalid/private-fixture-url"); }
            "invalid-url" => { command.env("TEST_WEBHOOK_URL", "private-invalid-url"); }
            "fragment" => { command.env("TEST_WEBHOOK_URL", format!("{url}#private")); }
            "userinfo" => { command.env("TEST_WEBHOOK_URL", format!("http://private@{}/", listener.local_addr()?)); }
            "malformed-config" => fixture.write_home(".config/agent-session-hook/webhooks.toml", "private invalid configuration")?,
            "unknown-field" => fixture.write_home(".config/agent-session-hook/webhooks.toml", "[webhooks.routine]\nsource = 'environment'\nurl_env = 'TEST_WEBHOOK_URL'\ntoken_env = 'TEST_WEBHOOK_TOKEN'\nprivate = 'session'\n")?,
            "malformed-input" => input = "private malformed input".to_owned(),
            "unsupported-event" => input = json!({"hook_event_name":"PreToolUse","cwd":fixture.repository()}).to_string(),
            "relative-worktree" => input = json!({"hook_event_name":"Stop","cwd":"relative"}).to_string(),
            "missing-worktree" => input = json!({"hook_event_name":"Stop","cwd":"/does-not-exist-private"}).to_string(),
            "wrong-shape" => input = json!({"hook_event_name":{},"cwd":fixture.repository()}).to_string(),
            "oversized-input" => input = "x".repeat(1_048_577),
            _ => return Err("unknown test case".into()),
        }
        let input_file = tempfile::tempfile()?;
        let mut writer = &input_file;
        writer.write_all(input.as_bytes())?;
        writer.seek(SeekFrom::Start(0))?;
        command.stdin(input_file);
        let (output, received) = if case == "oversized-input" {
            listener.set_nonblocking(true)?;
            (command.output()?, None)
        } else {
            command.arg(&input);
            exchange(&listener, &mut command, None, "204 No Content")?
        };
        assert!(!output.status.success(), "accepted {case}");
        assert!(received.is_none(), "sent {case}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr)?;
        assert!(stderr.starts_with("session-end:"), "{case}");
        assert!(!stderr.contains("private"), "{case}");
        assert!(!stderr.contains(TOKEN), "{case}");
        assert!(!stderr.contains(&url), "{case}");
    }
    Ok(())
}

#[test]
fn http_errors_and_redirects_are_sanitized_and_not_retried() -> TestResult {
    for status in [
        "401 Unauthorized",
        "429 Too Many Requests",
        "500 Internal Server Error",
        "302 Found",
    ] {
        let fixture = support::Fixture::new()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let target = TcpListener::bind("127.0.0.1:0")?;
        target.set_nonblocking(true)?;
        let url = format!("http://{}/private-fixture-url", listener.local_addr()?);
        let status = format!(
            "{status}\r\nLocation: http://{}/private-redirect",
            target.local_addr()?
        );
        let mut command = configure(&fixture, &url, "codex")?;
        command.arg(json!({"hook_event_name":"Stop","cwd":fixture.repository()}).to_string());
        let (output, received) = exchange(&listener, &mut command, None, &status)?;
        assert!(!output.status.success());
        assert!(received.is_some());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr)?;
        assert!(stderr.starts_with("session-end: webhook"));
        assert!(!stderr.contains("private"));
        assert!(!stderr.contains(TOKEN));
        assert_eq!(
            target.accept().err().ok_or("redirect followed")?.kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    Ok(())
}

#[test]
fn complete_native_input_does_not_wait_for_stdin_to_close() -> TestResult {
    let fixture = support::Fixture::new()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/callback", listener.local_addr()?);
    let mut child = configure(&fixture, &url, "claude")?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.as_mut().ok_or("stdin unavailable")?.write_all(
        json!({"hook_event_name":"Stop","cwd":fixture.repository()})
            .to_string()
            .as_bytes(),
    )?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let received = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(request(stream, "204 No Content")?),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child.try_wait()?.is_some() || std::time::Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    if received.is_none() {
        child.kill()?;
    }
    let output = child.wait_with_output()?;
    assert!(
        received.is_some(),
        "hook waited for EOF after a complete event"
    );
    assert!(output.status.success());
    Ok(())
}

#[test]
fn stalled_and_unavailable_http_fail_privately() -> TestResult {
    let fixture = support::Fixture::new()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}/private-fixture-url", listener.local_addr()?);
    let mut command = configure(&fixture, &url, "codex")?;
    command.arg(json!({"hook_event_name":"Stop","cwd":fixture.repository()}).to_string());
    let started = std::time::Instant::now();
    let (output, received) = exchange(&listener, &mut command, None, "silent")?;
    assert!(received.is_some());
    assert!(!output.status.success());
    assert!(started.elapsed() >= Duration::from_millis(2500));
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "session-end: webhook delivery failed\n"
    );
    drop(listener);
    let output = command.output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "session-end: webhook delivery failed\n"
    );
    Ok(())
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
fn session_cwd_is_canonicalized_without_becoming_git_root() -> TestResult {
    let fixture = support::Fixture::new()?;
    let status = Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .arg(fixture.repository())
        .env_clear()
        .env("HOME", fixture.home())
        .status()?;
    assert!(status.success());
    let nested = fixture.repository().join("nested");
    std::fs::create_dir(&nested)?;
    let alias = fixture.home().join("alias");
    std::os::unix::fs::symlink(&nested, &alias)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut command = configure(
        &fixture,
        &format!("http://{}/callback", listener.local_addr()?),
        "codex",
    )?;
    command.arg(json!({"hook_event_name":"Stop", "cwd":alias}).to_string());
    let (output, received) = exchange(&listener, &mut command, None, "204 No Content")?;
    assert!(output.status.success());
    let (_, body) = received.ok_or("missing webhook")?;
    assert_eq!(body.get("worktree"), Some(&json!(nested.canonicalize()?)));
    Ok(())
}

#[test]
fn environment_proxies_do_not_receive_webhooks() -> TestResult {
    let fixture = support::Fixture::new()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let proxy = TcpListener::bind("127.0.0.1:0")?;
    proxy.set_nonblocking(true)?;
    let mut command = configure(
        &fixture,
        &format!("http://{}/callback", listener.local_addr()?),
        "codex",
    )?;
    let proxy_url = format!("http://{}", proxy.local_addr()?);
    command
        .env("http_proxy", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("NO_PROXY", "")
        .env("no_proxy", "");
    command.arg(json!({"hook_event_name":"Stop", "cwd":fixture.repository()}).to_string());
    let (output, received) = exchange(&listener, &mut command, None, "204 No Content")?;
    assert!(output.status.success());
    assert!(received.is_some());
    assert_eq!(
        proxy
            .accept()
            .err()
            .ok_or("proxy received credentials")?
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}
