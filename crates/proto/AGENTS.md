# jw-proto

What the daemon and its clients (the TUI, the CLI, claude's hooks) say over the unix socket.
It depends only on jw-core, for `layout::Tree`.

## What lives where

- `src/proto.rs`:
  - `PROTOCOL` (`:18`), and the messages `ClientMsg` (`:55`) and `DaemonMsg` (`:271`), with
    `PaneInfo`, `NewPane`, `PaneLeaf` and `Theme`. The `Mobile*` messages, `MobileState` and
    `MobileDevice` control the daemon's mobile service.
  - Framing: `write_frame` and `read_frame` (`:499`, `:526`). A frame is a u32 length, then a
    kind: 0 is JSON; 1 is a JSON head followed by the message's bytes raw (Output, Snapshot, Screen, History,
    Input) — see `Raw`.
  - `read_raw` and `write_raw`: a frame's body without decoding it, for the phone bridge, which
    relays frames it does not read.
  - `socket_path` (`:615`), `pidfile` and `is_view` (a `view:` role has no process).
- `clean` (`proto.rs`): a phone's name or code made safe to print, shared by the bridge, the CLI
  and the TUI.
- `src/client.rs`: `Client` (`:19`), which connects and starts the daemon when nobody
  listens.
- `src/kitty.rs`: `Kitty`, a pane's kitty keyboard flag stacks. The daemon and the TUI
  each keep one per pane, fed by their vt100 parser. It travels in-band, so it is not a
  message (spec, addendum 5).

## Keep in mind

- **Any change to a message, a field, or `PaneInfo` bumps `PROTOCOL`.** That includes a new
  `#[serde(default)]` field. The client checks the version on `Hello` and asks the user to
  restart an old daemon, instead of quietly showing wrong data.
- **Terminal bytes travel raw.** Never send them as JSON arrays: `Raw` must cover any new
  message that carries bytes.

## Tests

`cargo test -p jw-proto` (frame round trips). The real protocol is exercised by
`crates/jw/tests/daemon.rs`.
