# jw-mobile

The bridge between a phone and the daemon (spec: [docs/specs/mobile.md](../../docs/specs/mobile.md)).
It depends on jw-proto only; `jw` will start it (`jw mobile`, part M2).

## What lives where

- `src/bridge.rs`: `Bridge`, a WebSocket server on loopback. Each phone gets one daemon
  connection. A binary message is a jw-proto frame without its length, relayed with
  `read_raw` and `write_raw`; a text message is a control message.

## Keep in mind

- **Loopback only.** `Bridge::start` refuses any other address: whatever reaches the bridge
  reaches every pane, and the daemon has no peer check of its own.
- **Never start a daemon.** No daemon on the socket is the error `daemon`, not a start.
- **Check the protocol here.** The daemon does not compare versions; the bridge does, per phone.
- **No listener without auth may be runnable.** Auth is part M2; until then nothing in the
  `jw` binary starts a `Bridge`.
- **A slow phone must not grow memory, and a stalled one is dropped.** The queue from the daemon is bounded on purpose, so
  the daemon's own queue limit takes over.
- **Never log terminal bytes or tokens** (REQ-27).

## Tests

`cargo test -p jw-mobile` runs the bridge against a fake daemon (protocol mismatch, hang up,
no daemon, stop). `crates/jw/tests/mobile.rs` runs it against the real daemon.
