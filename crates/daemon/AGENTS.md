# jw-daemon

The daemon: it owns every terminal (one PTY and one vt100 parser per pane) and each
workspace's tree of panes. It survives every client. It knows nothing of sessions, because
workspace ids are already unique across them.

## What lives where

- `src/lib.rs`:
  - `run` (`:53`) is the accept loop. `restore` (`:904`) brings back `session.json` at
    start. `save_on_signal` (`:95`) saves everything on SIGTERM, SIGINT or SIGHUP.
  - `serve` (`:439`) runs one client: a reader, and a writer fed by its `Tx`.
  - `handle` (`:482`) takes every `ClientMsg`.
  - `attach`/`watch` (`:639`/`:666`) send a tree, then each pane's Snapshot.
  - `spawn` (`:1139`) starts a pane's process; its reader thread answers what the program asks
    its terminal (`Titles`, `:234`: OSC 10/11, `?996n`, `?2031`, `6n`, `c`).
  - `set_theme` (`:1086`) keeps the client's theme and sends `?997` to panes with mode 2031.
  - `edit` (`:875`) changes a tree; `save` (`:964`) writes `session.json` and the scrollback.
  - `rekey` (`:681`) moves a workspace to a new id.
- `src/outbox.rs`: `Tx`, a client's bounded queue. Past 8 MiB it drops that client's output
  for a pane, then resyncs it with a Snapshot (REQ-76).
- `src/ring.rs`: the last 2 MiB of each pane's raw output. A Snapshot replays it, and the
  scrollback files hold it across restarts.

## Keep in mind

- **Lock order is workspaces → panes → a pane's state.** A pane's state lock covers its
  parser and its subscribers, which is what makes "snapshot, then output" race free.
  Broadcast output while holding it.
- **Save before telling.** A tree change a client has seen must already be in `session.json`
  (see `edit`).
- **Reply to a pane with its state lock dropped.** Take `io` only after releasing `state`
  (the reader's replies, `set_theme`), never both at once.
- **Never block a pane's reader thread on a client.** Clients go through their `Tx`.
- **Panes get `JW_PANE_ID`.** claude's hooks (`jw hook`) find their pane by it.

## Tests

`cargo test -p jw-daemon` for the queue and the ring. The real daemon is tested in
`crates/jw/tests/daemon.rs`, which starts the `jw` binary on its own socket. Run those after
any change here.
