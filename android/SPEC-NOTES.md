# Notes for the spec (from the app agent)

- **Prototype is out of date and lacks a screen.** It says "jw-proto v8" (real `PROTOCOL` is 9). REQ-16's update
  screen is not in it: M3 builds it from the connect screen's layout and plain text, so it is **not approved UI**.
  The reconnecting screen and the notices on the scan screen are in the same position.
- **`ready.palettes`** is the array of the spec (`{name,dark,colors}`, `bridge.rs`): the app takes One Dark / One Light by name,
  else the first dark / light theme, role by role over the built-in fallback. `settings` is kept raw and unused; which theme
  the user runs is in `ready.settings` (`{dark, light}` theme names, seen in the bridge tests); the app does not use it yet (TODO M4).
- `PROTOCOL` moved to 10 while M3 was built; the app's constant is in `proto/Frame.kt`.
- **`paired` can be followed by `error daemon` with no `ready`.** The app stores the device on `paired`, before `ready`.
- Silent closes exist (close before auth, the 16-connection cap, 60 s of silence): the app treats them as a network failure.
- Backoff is 5, 10, 20, 30 s (cap 30 s) so that five failures never fit in the REQ-5 60 s window; `limited` waits 5 min.
- Pairing failures are never retried with the same token, except DNS failures (the token is not consumed).
- **Quick tunnels only.** `Qr.kt` accepts only `*.trycloudflare.com` (no port, userinfo, `..`, trailing dot), so a spoofed QR
  cannot collect the token and code. A named tunnel (OPEN-2) will need this rule widened.
- **Cleartext is off** (`network_security_config.xml`); minSdk 26 and 27 would allow it otherwise.
- **Needs the user's OK (not in the approved prototype):** the protocol-mismatch screen, the reconnecting screen, the notices on the scan screen.

## Dependency ages (checked 2026-10-11 from the `Last-Modified` of each artifact; rule: at least 2 weeks)

| Dependency | Version | Released |
|---|---|---|
| Gradle distribution | 8.14.3 | 2025-07-04 |
| Android Gradle Plugin | 8.13.0 | 2025-09-02 |
| Kotlin (and compose, serialization plugins) | 2.2.21 | 2025-10-23 |
| Compose BOM | 2026.06.01 | 2026-07-01 |
| activity-compose | 1.12.4 | 2026-02-11 |
| CameraX | 1.6.0 | 2026-03-25 |
| OkHttp / MockWebServer | 4.12.0 | 2023-10-17 |
| ZXing core | 3.5.4 | 2025-11-11 |
| kotlinx-serialization-json | 1.9.0 | 2025-06-27 |
| kotlinx-coroutines | 1.10.2 | 2025-04-08 |
| Guava (android flavour, for `ListenableFuture`) | 33.4.8-android | 2025-04-14 |
| JUnit | 4.13.2 | 2021-02-13 |

Not done: `slowPcAnswerIsWaitedFor` still sleeps 15 s for real; a virtual clock cannot drive MockWebServer's real I/O.

## On the phone (CPH2417, Android 15), 2026-10-11
Checked: install, launch, scan screen with the camera granted, dark and light system theme, landscape, Back, force-stop and reopen. No crash.
Not checked: the camera permission dialog (the ROM granted it on the first tap, and `pm revoke`/`pm clear` are refused to adb, so "deny" and
"don't ask again" were not driven), and the pairing, mismatch, reconnecting and connected screens (they need a bridge; the brain runs the e2e).
Look: header, mode chip and status bar, 48 dp rows and buttons, JetBrains Mono bundled (`FONTS.md`), the prototype's palette roles.
The scan screen in landscape gives the camera the whole width; the title shrinks behind it. Fine for now, worth a second look in M4.

## M4 (workspaces, sessions, panes, y and n)

- **The brief's "W2 SHAPE" label does not exist in `docs/specs/mobile.md`**; the shape is in the Interfaces rows for the `workspaces` and
  `sessions` answers. The app follows those rows and the assertions of `a_paired_phone_is_told_the_workspaces_and_sessions_of_the_pcs_jw`.
- **The join `PaneInfo.stream == item.id` is UNCONFIRMED for kind `root` and `folder`** (confirmed by the server agent for worktrees only).
  `WorkspacesTest.joinIsByStreamId` pins it by id for all three kinds and says so; if the server answers otherwise, that test and
  `Workspaces.panesOf` are the only places to change.
- **Marks**: `✻ ? ⚡` as `App::marks` in `tui/src/tui/mod.rs:1130` (the spec cites `:960-979`, stale; `crates/tui/AGENTS.md` cites `:540`, also stale).
  The merged-PR mark `⚑` is left out: the bridge sends no PR state (`pr` is only the number jw recorded). The legend lists the other four.
- **Refresh**: `List` every 2 s only while the workspaces, avisos or workspace screen is on screen and the activity is resumed (`RefreshRule`).
  `sessions` and `workspaces` are asked at every `ready` (REQ-15) and when another session is picked; they are not polled.
