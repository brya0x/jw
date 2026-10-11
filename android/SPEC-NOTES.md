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
