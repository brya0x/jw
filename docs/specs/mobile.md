---
status:      agreed
scope:       [crates/web/, crates/proto/src/proto.rs, crates/jw/src/main.rs, crates/jw/src/help.rs, crates/jw/Cargo.toml, crates/core/src/session.rs (RESERVED), android/, docs/specs/mobile.md]
depends_on:  [cloudflared on PATH, Android SDK + Kotlin/Compose, jw daemon unchanged at PROTOCOL 8]
supersedes:  []
---
# jw mobile: work in your jw sessions from an Android phone, through a Cloudflare tunnel

## Contract

- `jw mobile` on the PC starts a **bridge** (WebSocket on loopback) and a `cloudflared` quick tunnel to it, then prints a QR holding the tunnel URL and a one-time pairing token.
- The **Android app** (native Kotlin, Compose) scans the QR, pairs once, then shows what the TUI shows: sessions, workspaces, panes, diff, settings, in jw's own theme.
- A *workspace* is any open folder: a project root, a worktree or a plain folder (`rust-tui.md:12`). The app enters all three.
- The bridge relays daemon frames unchanged. It adds auth, a protocol check and a small JSON control channel (palettes, workspaces, diff). The daemon and `PROTOCOL` do not change.
- Whoever holds a valid device token can type into every pane of the PC, so the bridge is the only gate (the daemon has no peer check, `crates/daemon/src/lib.rs:142,157`).
- UI reference: the approved prototype, saved as `docs/specs/mobile-prototype.html`.

## Interfaces

| Piece | Where | Shape |
|---|---|---|
| CLI | arm at `crates/jw/src/main.rs:45-60`, row at `help.rs:14`, name in `core/src/session.rs:18` | `jw mobile [--read-only] [--port N]`, `jw mobile devices`, `jw mobile revoke <device>` |
| Bridge | `crates/web/src/bridge.rs` (jw-web; sync, threads, as `daemon/src/lib.rs:70-75`) | listens on `127.0.0.1:<port>`; WS **binary** message = one jw-proto frame, kind byte plus body, no length prefix (`proto/src/proto.rs:364-390`); WS **text** message = control JSON |
| Raw frames | `proto/src/proto.rs` `read_raw`, `write_raw` | a frame's body (kind byte and what follows) without decoding it; both check the kind and `MAX_FRAME` |
| Control, phone to bridge | JSON, tag `t` | `pair{token,name}`, `auth{device}`, `workspaces`, `sessions`, `diff{ws}`, `open{path}`, `worktree{project,branch}` |
| Control, bridge to phone | JSON, tag `t` | `ready{bridge:1,protocol,palettes,settings}`, `paired{device}`, `workspaces`, `sessions`, `diff`, `error{code,msg}` |
| Palettes | `tui/src/theme.rs:19-37` | the 10 built-in themes plus user themes, as colour roles, plus `dark`; `settings` names the dark and light slots |
| Store | `<config dir>/mobile.json`, mode 0600 | device name, token hash, last seen |
| Tunnel | child `cloudflared tunnel --url http://127.0.0.1:<port>` | URL read from its output |
| App | `android/` | Gradle; modules `proto` (frame codec), `term` (VT emulator), `app` (Compose) |

## Acceptance

