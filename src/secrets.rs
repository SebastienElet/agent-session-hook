use crate::{Error, failure};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(any(target_os = "macos", test))]
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    webhooks: BTreeMap<String, SecretSource>,
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum SecretSource {
    Keychain { service: String, account: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Credentials {
    pub(crate) url: String,
    pub(crate) token: String,
}

pub(crate) fn configured_source(name: &str) -> Result<SecretSource, Error> {
    if name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(failure("invalid webhook name"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| failure("absolute HOME is required"))?;
    let text = std::fs::read_to_string(home.join(".config/agent-session-hook/webhooks.toml"))
        .map_err(|_| failure("webhook configuration is unavailable"))?;
    let mut config: Configuration =
        toml::from_str(&text).map_err(|_| failure("invalid webhook configuration"))?;
    config
        .webhooks
        .remove(name)
        .ok_or_else(|| failure("webhook name is not configured"))
}

impl SecretSource {
    pub(crate) fn resolve(self) -> Result<Credentials, Error> {
        match self {
            Self::Keychain { service, account } => keychain(&service, &account),
        }
    }
}

#[cfg(target_os = "macos")]
fn keychain(service: &str, account: &str) -> Result<Credentials, Error> {
    let mut command = std::process::Command::new("/usr/bin/security");
    command.args(["find-generic-password", "-s", service, "-a", account, "-w"]);
    keychain_credentials(command)
}

#[cfg(any(target_os = "macos", test))]
fn keychain_credentials(mut command: std::process::Command) -> Result<Credentials, Error> {
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| failure("keychain lookup failed"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failure("keychain lookup failed or timed out"));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|_| failure("keychain lookup failed"))?;
    if !output.status.success() {
        return Err(failure("keychain lookup failed"));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| failure("invalid keychain credentials"))
}

#[cfg(not(target_os = "macos"))]
fn keychain(_service: &str, _account: &str) -> Result<Credentials, Error> {
    Err(failure("keychain requires macOS"))
}

#[cfg(test)]
mod tests {
    use super::keychain_credentials;
    use std::process::Command;

    #[test]
    fn credential_process_success_and_failures_are_private()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut valid = Command::new("/bin/sh");
        valid.args(["-c", "printf '%s' '{\"url\":\"https://example.invalid/private\",\"token\":\"private-token\"}'"]);
        let credentials = keychain_credentials(valid)?;
        assert_eq!(credentials.url, "https://example.invalid/private");
        assert_eq!(credentials.token, "private-token");
        for script in [
            "printf private; printf private >&2; exit 1",
            "printf private",
            "exec /bin/sleep 5",
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            let started = std::time::Instant::now();
            let error = keychain_credentials(command)
                .err()
                .ok_or("credentials unexpectedly accepted")?
                .to_string();
            assert!(error.starts_with("session-end:"));
            assert!(!error.contains("private"));
            assert!(started.elapsed() < std::time::Duration::from_secs(3));
        }
        let error = keychain_credentials(Command::new("/does-not-exist-private"))
            .err()
            .ok_or("missing executable accepted")?
            .to_string();
        assert_eq!(error, "session-end: keychain lookup failed");
        Ok(())
    }
}
