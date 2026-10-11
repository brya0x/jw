# jw-web

The web server in front of the daemon (spec: [docs/specs/mobile.md](../../docs/specs/mobile.md)).
It depends on jw-proto only; `jw mobile` (`crates/jw/src/mobile.rs`) starts it behind a
Cloudflare tunnel. Nothing in it is specific to phones, so the name is not either.

## What lives where

- `src/bridge.rs`: `Bridge`, a WebSocket server on loopback. A phone is let in by `pair` or
  `auth` and nothing else, then gets one daemon connection. A binary message is a jw-proto
  frame without its length, relayed with `read_raw` and `write_raw`; a text message is a
  control message. `Timings` holds every wait, so tests make them short.
- `src/auth.rs`: `Pairing` (the one-use token), `Store` (`mobile.json`, hashes only), `Limiter`
  (failed attempts per address).

## Keep in mind

- **Loopback only.** `Bridge::start` refuses any other address: whatever reaches the bridge
  would reach every pane, and the daemon has no peer check of its own.
- **Nothing reaches the daemon before auth.** The order in `serve` is: handshake (no `Origin`),
  `pair` or `auth`, then the daemon, then `ready`. Keep it.
- **Never start a daemon.** No daemon on the socket is the error `daemon`, not a start.
- **Check the protocol here.** The daemon does not compare versions; the bridge does, per phone.
- **Secrets are stored hashed, the file is 0600, and every change takes the `flock`.** `jw mobile
  revoke` writes it from another process while the bridge runs.
- **Anything printed on the PC from a phone is cleaned first** (`clean`): no escape codes in
  someone's terminal.
- **Behind `cloudflared` every peer is 127.0.0.1.** The limiter keys on `Cf-Connecting-Ip`.
- **A slow phone must not grow memory, and a stalled one is dropped.** The queue from the
  daemon is bounded on purpose, so the daemon's own queue limit takes over.
- **Never log terminal bytes or tokens** (REQ-27).

## Tests

`cargo test -p jw-web` runs the bridge against a fake daemon (auth, pairing, limits, read-only,
revoke, ping, protocol mismatch, stop). `crates/jw/tests/web.rs` runs it against the real
daemon. Nothing here needs `cloudflared` or the network; the tunnel is checked by hand.