| REQ | Requirement |
|---|---|
| 1 | WHEN `jw mobile` starts, the bridge SHALL listen on 127.0.0.1 only, start `cloudflared`, and print the tunnel URL and a QR of it with the pairing token. |
| 2 | THE pairing token SHALL be 128 random bits, valid for one use and for 120 s. |
| 3 | WHEN a valid pairing token arrives, the bridge SHALL ask on the PC's terminal to confirm the device name, and only then issue a 256-bit device token, storing its hash only. |
| 4 | IF a connection sends anything but `pair` or `auth` before authenticating, THEN the bridge SHALL close it and forward nothing to the daemon. |
| 5 | IF five pair or auth attempts fail within 60 s from one address, THEN the bridge SHALL refuse that address for 5 min. |
| 6 | WHEN `jw mobile revoke <device>` runs, the bridge SHALL close that device's connections and reject its token. |
| 7 | WHERE `--read-only` is set, the bridge SHALL drop every message except `Hello`, `Attach`, `Detach` and `List`, and every control request except the reads. |
| 8 | WHEN the bridge connects to the daemon, it SHALL send `Hello`; IF the daemon's `PROTOCOL` differs from the bridge's, THEN it SHALL refuse every device with error `protocol`. |
| 9 | WHEN the bridge stops, it SHALL kill `cloudflared` and close every device connection. |
| 10 | WHILE a device is authenticated, the bridge SHALL relay its binary messages to the daemon and the daemon's frames back unchanged, over one daemon connection per device. |
| 11 | IF the daemon connection closes, THEN the bridge SHALL send error `daemon` and close the device's WS. |
| 12 | WHEN the phone asks, the bridge SHALL answer `workspaces` and `sessions` from the registry and folders (`jw ls --json` data): roots, worktrees and folders. |
| 13 | WHEN the phone sends `diff{ws}`, the bridge SHALL answer with the workspace's changed files and hunks against its base. |
| 14 | WHEN the app has no stored device, it SHALL open the QR scanner and pair; it SHALL keep the device token in Keystore-backed storage. |
| 15 | WHEN the connection drops, the app SHALL reconnect with backoff, re-attach, and treat the new `Snapshot` as the truth. |
| 16 | IF `ready.protocol` differs from the app's, THEN the app SHALL show an update screen and not attach. |
| 17 | THE app SHALL draw with the palette from `ready`, following the system dark or light setting unless the user pins a theme in Settings. |
| 18 | THE workspaces screen SHALL list roots with their worktrees nested, folders in their own section, and the marks `✻ ? ⚡ ⚑` derived from `PaneInfo` as the TUI does (`tui/src/tui/mod.rs:960-979`). |
| 19 | WHEN the user taps any workspace row, root, worktree or folder, the app SHALL attach and show its panes. |
| 20 | WHILE a pane's agent is `Waiting`, the row and the pane SHALL offer y and n; WHEN tapped, the app SHALL send `Input` with the configured bytes. |
| 21 | THE terminal surface SHALL render `Snapshot` and `Output` through an on-device VT emulator. |
| 22 | THE app SHALL offer a key row (esc, tab, enter, arrows, ^C, ^D, ^R) and text selection to the Android clipboard. |
| 23 | WHEN the user taps `^␣`, the app SHALL show the TUI's actions (`rust-tui.md:22-32`) and run each with the same client message the TUI sends. |
| 24 | THE app SHALL NOT send `Resize` unless "fit to phone" is on; WHEN it is turned off or the pane is left, the app SHALL send the size the pane had before. |
| 25 | WHEN the user opens Changes, the app SHALL show files and hunks with add and delete colours, and keep "viewed" per file on the device. |
| 26 | THE app SHALL size every touch target at 48 dp or more and show the mode chip (NORMAL, TERM, DIFF, SET, ?) and status bar of the prototype. |
| 27 | THE app and bridge SHALL NOT log terminal bytes or tokens. |
| 28 | THE Avisos tab SHALL list every pane whose agent is `Waiting`, with y and n. |

---

## Rationale

- **RAT-1 Bridge outside the daemon.** The daemon is single-user by socket mode (`lib.rs:142,157`). Network exposure belongs in a separate process that can be stopped, revoked and read-only, so the daemon never listens on a network.
- **RAT-2 Frames unchanged.** The phone is a full proto client: no new daemon message, so no `PROTOCOL` bump (`proto/AGENTS.md:23-24`). The control channel carries only what the daemon never held (palettes, registry, diff).
- **RAT-3 Cloudflare quick tunnel.** User's choice, 2026-10-11. Cloudflare ends TLS, so the bridge needs no TLS crate and listens in plain text on loopback.
- **RAT-4 Sync WebSocket, threads.** The workspace has no async runtime (`crates/jw/Cargo.toml:9-20`) and the daemon is thread-per-client. A sync WS crate keeps that model.
- **RAT-5 Native Kotlin.** User's choice, 2026-10-11. iOS comes later, so `proto` and `term` hold no UI and can be ported (Kotlin Multiplatform is an option).
- **RAT-6 Own spec file.** `rust-tui.md` is 555 lines and `AGENTS.md:42` names it as "the spec". `AGENTS.md` gets a pointer to this one.
- **RAT-7 Emulator on the phone.** The TUI also receives raw bytes (`Snapshot` ring, 2 MiB, `lib.rs:383-407`). Same input, same model.
- **RAT-8 PC confirms pairing.** The QR sits on a screen that may be shared. A token alone must not be enough.
- **RAT-9 Look.** Terminal character (mono type, jw marks, mode chip, status bar) with touch rows and sheets. The user rejected a pure terminal copy, then a pure card UI, then asked for this middle (prototype v2).

## Risks

