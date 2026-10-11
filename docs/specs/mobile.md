---
status:      agreed
scope:       [crates/daemon/src/mobile.rs, crates/daemon/src/lib.rs, crates/web/src/, crates/proto/src/proto.rs, crates/core/src/qr.rs, crates/tui/src/tui/ (modal.rs, mod.rs, draw.rs), crates/tui/src/settings.rs, crates/jw/src/mobile.rs, android/, docs/specs/mobile.md]
depends_on:  [cloudflared on PATH, Android SDK + Kotlin/Compose]
supersedes:  []
---
# jw mobile: work in your jw sessions from an Android phone, through a Cloudflare tunnel

## Contract

- The **daemon hosts mobile**: a bridge (WebSocket on loopback) and a `cloudflared` quick tunnel to it. It starts only when asked and stays up after the TUI closes.
- Anything that talks to the daemon can drive and watch it: the **Mobile modal** in the TUI (`^␣ m`), and `jw mobile` in a terminal. Pairing is answered by whoever is at the PC, never by the phone.
- The **Android app** (native Kotlin, Compose) scans the QR, pairs once, then shows what the TUI shows: sessions, workspaces, panes, diff, settings, in jw's own theme.
- A *workspace* is any open folder: a project root, a worktree or a plain folder (`rust-tui.md:12`). The app enters all three.
- The bridge relays frames unchanged and adds auth, a protocol check and a small JSON control channel (palettes, workspaces, diff).
- Whoever holds a valid device token can type into every pane of the PC, so the bridge is the only gate (the daemon has no peer check, `crates/daemon/src/lib.rs:142,157`).
- The mobile feature bumps `PROTOCOL` once (REQ-50). UI references: `docs/specs/mobile-prototype.html` (the app) and `docs/specs/mobile-modal-prototype.html` (the TUI modal), both approved.

## Interfaces

| Piece | Where | Shape |
|---|---|---|
| Daemon service | `crates/daemon/src/mobile.rs` (new), stopped in `save_on_signal` between `save()` and `exit(0)` (`lib.rs:116-119`) | phases `off`, `starting`, `ready`, `failed`; its own threads and locks, none shared with panes (REQ-40) |
| Watchers | a list on `Daemon` (`lib.rs:161-172`); the daemon has no client registry or all-client broadcast today (`lib.rs:175-188`, `:361-363` fan out per workspace and per pane) | a client is a watcher from `MobileWatch` until it disconnects (`unsubscribe`, `lib.rs:1214-1220`) |
| Client to daemon | `proto.rs` `ClientMsg` | `MobileWatch`, `MobileStart{read_only}`, `MobileStop`, `MobileCode`, `MobileReadOnly{on}`, `MobileAnswer{request,yes}`, `MobileRevoke{device}` |
| Daemon to client | `proto.rs` `DaemonMsg` | `MobileState{phase,step,host,payload,expires_in,read_only,devices,error}`, `MobilePair{request,name,code}`, `MobilePairDone{request}`. `devices` are `{id,name,paired,last_seen,connected}`. `payload` is the QR text. |
| CLI | `crates/jw/src/mobile.rs`, `main.rs`, `help.rs`, `core/src/session.rs:18` | `jw mobile` (watch, start if off, show), `jw mobile stop`, `jw mobile devices`, `jw mobile revoke <device>` (these two read and write the store, so they work with mobile off) |
| TUI | `modal.rs:17` (`Modal`), `mod.rs:1349-1426` (`on_daemon`), `mod.rs:3270` (`submit_modal`), `draw.rs:623-692` (`status`), `settings.rs:48` (`ACTIONS`) | modal `Mobile` with views off, starting, ready, error, pair, devices; action `mobile`, group `go`, default key `m` (free, unbound); indicator in the status bar |
| QR | `crates/core/src/qr.rs` (new; the TUI and the CLI both draw it) | text to black-on-white modules; drawn two modules per cell with `▀` in 24-bit colour, which a terminal theme cannot remap |
| Bridge | `crates/web/src/bridge.rs` (jw-web; sync, threads, as `daemon/src/lib.rs:70-78`) | listens on `127.0.0.1:<port>`; WS **binary** message = one jw-proto frame, kind byte plus body, no length prefix; WS **text** message = control JSON. It connects to the daemon's own socket, one connection per phone. |
| Tunnel | `crates/web/src/tunnel.rs` (moved from `jw/src/mobile.rs`) | child `cloudflared tunnel --url http://127.0.0.1:<port>`; name read from its output; waits for DNS. `JW_CLOUDFLARED` names another program and skips the DNS wait (tests). |
| Raw frames | `proto/src/proto.rs` `read_raw`, `write_raw` | a frame's body without decoding it |
| Control, phone to bridge | JSON, tag `t` | `pair{token,name,code}`, `auth{device}`, `workspaces`, `sessions`, `diff{ws}`, `open{path}`, `worktree{project,branch}` |
| Control, bridge to phone | JSON, tag `t` | `ready{bridge:1,protocol,read_only,palettes,settings}`, `paired{device,id}`, `workspaces`, `sessions`, `diff`, `error{code,msg}` |
| Store | `<config dir>/mobile.json`, mode 0600, changed under a `flock` on `mobile.json.lock` | device id, name, SHA-256 of its token, created, last seen |
| Pairing | `pair.code` | 4 to 12 digits the phone generates and shows; `name` and `code` are stripped of control characters before anyone prints them |
| QR payload | `jw://<tunnel host>/<pairing token>` | the token is 128 bits, hex |
| Error codes | `error.code` | `unexpected`, `timeout`, `limited`, `expired`, `unknown`, `denied`, `revoked`, `read_only`, `unsupported`, `protocol`, `daemon`. The app treats `unknown` and `revoked` as final and does not retry, or its own retries lock it out (REQ-5). |
| App | `android/` | Gradle; modules `proto` (frame codec), `term` (VT emulator), `app` (Compose) |

