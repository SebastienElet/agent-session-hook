# agent-session-hook

Opt-in session-end webhook CLI, ported from arnes commit `5621346`.

## Install

With Rust 1.98.1 and a C toolchain installed, from this checkout:

```sh
cargo install --locked --path .
```

Put `~/.cargo/bin` on the hook runner's `PATH`, or use the executable's absolute
path. Uses libcurl and platform TLS; Linux builds need the OpenSSL development
libraries and pkg-config. macOS supports Keychain and environment credentials;
Linux supports environment credentials.

## Configure

Set `AGENT_SESSION_HOOK_WEBHOOK=routine` when launching the session. It selects a
secret by name. Absent or empty means no input processing, configuration read,
credential lookup or HTTP request. Names contain 1–64 ASCII letters, digits, `_`
or `-`. No other `AGENT_SESSION_HOOK_*` variables are used.

Keep only references in `$HOME/.config/agent-session-hook/webhooks.toml` (`HOME`
must be absolute):

```toml
[webhooks.routine]
source = "keychain"
service = "agent-session-hook-routine"
account = "webhook"
```

On macOS, provision that generic-password item locally with a JSON value:
`{"url":"https://your-endpoint.example/callback","token":"your-token"}`.
Keychain lookup uses `/usr/bin/security` with a one-second deadline.
Alternatively:

```toml
[webhooks.routine]
source = "environment"
url_env = "ROUTINE_WEBHOOK_URL"
token_env = "ROUTINE_WEBHOOK_TOKEN"
```

The named environment variables hold the URL and token. Supply credentials only
to a trusted hook runner: an agent inheriting those variables can read them.
Keychain access and runner isolation are deployment responsibilities; this CLI is
not a secret broker. Keep credential values out of repositories, hook arguments
and hook configuration. Diagnostics omit credentials and session data.

## Hooks

The entry point is `agent-session-hook end --source codex|claude|herdr`.
Native hooks receive their original event JSON on stdin, including an absolute,
existing `cwd`. Codex `notify` may pass its appended JSON argument instead.

For Codex, merge this into `~/.codex/hooks.json`:

```json
{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"agent-session-hook end --source codex","timeout":5}]}]}}
```

Alternatively, set Codex `notify` in `~/.codex/config.toml`:

```toml
notify = ["agent-session-hook", "end", "--source", "codex"]
```

For Claude Code, merge these entries into `~/.claude/settings.json`:

```json
{"hooks":{
  "Stop":[{"hooks":[{"type":"command","command":"agent-session-hook end --source claude","timeout":5}]}],
  "Notification":[{"hooks":[{"type":"command","command":"agent-session-hook end --source claude","timeout":5}]}]
}}
```

Configure the Herdr lifecycle runner to invoke the corresponding command:

```sh
agent-session-hook end --source herdr --event done --worktree /absolute/worktree
agent-session-hook end --source herdr --event exited --worktree /absolute/worktree
```

The runner must retain the session's webhook name after agent exit. Native hook
installation, Herdr wiring and launcher deployment are external to this crate.

## Delivery

One POST with `Authorization: Bearer <token>` and `Content-Type: application/json`:

```json
{"worktree":"/canonical/session/cwd","event":"Stop","time":1792920000}
```

Exactly three fields: canonical session cwd (no Git-root lookup), event, and UTC
Unix seconds. Events are `Stop`, `Notification`, `done` or `exited`; Codex
`agent-turn-complete` maps to `Stop`. Session text is never sent.

HTTPS is required except for literal loopback HTTP (`127.0.0.1` or `[::1]`).
Userinfo and fragments are rejected. Redirects, environment proxies and automatic
retries are disabled. HTTP has a three-second timeout; only 2xx succeeds. Failures
exit 1 with a generic diagnostic. Events are independent, without deduplication
or a delivery guarantee.

Four owner decisions remain open, preserving the port's current choices:
`src/webhook_format.rs` (Bearer + JSON), `src/worktree.rs` (session cwd),
`src/http_client.rs` (curl/http), `src/secrets.rs` (Keychain + environment).

Local TCP tests exercise the CLI and delivery protocol. Real Codex/Claude bot
wakeups, Herdr lifecycle delivery and secret isolation require deployment
validation. CI runs on Linux; its sub-minute runtime target needs a remote run.

BSD-2-Clause; see [LICENSE](LICENSE).
