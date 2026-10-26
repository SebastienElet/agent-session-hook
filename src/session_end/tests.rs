use super::{SessionEndArgs, Source, deliver};
use crate::{Error, secrets::Credentials};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type TestError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), TestError>;
type Received = (String, Value);
const TOKEN: &str = "fixture-private-token";

fn args(source: Source, notification: Option<String>) -> SessionEndArgs {
    SessionEndArgs {
        source,
        event: None,
        worktree: None,
        notification,
    }
}

fn repository() -> std::io::Result<tempfile::TempDir> {
    let temporary = tempfile::tempdir()?;
    std::fs::create_dir(temporary.path().join(".git"))?;
    Ok(temporary)
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
    if status == "silent-body" {
        write!(
            reader.get_mut(),
            "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n"
        )?;
    }
    if status == "silent" || status == "silent-body" {
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

fn exchange(
    listener: &TcpListener,
    args: SessionEndArgs,
    input: Vec<u8>,
    credentials: Credentials,
    status: &str,
) -> Result<(Result<(), Error>, Option<Received>), TestError> {
    listener.set_nonblocking(true)?;
    let thread = std::thread::spawn(move || deliver(args, input.as_slice(), || Ok(credentials)));
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let received = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(request(stream, status)?),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if thread.is_finished() {
                    break None;
                }
                if std::time::Instant::now() >= deadline {
                    return Err("delivery did not terminate".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let result = thread.join().map_err(|_| "delivery panicked")?;
    assert_eq!(
        listener.accept().err().ok_or("duplicate webhook")?.kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok((result, received))
}

fn credentials(listener: &TcpListener) -> std::io::Result<Credentials> {
    Ok(Credentials {
        url: format!("http://{}/private-fixture-url", listener.local_addr()?),
        token: TOKEN.to_owned(),
    })
}

#[test]
fn native_sources_and_herdr_post_only_git_root_event_and_time() -> TestResult {
    for (source, event, notify) in [
        (Source::Codex, "Stop", true),
        (Source::Codex, "Stop", false),
        (Source::Claude, "Stop", false),
        (Source::Claude, "Notification", false),
        (Source::Herdr, "done", false),
        (Source::Herdr, "exited", false),
    ] {
        let repository = repository()?;
        let nested = repository.path().join("nested");
        std::fs::create_dir(&nested)?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let before = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let input = if notify {
            json!({"type":"agent-turn-complete","cwd":nested,"last-assistant-message":"private session"})
        } else {
            json!({"hook_event_name":event,"cwd":nested,"message":"private notification","last_assistant_message":"private session","transcript_path":"/private/transcript"})
        }.to_string();
        let mut args = args(source, notify.then_some(input.clone()));
        if matches!(source, Source::Herdr) {
            args.event = Some(event.to_owned());
            args.worktree = Some(nested);
        }
        let (result, received) = exchange(
            &listener,
            args,
            input.into_bytes(),
            credentials(&listener)?,
            "204 No Content",
        )?;
        result?;
        let (headers, body) = received.ok_or("webhook was not received")?;
        assert!(headers.starts_with("POST /private-fixture-url HTTP/1.1\r\n"));
        let headers = headers.to_ascii_lowercase();
        assert!(headers.contains(&format!("authorization: bearer {TOKEN}\r\n")));
        assert!(headers.contains("content-type: application/json\r\n"));
        let timestamp = body
            .get("time")
            .and_then(Value::as_u64)
            .ok_or("missing timestamp")?;
        assert!(
            timestamp >= before
                && timestamp <= SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
        );
        assert_eq!(
            body,
            json!({"worktree":repository.path().canonicalize()?,"event":event,"time":timestamp})
        );
    }
    Ok(())
}

#[test]
fn invalid_input_and_credentials_do_not_send_or_leak() -> TestResult {
    for case in [
        "empty-token",
        "header-injection",
        "insecure-url",
        "invalid-url",
        "fragment",
        "userinfo",
        "malformed-input",
        "unsupported-event",
        "relative-worktree",
        "missing-worktree",
        "non-repository",
        "wrong-shape",
        "oversized-input",
        "claude-notify",
        "native-flags",
        "herdr-missing-event",
        "herdr-missing-worktree",
        "herdr-notify",
    ] {
        let repository = repository()?;
        let outside = tempfile::tempdir()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let mut credentials = credentials(&listener)?;
        let mut input = json!({"hook_event_name":"Stop","cwd":repository.path()}).to_string();
        let mut args = args(Source::Codex, None);
        match case {
            "empty-token" => credentials.token.clear(),
            "header-injection" => credentials.token = "private\r\nInjected: credential".to_owned(),
            "insecure-url" => credentials.url = "http://example.invalid/private".to_owned(),
            "invalid-url" => credentials.url = "private-invalid-url".to_owned(),
            "fragment" => credentials.url.push_str("#private"),
            "userinfo" => credentials.url = format!("http://private@{}/", listener.local_addr()?),
            "malformed-input" => input = "private malformed input".to_owned(),
            "unsupported-event" => {
                input = json!({"hook_event_name":"PreToolUse","cwd":repository.path()}).to_string()
            }
            "relative-worktree" => {
                input = json!({"hook_event_name":"Stop","cwd":"relative"}).to_string()
            }
            "missing-worktree" => {
                input = json!({"hook_event_name":"Stop","cwd":repository.path().join("missing")})
                    .to_string()
            }
            "non-repository" => {
                input = json!({"hook_event_name":"Stop","cwd":outside.path()}).to_string()
            }
            "wrong-shape" => {
                input = json!({"hook_event_name":{},"cwd":repository.path()}).to_string()
            }
            "oversized-input" => input = "x".repeat(1_048_577),
            "claude-notify" => {
                args.source = Source::Claude;
                args.notification = Some(input.clone());
            }
            "native-flags" => args.event = Some("Stop".to_owned()),
            "herdr-missing-event" => {
                args.source = Source::Herdr;
                args.worktree = Some(repository.path().to_path_buf());
            }
            "herdr-missing-worktree" => {
                args.source = Source::Herdr;
                args.event = Some("done".to_owned());
            }
            "herdr-notify" => {
                args.source = Source::Herdr;
                args.notification = Some(input.clone());
            }
            _ => return Err("unknown case".into()),
        }
        let (result, received) = exchange(
            &listener,
            args,
            input.into_bytes(),
            credentials,
            "204 No Content",
        )?;
        let error = result.err().ok_or(format!("accepted {case}"))?.to_string();
        assert!(received.is_none(), "sent {case}");
        assert!(error.starts_with("session-end:"));
        assert!(!error.contains("private"));
        assert!(!error.contains(TOKEN));
    }
    Ok(())
}

#[test]
fn credential_failure_is_reported_before_delivery() -> TestResult {
    let repository = repository()?;
    let input = json!({"hook_event_name":"Stop","cwd":repository.path()}).to_string();
    let result = deliver(args(Source::Codex, Some(input)), std::io::empty(), || {
        Err(crate::failure("keychain lookup failed"))
    });
    assert_eq!(
        result.err().ok_or("missing error")?.to_string(),
        "session-end: keychain lookup failed"
    );
    Ok(())
}

#[test]
fn http_errors_and_redirects_are_private_without_retries() -> TestResult {
    for status in [
        "401 Unauthorized",
        "429 Too Many Requests",
        "500 Internal Server Error",
        "302 Found",
    ] {
        let repository = repository()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let target = TcpListener::bind("127.0.0.1:0")?;
        target.set_nonblocking(true)?;
        let status = format!(
            "{status}\r\nLocation: http://{}/private-redirect",
            target.local_addr()?
        );
        let input = json!({"hook_event_name":"Stop","cwd":repository.path()}).to_string();
        let (result, received) = exchange(
            &listener,
            args(Source::Codex, Some(input)),
            Vec::new(),
            credentials(&listener)?,
            &status,
        )?;
        assert!(received.is_some());
        let error = result.err().ok_or("HTTP failure accepted")?.to_string();
        assert_eq!(error, "session-end: webhook rejected delivery");
        assert_eq!(
            target.accept().err().ok_or("redirect followed")?.kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    Ok(())
}

#[test]
fn stalled_and_unavailable_http_fail_privately() -> TestResult {
    let repository = repository()?;
    let input = json!({"hook_event_name":"Stop","cwd":repository.path()}).to_string();
    for status in ["silent", "silent-body"] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let started = std::time::Instant::now();
        let (result, received) = exchange(
            &listener,
            args(Source::Codex, Some(input.clone())),
            Vec::new(),
            credentials(&listener)?,
            status,
        )?;
        assert!(received.is_some());
        assert_eq!(
            result.err().ok_or("stall accepted")?.to_string(),
            "session-end: webhook delivery failed"
        );
        assert!(started.elapsed() >= Duration::from_millis(2500));
        assert!(started.elapsed() < Duration::from_secs(6));
        let credentials = credentials(&listener)?;
        drop(listener);
        let result = deliver(
            args(Source::Codex, Some(input.clone())),
            std::io::empty(),
            || Ok(credentials),
        );
        assert_eq!(
            result.err().ok_or("unavailable HTTP accepted")?.to_string(),
            "session-end: webhook delivery failed"
        );
    }
    Ok(())
}

#[test]
fn environment_proxies_do_not_receive_webhooks() -> TestResult {
    if std::env::var_os("AGENT_SESSION_HOOK_PROXY_TEST").is_some() {
        let repository = repository()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let input = json!({"hook_event_name":"Stop","cwd":repository.path()}).to_string();
        let (result, received) = exchange(
            &listener,
            args(Source::Codex, Some(input)),
            Vec::new(),
            credentials(&listener)?,
            "204 No Content",
        )?;
        result?;
        assert!(received.is_some());
        return Ok(());
    }
    let proxy = TcpListener::bind("127.0.0.1:0")?;
    proxy.set_nonblocking(true)?;
    let proxy_url = format!("http://{}", proxy.local_addr()?);
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "session_end::tests::environment_proxies_do_not_receive_webhooks",
        ])
        .env("AGENT_SESSION_HOOK_PROXY_TEST", "1")
        .env("http_proxy", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("all_proxy", &proxy_url)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        proxy.accept().err().ok_or("proxy received webhook")?.kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}