## Acceptance

| REQ | Requirement |
|---|---|
| 1 | WHEN the daemon starts mobile, it SHALL listen on 127.0.0.1 only, start `cloudflared`, wait until the tunnel's name resolves in DNS, and only then offer the QR payload `jw://<tunnel host>/<token>`. |
| 2 | THE pairing token SHALL be 128 random bits, valid for one use and for 120 s. |
| 3 | WHEN a valid pairing token arrives, the bridge SHALL spend it at once, ask the person at the PC (REQ-36) with the device name and the code the phone sent, and only on yes issue a 256-bit device token, storing its hash only. |
| 4 | IF a connection sends anything but `pair` or `auth` before authenticating, THEN the bridge SHALL close it and forward nothing to the daemon. |
| 5 | IF five pair or auth attempts fail within 60 s from one address, THEN the bridge SHALL refuse that address for 5 min. |
| 6 | WHEN a device is revoked, by `MobileRevoke` or `jw mobile revoke`, the bridge SHALL reject its token and close its connections within 2 s. |
| 7 | WHERE read-only is on, the bridge SHALL drop every message except `Hello`, `Attach`, `Detach` and `List`, and every control request except the reads. |
| 8 | WHEN the bridge connects to the daemon, it SHALL send `Hello`; IF the daemon's `PROTOCOL` differs from the bridge's, THEN it SHALL refuse the phone with error `protocol`. |
| 9 | WHEN mobile stops, by `MobileStop` or because the daemon got SIGTERM, SIGINT or SIGHUP, the daemon SHALL kill `cloudflared` and close every phone, within 2 s and before it exits. |
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
| 27 | THE app, the bridge and the daemon SHALL NOT log terminal bytes or tokens. |
| 28 | THE Avisos tab SHALL list every pane whose agent is `Waiting`, with y and n. |
| 29 | IF a WebSocket handshake carries an `Origin` header, THEN the bridge SHALL refuse it with 403. |
| 30 | WHEN `jw mobile` runs and no daemon is listening, it SHALL start one as the TUI does; IF the daemon speaks another `PROTOCOL`, THEN it SHALL exit with a message saying what to do and show nothing. |
| 31 | WHILE a device is connected, the bridge SHALL ping it every 30 s and drop it after twice that in silence. |
| 32 | THE bridge SHALL close a connection that has not authenticated 10 s after its handshake (the wait for the person at the PC not counted), and SHALL hold at most 16 such connections. |
| 33 | THE daemon SHALL start mobile only on `MobileStart`, never by itself, a restart included. |
| 34 | WHEN `MobileStart` arrives while mobile is off, the daemon SHALL start it and send `MobileState` to every watcher at each step (cloudflared started, tunnel registered, waiting for DNS) and when ready; WHILE it is not off, `MobileStart` SHALL change nothing and be answered with the current state. |
| 35 | WHEN a client sends `MobileWatch`, the daemon SHALL answer with `MobileState` and send it every later change until the client disconnects. |
| 36 | WHEN a phone asks to pair, the daemon SHALL send `MobilePair` to every watcher, take the first `MobileAnswer`, and send `MobilePairDone` to the others; IF no watcher is attached, or none answers within 60 s, THEN it SHALL deny. |
| 37 | WHEN `MobileReadOnly` arrives, the daemon SHALL apply it at once to connected phones and to new ones. |
| 38 | WHEN `MobileCode` arrives, the daemon SHALL replace the pairing token, ending the old one, and send `MobileState`. |
| 39 | WHEN `MobileRevoke` arrives, the daemon SHALL revoke as REQ-6 says and send `MobileState`. |
| 40 | THE mobile service SHALL NOT hold a pane, workspace or session lock while it waits for the network, the tunnel or a person. |
| 41 | WHEN `jw mobile` runs, it SHALL watch, start mobile if it is off, print each step, then the QR and the link, and answer `MobilePair` in the terminal; Ctrl-C SHALL leave mobile running; `jw mobile stop` SHALL send `MobileStop`. |
| 42 | THE TUI SHALL send `MobileWatch` right after `Hello` and keep the latest `MobileState`. |
| 43 | WHEN the leader and the `mobile` key (default `m`) are pressed, the TUI SHALL open the Mobile modal; the key SHALL be a row of `ACTIONS` in group `go`, so which-key lists it and Settings can rebind it. |
| 44 | THE Mobile modal SHALL have the views and keys of the prototype: off (`↵` start, `d` devices, `esc` close); starting (`s` stop, `esc` hide); ready (`n` new code, `d` devices, `r` read-only, `s` stop, `esc` hide); error (`↵` try again, `esc` close); devices (`↑↓` select, `x` revoke, which asks `y` or `n`, `esc` back). `esc` SHALL hide the modal and never stop mobile. |
| 45 | WHEN `MobilePair` arrives and no other modal or picker is open, the TUI SHALL open the pair question over the screen with the chip CONFIRM; `y` SHALL answer yes, and `n` or `esc` no. WHILE another modal is open, it SHALL show `? pair request` in the status bar and ask when that modal closes. WHEN `MobilePairDone` arrives, it SHALL close the question. |
| 46 | WHILE mobile is not off, the status bar SHALL show at its right `◉ mobile` in cyan, `◉ N phone` or `phones` in green when N are connected, and a yellow `ro` when read-only. |
| 47 | THE ready view SHALL draw the QR through `core::qr`, with the time left before the code ends; IF the area is smaller than 66×28, THEN it SHALL not draw the QR, SHALL say what size it needs, and `c` SHALL copy the link (OSC 52). |
| 48 | WHEN the code has expired, the ready view SHALL say so and offer `n`. |
| 49 | WHEN `MobileState` is `failed`, the error view SHALL show the daemon's message, such as "cloudflared is not installed; brew install cloudflared". |
| 50 | THE mobile feature SHALL bump `PROTOCOL` once, to the first number free past main's when it merges (`proto/AGENTS.md:23-24`). |

