# jw

A terminal for running coding agents in parallel. It is its own multiplexer: a daemon owns
every terminal, and a TUI shows them. The README says what jw does for its users; this file
says how the code is laid out and what to keep in mind when changing it.

## Map

| Crate | Owns | Its own rules |
|---|---|---|
| `crates/core` (jw-core) | project config and layouts, the registry of worktrees, sessions and folders, git and GitHub, the actions on worktrees | [crates/core/AGENTS.md](crates/core/AGENTS.md) |
| `crates/proto` (jw-proto) | the messages between daemon and clients, their frames, the client end of the socket | [crates/proto/AGENTS.md](crates/proto/AGENTS.md) |
| `crates/daemon` (jw-daemon) | every PTY and its vt100 screen, each workspace's pane tree, `session.json`, scrollback | [crates/daemon/AGENTS.md](crates/daemon/AGENTS.md) |
| `crates/tui` (jw-tui) | the TUI: sidebar, panes, pickers, diff and Markdown viewers, settings, themes | [crates/tui/AGENTS.md](crates/tui/AGENTS.md) |
| `crates/jw` (jw) | the binary, the CLI, and the daemon's integration tests | [crates/jw/AGENTS.md](crates/jw/AGENTS.md) |

**Dependencies run one way:** `core ← proto ← daemon`, `core, proto ← tui`, everything ←
`jw`. The TUI never depends on the daemon, and core never on proto. The compiler enforces
this; don't add a dependency against it. Move the shared piece down instead.

Each crate root imports the modules it uses from the others (`use jw_core::{stream, …}`), so
code keeps writing `crate::stream::…` across crate lines.

## Commands

```sh
cargo build --workspace
cargo test --workspace                                   # unit + the daemon's integration tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo install --path crates/jw --locked                  # put the new jw on PATH
```

To see the TUI as text from a script, use `scripts/drive.py` (needs
`cargo build --workspace --examples`).

## Rules

- **The spec is `docs/specs/rust-tui.md`:** Contract, Interfaces, REQ-n, RISK-n, and the parts
  table with commit hashes. Read the part you touch. When behaviour changes, update it in the
  same commit and add a Corrections line when something it said was wrong.
- **Commit locally, one commit per part, and never push or open a PR unless asked.**
- **Never bind a key with Alt:** AeroSpace (the window manager) owns Alt.
- **UI changes start with an interactive HTML prototype** that the user approves before the
  spec or the code.
- **Bump `PROTOCOL` (`crates/proto/src/proto.rs`) whenever a message or `PaneInfo` changes.**
  An old daemon must be refused, not silently misread.
- **Never `pkill jw`.** The user's daemon runs their real work. Tests and scripts use their own
  socket dir and kill only that pid.
