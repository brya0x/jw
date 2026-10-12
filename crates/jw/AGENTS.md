# jw (the binary)

`jw` with no arguments, or with a session name, opens the TUI (jw-tui). `jw daemon` runs
jw-daemon, and is started by the client, never by hand. Everything else is the CLI in
`src/cli.rs`.

## What lives where

- `src/main.rs`: dispatches the subcommands. `tui()` picks the session (the one named, else
  the last used one).
- `src/cli.rs`: `new_session` (`:73`), `sessions` (`:107`), and the commands agents use:
  `ls` (`:407`), `read` (`:454`), `worktree` (`:151`), `prompt` (`:94`), plus `hook`
  (`:47`), which claude's hooks run. They act on `$JW_SESSION`.
- `src/help.rs`: `COMMANDS`, what `jw help` prints. A new command gets a row there.
- `src/theme.rs`: `jw theme ls|install|export|use`, over `jw_tui::theme` (S16). A URL install
  runs `curl`.
- `src/skill.rs` and `skill/SKILL.md`: the Claude Code skill, built into the binary;
  `jw skill install` copies it to `~/.claude/skills/jw/`.
- `src/mobile.rs`: `jw mobile`, a client of the daemon's mobile service: it starts mobile if it
  is off, prints each step, the QR and the link, and answers pair questions at the keyboard.
  `stop` never starts a daemon; `devices` and `revoke` use the store and need none. Spec:
  `docs/specs/mobile.md`.
- `examples/screen.rs`: prints the screen a byte stream leaves, for `scripts/drive.py`.
- `tests/daemon.rs`: the daemon's integration tests. They start this binary on a socket of
  their own. `tests/web.rs` puts jw-web in front of that daemon. `tests/mobile.rs` and `tests/mobile_cli.rs` drive the
  daemon's mobile service with a stub for `cloudflared` (`JW_CLOUDFLARED`), a `PATH` without
  one, and config and state dirs of its own: a test here must never be able to start a real
  tunnel, read the user's paired devices or open a listener on the network of whoever runs it:
  `JW_LAN_ADDR` is always set for the daemon (`off`, loopback, or `file:<path>`), never unset, because unset means "find the real address and listen on it".

## Keep in mind

- **`jw hook` must stay silent and exit 0.** It runs inside claude on every prompt and tool
  call.
- **The CLI's output is read by agents.** Keep `jw ls --json` stable, and document changes
  to it in `skill/SKILL.md`, then `jw skill install`.
- **A session name can't be a subcommand** (`session::RESERVED`). Add new subcommands there
  and in `help::COMMANDS`; a test checks that every command in help is reserved.

## Tests

`cargo test -p jw`. The daemon tests use `CARGO_BIN_EXE_jw` and kill only their own daemon.
