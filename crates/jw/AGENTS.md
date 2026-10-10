# jw (the binary)

`jw` with no arguments, or with a session name, opens the TUI (jw-tui). `jw daemon` runs
jw-daemon, and is started by the client, never by hand. Everything else is the CLI in
`src/cli.rs`.

## What lives where

- `src/main.rs`: dispatches the subcommands. `tui()` picks the session (the one named, else
  the last used one).
- `src/cli.rs`: `new_session` (`:73`), `sessions` (`:107`), and the commands agents use:
  `ls` (`:407`), `read` (`:454`), `worktree` (`:151`), `prompt` (`:28`), plus `hook`
  (`:47`), which claude's hooks run. They act on `$JW_SESSION`.
- `src/help.rs`: `COMMANDS`, what `jw help` prints. A new command gets a row there.
- `src/theme.rs`: `jw theme ls|install|export|use`, over `jw_tui::theme` (S16). A URL install
  runs `curl`.
- `src/skill.rs` and `skill/SKILL.md`: the Claude Code skill, built into the binary;
  `jw skill install` copies it to `~/.claude/skills/jw/`.
- `examples/screen.rs`: prints the screen a byte stream leaves, for `scripts/drive.py`.
- `tests/daemon.rs`: the daemon's integration tests. They start this binary on a socket of
  their own.

## Keep in mind

- **`jw hook` must stay silent and exit 0.** It runs inside claude on every prompt and tool
  call.
- **The CLI's output is read by agents.** Keep `jw ls --json` stable, and document changes
  to it in `skill/SKILL.md`, then `jw skill install`.
- **A session name can't be a subcommand** (`session::RESERVED`). Add new subcommands there
  and in `help::COMMANDS`; a test checks that every command in help is reserved.

## Tests

`cargo test -p jw`. The daemon tests use `CARGO_BIN_EXE_jw` and kill only their own daemon.
