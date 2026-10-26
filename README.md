# agent-session-hook

Opt-in session-end webhooks for Codex, Claude Code and Herdr. Requires macOS for
Keychain credentials. Ported from arnes commit `5621346`.

## Install

From this checkout, with Rust 1.98.1 and a C toolchain for ring:

```sh
cargo install --locked --path .
```

Put `~/.cargo/bin` on the hook runner's `PATH`, or use an absolute executable path.
HTTPS uses ureq, rustls, ring and bundled webpki roots; no OpenSSL or libcurl.

## Configure

Launch the session with `AGENT_SESSION_HOOK_WEBHOOK=routine`. This selects a secret
by name, never by value. Absent or empty means no input processing, configuration
read, Keychain lookup or HTTP request. Names contain 1–64 ASCII letters, digits,
`_` or `-`.

Store only references in `$HOME/.config/agent-session-hook/webhooks.toml` (`HOME`
must be absolute):

```toml
[webhooks.routine]
source = "keychain"
service = "agent-session-hook-routine"
account = "webhook"
```

Provision that macOS generic-password item locally with a JSON value containing
`url` and `token` strings, for example
`{"url":"https://your-endpoint.example/callback","token":"your-token"}`.
The CLI reads it through `/usr/bin/security`, with a one-second deadline.
Keychain access and runner isolation are deployment responsibilities. Keep
credential values out of repositories, hook arguments and hook configuration.
Diagnostics omit credentials and session data. Declared hooks fail explicitly
on platforms without macOS Keychain support.

## Usage

Native hooks receive their original JSON on stdin, including an absolute,
existing session `cwd`:

```sh
agent-session-hook end --source codex
agent-session-hook end --source claude
```

Use the Codex `Stop` hook in `~/.codex/hooks.json`, or configure Codex notify in
`~/.codex/config.toml`; notify appends its JSON argument:

```toml
notify = ["agent-session-hook", "end", "--source", "codex"]
```

For Claude Code, register the command for `Stop` and `Notification` in
`~/.claude/settings.json`. For Herdr lifecycle events, invoke:

```sh
agent-session-hook end --source herdr --event done --worktree /absolute/session/cwd
agent-session-hook end --source herdr --event exited --worktree /absolute/session/cwd
```

The runner must retain the session's webhook name after agent exit. Installation
of native hooks, Herdr wiring and launcher deployment are external to this crate.

## Payload and delivery

One POST with `Authorization: Bearer <token>` and `Content-Type: application/json`:

```json
{"worktree":"/canonical/git/root","event":"Stop","time":1792920000}
```

Exactly three fields: Git worktree root, event, and UTC Unix seconds. Resolution
starts at the canonical cwd and walks up to the nearest directory containing a
`.git` directory or file, including linked worktrees and submodules. It does not
invoke Git or honor `GIT_DIR` overrides. Outside a Git worktree, the command exits
1 with `session-end: cwd is not inside a Git worktree`, without sending a request.
Events are `Stop`, `Notification`, `done` or `exited`; Codex `agent-turn-complete`
maps to `Stop`. Session text is never sent.

HTTPS is required except for literal loopback HTTP (`127.0.0.1` or `[::1]`).
Userinfo and fragments are rejected. Redirects, environment proxies and automatic
retries are disabled. The three-second HTTP timeout includes response-body reads;
only 2xx succeeds. Failures exit 1 with a generic diagnostic. Events are independent,
without deduplication or a delivery guarantee.

Tests cover Git discovery, native input, local HTTP delivery and a substituted
Keychain process. Real agent wakeups and deployment isolation remain to be checked.

BSD-2-Clause; see [LICENSE](LICENSE).