- **y and n** (REQ-20, OPEN-5, RISK-4): default approve = Enter (`\r`), reject = Esc (`\x1b`); Settings offers Enter, `y`, `y`+Enter and Esc, `n`, `n`+Enter.
  Buttons are enabled only while the latest `Panes` (at most 2 s old) says Waiting for that pane, once per snapshot (`AnswerRule`); a toast confirms.
  **Not verified against a real agent prompt**: it needs the user to try it by hand.
- **Needs the user's OK (not in the prototype):** the `Escanear otro código` button and unreachable text on the reconnecting screen; the
  answer-bytes section of Settings; no "nueva sesión", "abrir carpeta" or "nuevo worktree" actions (the bridge has no such requests yet); the search button and the
  `^␣` leader sheet were left out until M5 (now built, REQ-23).
- `read_only` answers to `Input` show the toast "solo lectura"; with a read-only phone the y and n buttons stay disabled.

## Connection screens (REQ-51 to REQ-54), built from the approved prototype

- **Which failure screen each cause gets.** Only five are approved, so each cause is drawn as the nearest one (`Session.pairWhy`, `ScanWhy`):
  `denied` and a stored device the PC forgot (`unknown`, `revoked`) are "Tu PC rechazó este teléfono"; `expired`, `unknown`, `timeout` and any other
  bridge error while pairing are "Ese código ya no sirve"; `limited` is "Demasiados intentos"; a DNS give-up, a dead tunnel or a network error while
  pairing are "No encuentro tu PC". The bridge's own `protocol` and `daemon` errors while reconnecting keep the plain reconnecting text (not in the prototype).
- **"No encuentro tu PC" draws only what was measured.** `internet · datos móviles / wifi` is `ConnectivityManager` (`ACCESS_NETWORK_STATE`);
  `Cloudflare responde` is drawn only when the handshake got an HTTP answer that was not an upgrade (`Failure.Http`, Cloudflare's 530 for a gone tunnel);
  the PC's row is the one that failed. With nothing measured the screen shows just the PC row. "reintentar" appears only where a retry means something
  (the reconnecting screen is retried by itself, and a pairing token is spent), so it is not drawn.
- **Pairing:** "PC alcanzado" is shown once the WebSocket opened (`BridgeEvent.Open`); the prototype's "(60 s)" is not drawn (no number in the spec to take it from).
- **Paste link** is a text field, never a clipboard read; the link is not logged or stored.
- **Not done / not tried by hand:** the camera permission "blocked" screen is drawn from a flag in `ConnectScreensTest` (adb cannot revoke it on this ROM);
  the torch is hidden when the phone has no flash unit.

## Wifi first, the tunnel as the fallback (L3, REQ-59 to REQ-64)

- **The pin.** `PinnedTrust` trusts exactly the certificate whose SHA-256 is the `fp` of the QR or of `endpoints`; the chain, the name and the dates are not looked at, and an empty chain is refused. It is built only for a wifi address (`Bridge.pinnedClient`, a new `OkHttpClient` derived from the base one); the tunnel's connection never goes through it and keeps the system's checks. Proved on the phone's own TLS stack (`PinnedTlsDeviceTest`, Conscrypt, CPH2417 Android 15): the right pin connects, a wrong one is refused before anything is sent, an unpinned client refuses a self-signed certificate.
- **What a host may be.** The pairing token and the device token go only to a quick-tunnel host (`*.trycloudflare.com`) or a private IPv4 (10/8, 172.16/12, 192.168/16) with a port and a 64-hex fingerprint (`Hosts`). A QR or an `endpoints` entry that fails this is dropped, and the rest of the message is kept. What is read back from the encrypted file is checked again.
- **Route order.** Each connection tries the wifi first with a 3 s limit for the socket to open, then the tunnel. A pairing falls to the tunnel only when no connection opened on the wifi, so the single-use token was never sent. An attempt that reached `ready` is never retried on the other route.
- **`endpoints`** is accepted only after this connection's `ready`; it updates the stored device (tunnel host and wifi address with pin), and only when something changed.
- **Back on the wifi.** While on the tunnel with a wifi address held and the phone on wifi (`TRANSPORT_WIFI`), every 30 s the app does a TCP and TLS handshake against the pin (no token, `Bridge.probe`); when it answers, the connection moves, without a "reconnecting" screen and without a failure counted.
- **Store.** `device.bin` is now `v2\n<device>\n<tunnel host>\n<wifi host:port>\n<fp>`; the first form (`<host>\n<device>`) is still read.
- **Local network permission.** Not looked up: Android 16 may gate access to the local network for apps targeting it; the phone here is Android 15 and connects without it. To check before targeting 16.
- **Not tried by hand yet:** pairing with the real camera over the wifi against the real daemon, and the fall to the tunnel with the phone off the wifi.
