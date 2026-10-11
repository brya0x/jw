# jw-daemon

The daemon: it owns every terminal (one PTY and one vt100 parser per pane) and each
workspace's tree of panes. It survives every client. It knows nothing of sessions, because
workspace ids are already unique across them.

## What lives where

- `src/lib.rs`:
  - `run` (`:62`) is the accept loop; it takes the `Look` the `jw` binary hands in (themes
    for a phone's `ready`). `restore` (`:996`) brings back `session.json` at start.
    `save_on_signal` (`:107`) saves everything on SIGTERM, SIGINT or SIGHUP, then stops mobile
    (2 s at most).
  - `serve` (`:515`) runs one client: a reader, and a writer fed by its `Tx`.
  - `handle` (`:559`) takes every `ClientMsg`.
  - `attach`/`watch` (`:736`/`:758`) send a tree, then each pane's Snapshot, or its `Screen`
    and then its `History` to a client that sent `SplitSnapshots` (`split_snapshot`).
  - `spawn` (`:1271`) starts a pane's process; its reader thread answers what the program asks
    its terminal (`Titles`, `:255`: OSC 10/11, `?996n`, `?2031`, `6n`, `c`, `?u`).
  - `set_theme` (`:1194`) keeps the client's theme and sends `?997` to panes with mode 2031.
  - `edit` (`:962`) changes a tree; `save` (`:1056`) writes `session.json` and the scrollback.
  - `rekey` (`:779`) moves a workspace to a new id.
- `src/outbox.rs`: `Tx`, a client's bounded queue. Past 8 MiB it drops that client's output
  for a pane, then resyncs it with a Snapshot (REQ-76).
- `src/mobile.rs`: `Mobile`, the service behind `MobileStart` and the rest (spec: mobile.md,
  REQ-33 to 40): phases off, starting, ready, failed; the bridge and `cloudflared` from jw-web;
  the `Lister` for a phone's `workspaces` and `sessions` (from `core::sidebar`); the watchers (clients that sent `MobileWatch`) and the pair question put to them. A start
  has an *epoch*: a stop or a failure bumps it, and a start or ticker thread of an older one
  does nothing more.
- `src/ring.rs`: the last 2 MiB of each pane's raw output. A Snapshot replays it, and the
  scrollback files hold it across restarts.

## Keep in mind

- **Lock order is saving → workspaces → panes → a pane's state.** `saving` is held for a whole
  save, so never call `save` with another lock held. A pane's state lock covers its
  parser and its subscribers, which is what makes "snapshot, then output" race free.
  Broadcast output while holding it.
- **Save before telling.** A tree change a client has seen must already be in `session.json`
  (see `edit` and `add`).
- **Reply to a pane with its state lock dropped.** Take `io` only after releasing `state`
  (the reader's replies, `set_theme`), never both at once.
- **Never block a pane's reader thread on a client.** Clients go through their `Tx`.
- **Mobile never takes a pane, workspace or session lock** (REQ-40), and never holds its own
  `Core` lock while it waits for the network, the tunnel or a person. `Mobile::stop` kills
  `cloudflared` and answers every question no first; closing the phones' sockets and joining
  happen on another thread that is waited for up to the limit. The device list is cached: a
  state never reads `mobile.json` under a lock.
- **Panes get `JW_PANE_ID`.** claude's hooks (`jw hook`) find their pane by it.

## Tests

`cargo test -p jw-daemon` for the queue, the ring and the mobile service's pair question. The
real daemon is tested in `crates/jw/tests/daemon.rs`, which starts the `jw` binary on its own
socket, and mobile in `crates/jw/tests/mobile.rs`, with a stub for `cloudflared` and a `PATH`
without one. Run those after any change here.
