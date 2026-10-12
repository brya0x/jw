# Work order: the app track (Android, Kotlin)

You are the **app agent**. You build the native Android app for `jw mobile`, in `android/`. Another agent, the **server agent**, works in a second pane of this same workspace on the Rust side. The **brain** (the main Claude session in this workspace) reads your progress, reviews each part and writes to you.

## Where you are

- Checkout: `/Users/bastacio/ws/jw-wt/mobile-app`, branch `feat/mobile-app`. **The server agent shares this checkout and its git index.** That is why the git rules below are strict.
- You touch **only `android/`**. You do not edit Rust, the scripts, `AGENTS.md` or the spec (`docs/specs/mobile.md` is the server agent's). When you find something the spec should say or got wrong, write it in `android/SPEC-NOTES.md` and the brain folds it in.
- Read first: the repo `AGENTS.md` (rules, and the note that UI work follows an approved prototype), then `docs/specs/mobile.md` (Contract, Interfaces, REQ 14 to 17 and 27 for your first part, and the Error codes row), then `docs/specs/mobile-prototype.html` (the approved look of the app: open it, read its CSS and JS; the themes are jw's palette roles, the style is terminal character with touch-sized rows and bottom sheets), then the protocol as it is really built: `crates/web/src/bridge.rs`, `crates/web/src/auth.rs`, `crates/proto/src/proto.rs` (frames: `write_frame`, `read_raw`), and `crates/jw/src/mobile.rs` (how the QR is made). The code is the truth; the spec is anchored to it.

## What you can rely on from the phone side

- The QR holds `jw://<tunnel host>/<pairing token>` (token: 32 hex characters, valid once, for 2 minutes). The WebSocket is `wss://<tunnel host>/` (Cloudflare ends TLS). No `Origin` header may be sent: a handshake that carries one is refused with 403, so make sure the HTTP client does not add one.
- First message is text JSON, tag `t`: `{"t":"pair","token":..,"name":..,"code":"482913"}` or `{"t":"auth","device":"<64 hex>"}`. The app generates the **6-digit code**, shows it large while it waits, and the person at the PC compares it. A pair answer is `paired{device,id}` then `ready{bridge,protocol,read_only,...}`; an auth answer is `ready`. Errors are `{"t":"error","code":..,"msg":..}` with the codes in the spec. **`unknown` and `revoked` are final: drop the stored token and go back to scanning, never retry**, or the app's own retries lock it out for five minutes (REQ-5). `limited` means wait.
- After `ready` the WebSocket carries binary messages: one jw-proto frame each, **without** the 4-byte length: a kind byte (0 is a JSON message, 1 is a u32 big-endian head length, a JSON head, then raw bytes), exactly as `write_frame` writes them minus the length.
- The server agent is adding `palettes` and `settings` to `ready` (part W1). Until it lands, use a built-in One Dark palette and read the new fields when they appear (they are optional at first).
- Waiting for the PC's answer can take up to a minute. The bridge pings every 30 s; the WebSocket library answers pings by itself.

## Your part now: M3 (REQ 14, 15, 16, 17, 27)

An Android app that scans, pairs, stays connected and shows a first screen. **Not** the workspaces, terminal or diff (M4 and later).

- Project layout: `android/` Gradle with Kotlin DSL and a version catalog, `minSdk 26`, `compileSdk 36` (the SDK here has platforms 35 and 36), JDK 17. Modules: `proto` (a plain Kotlin/JVM library: frame codec, control-message types, error codes, the reconnect policy as a pure state machine; no Android classes, so it can be ported to iOS later) and `app` (Compose, Material 3).
- `app`: a QR scanner (CameraX with a barcode decoder; pick one with the advisor, prefer a bundled model over one that needs Play services), the pairing screen, secure storage of the device token (Android Keystore with AES-GCM; never a plain preference), a WebSocket client (OkHttp), reconnect with backoff, a screen for a protocol mismatch (`ready.protocol` differing from the one the app was built for: say what to update and do not attach), a first screen that shows the connection (host, protocol, read-only), and the theme from `ready.palettes` with the One Dark fallback, following the system dark or light setting.
- Retries: a DNS failure on the first connect is normal for a minute or two after a tunnel is created (the phone's resolver may lag the PC's), so retry those for up to about three minutes before giving up. After `unknown` or `revoked`, never.
- **No tokens, codes or terminal bytes in logs** (REQ-27), not even at debug level.
- Look: follow the prototype, 48 dp touch targets, readable in both themes.

## Tests, and what you can check

- JVM unit tests: the frame codec (build frames by hand from the format above, both kinds, with 256 raw bytes), the control JSON, the reconnect state machine (backoff, final errors, DNS grace), the token store behind an interface with a fake.
- A fake bridge: OkHttp's `MockWebServer` speaking the documented protocol, for the pairing flow and the error codes. No network and no Rust needed.
- `./gradlew assembleDebug` and the unit tests must pass before each commit. Gradle is not installed here: bootstrap the wrapper (download the Gradle distribution and the wrapper jar from `services.gradle.org`, or `github.com/gradle`), pin versions at least two weeks old, commit the wrapper.
- A device or emulator: `adb devices` was empty when this was written and there is no emulator image. If a phone appears (`~/Library/Android/sdk/platform-tools/adb devices`), install and try the app and say so in `STATUS.md`. If not, say plainly that it was not run on a device; do not pretend.
- A real end-to-end with the real bridge needs a Rust build and a tunnel. You may build with `CARGO_TARGET_DIR` set to a directory of your own (the server agent uses `target/`) and run `jw mobile` from there **only with isolated state**: a short working directory, a **relative** `JW_SOCKET`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` in a temp directory, an isolated `jw daemon` that you started and whose pid you kill yourself. It takes about 80 s for the QR to appear. Do this once at the end of the part at most.

## Done means

- `./gradlew assembleDebug` and the unit tests pass; run them and read the output.
- `android/.gitignore` keeps build output and local files out; no secrets, no keystore files, no `local.properties`.
- `android/AGENTS.md` says how to build, test and where things live (short).
- Append one line to `.mobile-work/android/STATUS.md`: date, part, what it does, the commit hash. Then read `.mobile-work/android/INBOX.md`: the brain may have written to you.

## Git, because the index is shared

- Stage and commit **only `android/`**: `git add android` then `git commit -m "<message>" -- android`. The `--` with a path makes the commit take only that path even if the server agent has things staged.
- **Never** `git add -A`, `git add .`, `git commit -a`, `git stash`, `git checkout`, `git reset`, `git merge`, `git rebase`, `git push` or `gh`. Never touch anything outside `android/`.
- If `git` reports `index.lock`, wait a few seconds and retry. Do not delete the lock.
- Commit messages: conventional (`feat(android): M3, ...`), no `Co-Authored-By` unless the brain says so. One commit per part; a part this size may take a few commits if you tell the brain.
- Do not bump any version.

## Rules from the user

- **Never `pkill`.** Kill only a pid you started. Never touch the user's real jw daemon (`/Users/bastacio/.cargo/bin/jw daemon`); your own pane belongs to it.
- Never bind a key with Alt in anything you build for a keyboard.

## How you work

- Searching is not your job. Send every "where does X live" to the `scout` subagent (it runs on Haiku), one focused question each, several in parallel when you can. Read inline only the file you are about to edit.
- Call the advisor before you start and before you say a part is done.
- If something is the user's decision, or you are stuck after two real attempts, write `BLOCKED: <what and why>` as a line in `.mobile-work/android/STATUS.md` and stop until `INBOX.md` answers. Do not ask the user directly: the brain relays.
- When M3 is committed, append `M3 DONE` with a short summary, then wait for `INBOX.md` to say what comes next (M4 and later need server work first).