- **RISK-1 Token holder owns the PC.** REQ-2 to 7 limit it. Unverified: whether a quick-tunnel URL can leak through `cloudflared` logs.
- **RISK-2 Last `Resize` wins** (`lib.rs:588-592`). A phone resize would shrink the PC's pane. REQ-24 is the guard.
- **RISK-3 Quick tunnels.** No SLA, a new URL each run, and WebSocket over them is **unverified**. M2 starts by proving it.
- **RISK-4 Approve bytes.** y and n may not match the agent's prompt UI; Waiting detection comes from `AgentState` (`proto.rs:219`). Bytes are configurable.
- **RISK-5 Emulator.** Candidate: Termux's `terminal-emulator` (licence and kitty replay support **unverified**).
- **RISK-6 Android kills background sockets.** No alerts when the app is backgrounded (OPEN-3).
- **RISK-7 Cellular cost.** A resync sends a snapshot of up to 2 MiB (`outbox.rs:55-64`). The bridge may need to throttle.
- **RISK-8 New dependencies** in a dep-light workspace: a sync WS crate, `qrcode`, `sha2`, a random source.
- **RISK-10 Behind `cloudflared` every connection comes from 127.0.0.1.** REQ-5's "per address" would lock out the real phone, or let anyone lock it out. M2 keys the limit on `CF-Connecting-IP`, trusted only because the listener is loopback and `cloudflared` is its one client.
- **RISK-11 A web page can open `ws://127.0.0.1:<port>`.** Auth (REQ-4) is the guard. M2 also refuses a handshake that carries an `Origin` header (a phone app sends none) and adds that as a REQ.
- **RISK-12 M1 reaches the daemon before the phone has said anything.** `serve` connects and sends `Hello` first, which REQ-4 forbids once there is auth. M2 authenticates first, then connects, and gives the pre-auth phase its own deadline.
- **RISK-13 A dead phone behind `cloudflared` is only noticed when writes stall.** It keeps its daemon subscriptions meanwhile. M2 adds a WebSocket ping and drops on a missed pong.
- **RISK-9 Diff source.** The TUI draws diffs client-side (`tui/src/tui/diffview.rs`). Whether git diff generation can move to `core` for the bridge is **unchecked**.

## Open questions

- **OPEN-1** Resize default. Chosen: never resize, offer "fit to phone". Alternative: phone resizes always.
- **OPEN-2** Named tunnel (Cloudflare account, stable URL, no re-scan) instead of a quick one. Chosen: quick.
- **OPEN-3** Push alerts while backgrounded (needs a relay or ntfy). Chosen: none in v1.
- **OPEN-4** Package `android/` in this repo, minSdk 26. Chosen as written.
- **OPEN-5** Default approve and reject bytes.

## Parts

| Part | What | REQ | Main files |
|---|---|---|---|
| M1 ✓ | Bridge relay, protocol check, stop (REQ-9: the connections; `cloudflared` is M2) | 8–11 | `crates/web/src/bridge.rs`, `proto.rs`, `jw/tests/web.rs` |
| M2 | Tunnel (prove WS first), QR, pairing, auth, rate limit, revoke, read-only, `jw mobile` | 1–7 | `crates/web/`, `jw/src/main.rs`, `help.rs` |
| M3 | Android shell: scan, pair, reconnect, version screen, theme | 14–17, 27 | `android/app`, `android/proto` |
| M4 | Workspaces, sessions, enter, approve, Avisos | 12, 18–20, 28 | `android/app`, `crates/web/` |
| M5 | Terminal surface, keys, leader sheet, resize | 21–24 | `android/term`, `android/app` |
| M6 | Changes and diff | 13, 25 | `crates/web/`, `android/app` |
| M7 | Settings, touch and mode chrome, polish | 26 | `android/app` |

## Corrections

- The bridge was to live in `crates/jw/src/mobile/`. It lives in its own crate, `crates/web` (jw-web, first named jw-mobile and renamed: it relays frames and is not specific to phones): `jw` is bin-only, so its integration tests could not reach a bridge inside it, and a library has no dead code before `jw mobile` exists. `jw mobile` is wired in M2 together with auth, so no listener without auth is ever runnable.
- The spec took for granted that `jw-proto` could relay a frame as it is. It had no byte-level API: `read_raw` and `write_raw` were added and `MAX_FRAME` made public. No message changed, so `PROTOCOL` stays 8.
- The plan assumed the `Theme` message could carry a palette. It carries only `dark`, `fg`, `bg` (`proto.rs:172-177`), so palettes travel on the control channel.
- The plan assumed the daemon refuses old clients. It does not compare versions (`lib.rs:510-512`); the client does (`jw/src/server.rs:40-46`), hence REQ-8.

## Tests

- **Bridge, unit:** token one-use and expiry, rate limit, hash-only storage, read-only filter.
- **Bridge, integration** (`crates/jw/tests/`, own socket dir, kill only its own pid): a WS client pairs, authenticates, relays `Hello`, `Attach`, `Input` and reads a `Snapshot`; a `PROTOCOL` mismatch is refused; a bad token gets nothing.
- **Android:** frame-codec round trips against vectors generated by `jw-proto`; emulator golden screens; Compose screenshots against the prototype.
- **By hand:** `scripts/jw-test` plus a real phone over the tunnel. Never `pkill jw`.