---

## Rationale

- **RAT-1 The daemon hosts it** (the first version of this spec had it in `jw mobile`'s own process). The user wants mobile inside the TUI and still on after closing jw to walk away from the PC; the daemon is what outlives the TUI, and a pairing question can only be answered by a watcher the daemon already talks to. The price is a network-facing component in the process that holds the user's work, a child process, and a `PROTOCOL` bump (RISK-14, REQ-40, REQ-50).
- **RAT-2 The relay still adds no message.** A phone speaks the existing frames; only the control of mobile itself is new (`Mobile*`).
- **RAT-3 Cloudflare quick tunnel.** User's choice, 2026-10-11. Cloudflare ends TLS, so the bridge needs no TLS crate and listens in plain text on loopback.
- **RAT-4 Sync WebSocket, threads.** The workspace has no async runtime and the daemon is thread-per-client (`lib.rs:70-78`).
- **RAT-5 Native Kotlin.** User's choice, 2026-10-11. iOS comes later, so `proto` and `term` hold no UI and can be ported.
- **RAT-6 Own spec file.** `rust-tui.md` is long and `AGENTS.md` names it as "the spec"; `AGENTS.md` points here.
- **RAT-7 Emulator on the phone.** The TUI also receives raw bytes; same input, same model.
- **RAT-8 The PC confirms pairing, and nobody watching means no.** The QR sits on a screen that may be shared, so a token alone is not enough and the person must be at the PC, in the TUI or in `jw mobile`. This also settles a question the prototype raised: with nobody watching there is no one to ask.
- **RAT-9 Look of the app.** Terminal character with touch rows and sheets; the user rejected a pure terminal copy and a pure card UI.
- **RAT-10 The bridge keeps relaying through the daemon's own socket**, one connection per phone, instead of calling the daemon's internals. The relay and the auth gate stay the code that is already tested, and the daemon needs no special case for a phone.
- **RAT-11 `jw mobile` is a client**, so there is one host. Ctrl-C leaves mobile running because stopping it from a terminal you are leaving would defeat the point; `stop` is explicit.
- **RAT-12 The QR drawing is in core**, as both the TUI and the CLI need it and the TUI does not depend on jw-web (`AGENTS.md` dependency rule).
- **RAT-13 The pair question jumps in over the screen**, as the user approved in the prototype, except over another modal, where it would eat typed text (REQ-45).
- **RAT-14 Prototypes.** The app and the modal were approved by the user on 2026-10-11 (`mobile-prototype.html`, `mobile-modal-prototype.html`).

## Risks

- **RISK-1 Token holder owns the PC.** REQ-2 to 7 limit it. Unverified: whether a quick-tunnel URL can leak through `cloudflared` logs.
- **RISK-2 Last `Resize` wins** (the `Resize` arm of `handle` in `lib.rs`). A phone resize would shrink the PC's pane; REQ-24 is the guard.
- **RISK-3 Quick tunnels.** No SLA and a new URL each run. WebSocket over one **works** (checked 2026-10-11, `cloudflared` 2026.10.0: upgrade 101, binary frames, 256 raw bytes intact, about 95 ms round trip). The name is **not reachable when `cloudflared` says it is**: `Registered tunnel connection` came about 60 s before DNS resolved it, hence REQ-1. A phone that only watched stayed connected 151 s, answering pings every 30 s; Cloudflare's own idle limit is unmeasured.
- **RISK-4 Approve bytes.** y and n may not match the agent's prompt UI; Waiting detection comes from `AgentState`. Bytes are configurable.
- **RISK-5 Emulator.** Candidate: Termux's `terminal-emulator` (licence and kitty replay support **unverified**).
- **RISK-6 Android kills background sockets.** No alerts when the app is backgrounded (OPEN-3).
- **RISK-7 Cellular cost.** A resync sends a snapshot of up to 2 MiB (`outbox.rs`). The bridge may need to throttle.
- **RISK-8 New dependencies** in a dep-light workspace: a sync WS crate, `qrcode`, `sha2`.
- **RISK-9 Diff source.** The TUI draws diffs client-side (`tui/src/tui/diffview.rs`). Whether git diff generation can move to `core` for the bridge is **unchecked**.
- **RISK-10 Behind `cloudflared` every connection comes from 127.0.0.1.** Measured: the tunnel adds `Cf-Connecting-Ip`, so REQ-5 keys on it, trusted only because the listener is loopback and `cloudflared` is its one client.
- **RISK-11 A web page can open `ws://127.0.0.1:<port>`.** Measured: `Origin` passes through the tunnel, a phone app sends none, so REQ-29 refuses it. Auth is the second guard.
- **RISK-14 The daemon holds the user's real work and now also a network-facing part.** A panic or a hang in the bridge or the tunnel must not stall a pane or the signal path: its own threads, no shared pane lock (REQ-40), and a stop that gives up after 2 s (REQ-9).
- **RISK-15 `SIGKILL` on the daemon leaves `cloudflared` running.** Its port is free-chosen each time, so a leftover tunnel points at nothing, but it stays a process. Unsolved; `jw server stop` sends SIGTERM and is fine.
- **RISK-16 `MobileState` carries the pairing token** to every watcher. The socket is 0600, so only this user can be one; REQ-27 forbids logging it.
- **RISK-17 `PROTOCOL` and the numbering of REQs collide with parallel branches without a git conflict** (main went 8 to 9 while this was built). Take the next free numbers at merge time.
- **RISK-18 The daemon cannot tell a TUI from a CLI client** (`ClientMsg::Hello` carries no kind, `proto.rs:57-59`). It does not need to: `MobileWatch` is what makes a watcher.
- **RISK-19 Palettes live in the TUI crate** (`tui/src/theme.rs`), and the daemon must not depend on the TUI. W1 has to get them to the bridge without that edge, for example by the `jw` binary handing a callback to the daemon at start.

## Open questions

- **OPEN-1** Resize default. Chosen: never resize, offer "fit to phone".
- **OPEN-2** Named tunnel (Cloudflare account, stable URL, no re-scan) instead of a quick one. Chosen: quick.
- **OPEN-3** Push alerts while backgrounded (needs a relay or ntfy). Chosen: none in v1.
- **OPEN-4** Package `android/` in this repo, minSdk 26. Chosen as written.
- **OPEN-5** Default approve and reject bytes.
- **OPEN-6** Bring mobile back after a daemon restart. Chosen: no (REQ-33), because each start is a new public URL the user should have asked for.

## Parts

| Part | What | REQ | Main files |
|---|---|---|---|
| M1 ✓ `65cc24f` | Bridge relay, protocol check, stop | 8–11 | `crates/web/src/bridge.rs`, `proto.rs`, `jw/tests/web.rs` |
| M2 ✓ `720261b` | Pairing, auth, limits, revoke, read-only, tunnel and QR as the `jw mobile` process (D1 and D2 move the hosting) | 1–7, 29, 31, 32 | `crates/web/`, `jw/src/mobile.rs` |
| W1 | `palettes` and `settings` in `ready` | 17 | `crates/web/src/bridge.rs`, the `jw` binary or the daemon start |
| D1 | The daemon hosts mobile: tunnel moves to jw-web, `daemon/src/mobile.rs`, the messages, watchers, stop on signal, `PROTOCOL` bump | 1, 3, 6, 9, 33–40, 50 | `daemon/src/mobile.rs`, `daemon/src/lib.rs`, `web/src/tunnel.rs`, `proto.rs` |
| D2 | `jw mobile` as a client, `jw mobile stop`, shared QR in core | 30, 41 | `jw/src/mobile.rs`, `core/src/qr.rs` |
| T1 | The TUI: Mobile modal, `^␣ m`, indicator, pair question | 42–49 | `tui/src/tui/modal.rs`, `mod.rs`, `draw.rs`, `settings.rs` |
| M3 | Android shell: scan, pair, reconnect, version screen, theme | 14–17, 27 | `android/app`, `android/proto` |
| M4 | Workspaces, sessions, enter, approve, Avisos | 12, 18–20, 28 | `android/app`, `crates/web/` |
| M5 | Terminal surface, keys, leader sheet, resize | 21–24 | `android/term`, `android/app` |
| M6 | Changes and diff | 13, 25 | `crates/web/`, `android/app` |
| M7 | Settings, touch and mode chrome, polish | 26 | `android/app` |

Order: W1, D1, D2, T1 on the server track and M3 on the app track run in parallel; then M4 to M7. The work orders for the two agents that build them are `docs/specs/mobile-work/server.md` and `docs/specs/mobile-work/android.md`.

## Corrections

- RAT-1 first said the bridge belongs **outside** the daemon, in `jw mobile`'s own process, so the daemon never listens on a network. The user asked for mobile inside the TUI and alive after closing it, which only a daemon-hosted server gives; RAT-1 now says that and RISK-14 keeps what the first version was protecting.
- The first version said the daemon and `PROTOCOL` do not change. They change once (REQ-50).
- Ctrl-C in `jw mobile` used to stop the tunnel (M2). It now leaves mobile running (REQ-41); `jw mobile stop` stops it.
- REQ-30 first made `jw mobile` fail when no daemon was running. The server now lives in the daemon, so it starts one.
- RISK-11 pointed at "REQ-4a"; the rule is REQ-29.
- REQ-3 first had the PC confirm the device name the phone sent. Anyone holding the QR could pair as any name. The phone now shows a code, the PC prints it, and the person compares; the token is spent before the question.
- REQ-1 first offered the QR once `cloudflared` said the tunnel was registered; the name did not resolve for about a minute more (RISK-3).
- The bridge was to live in `crates/jw/src/mobile/`; it lives in its own crate, `crates/web` (jw-web, first named jw-mobile), because `jw` is bin-only and its integration tests could not reach a bridge inside it.
- `jw-proto` had no byte-level frame API: `read_raw` and `write_raw` were added and `MAX_FRAME` made public.
- The `Theme` message carries only `dark`, `fg`, `bg` (`proto.rs:172-177`), so palettes travel on the control channel.
- The daemon does not compare protocol versions (`lib.rs:540-542`); the client does, hence REQ-8.
- `crates/daemon/AGENTS.md` cites old line numbers (`serve` `:465`, now `:497`; `handle` `:508`, now `:540`; `attach` `:677`, now `:710`). D1 fixes them when it edits that file.

## Tests

- **Bridge, unit** (done in M2): token one-use and expiry, rate limit, hash-only storage, read-only filter, revoke, ping, `Origin`, pre-auth deadline and cap.
- **Tunnel, unit:** the name is found in `cloudflared`'s output; the registered line; the DNS wait gives up; the child dies with its handle.
- **Daemon, integration** (`crates/jw/tests/`, own socket dir, a stub program named by `JW_CLOUDFLARED` that prints a tunnel line, kill only its own pid): `MobileStart` walks the steps to ready; `MobileStop` and SIGTERM kill the stub; two watchers both get `MobilePair`, the first answer wins and the other gets `MobilePairDone`; no watcher means denied; `MobileWatch` gets the state at once and after each change; `MobileStart` twice changes nothing; the daemon starts with mobile off; a pane keeps answering while the stub hangs (REQ-40).
- **TUI, unit:** each modal view's keys and what it draws; the pair question opens over the screen and is held back by another modal; the indicator's three forms; the too-small fallback; `core::qr` gives a square with a quiet zone.
- **Android:** frame-codec round trips against vectors generated by `jw-proto`; emulator golden screens; Compose screenshots against the prototype.
- **By hand:** `scripts/jw-test mobile`, a phone camera on the QR, the modal's pair question, closing the TUI and checking mobile is still on. Never `pkill jw`.
