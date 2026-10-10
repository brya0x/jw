# jw-proto

What the daemon and its clients (the TUI, the CLI, claude's hooks) say over the unix socket.
It depends only on jw-core, for `layout::Tree`.

## What lives where

- `src/proto.rs`:
  - `PROTOCOL` (`:18`), and the messages `ClientMsg` (`:55`) and `DaemonMsg` (`:214`), with
    `PaneInfo`, `NewPane` and `PaneLeaf`.
  - Framing: `write_frame` and `read_frame` (`:335`, `:362`). A frame is a u32 length, then a
    kind: 0 is JSON; 1 is a JSON head followed by the message's bytes raw (Output, Snapshot,
    Input) — see `Raw`.
  - `socket_path` (`:408`), `pidfile` and `is_view` (a `view:` role has no process).
- `src/client.rs`: `Client` (`:19`), which connects and starts the daemon when nobody
  listens.

## Keep in mind

- **Any change to a message, a field, or `PaneInfo` bumps `PROTOCOL`.** That includes a new
  `#[serde(default)]` field. The client checks the version on `Hello` and asks the user to
  restart an old daemon, instead of quietly showing wrong data.
- **Terminal bytes travel raw.** Never send them as JSON arrays: `Raw` must cover any new
  message that carries bytes.

## Tests

`cargo test -p jw-proto` (frame round trips). The real protocol is exercised by
`crates/jw/tests/daemon.rs`.
