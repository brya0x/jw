# android

The native phone app for `jw mobile`. It talks to the bridge over a WebSocket, never to the daemon.
Spec: `docs/specs/mobile.md` (UI: `docs/specs/mobile-prototype.html`). Notes for the spec: `SPEC-NOTES.md`.

## Build and test

```sh
export ANDROID_HOME=~/Library/Android/sdk   # or a git-ignored local.properties with sdk.dir=...
./gradlew assembleDebug
./gradlew :proto:test :term:test :app:testDebugUnitTest
```

On the phone (only our own package; connectedDebugAndroidTest uninstalls the app after, so reinstall the debug APK):

```sh
ANDROID_SERIAL=<serial> ./gradlew :app:connectedDebugAndroidTest
# Keystore across a process death: run `save`, force-stop, run `load`
adb shell am instrument -w -e class dev.jw.mobile.KeystorePersistenceTest#save dev.jw.mobile.test/androidx.test.runner.AndroidJUnitRunner
adb shell am force-stop dev.jw.mobile
adb shell am instrument -w -e class dev.jw.mobile.KeystorePersistenceTest#load dev.jw.mobile.test/androidx.test.runner.AndroidJUnitRunner
```

JDK 17, Gradle wrapper 8.14.3, AGP 8.13.0, Kotlin 2.2.21, minSdk 26, compileSdk 36. One test waits 15 s on purpose
(the PC can take about a minute to answer a pairing).

## Where things live

- `proto/` plain Kotlin/JVM, no Android classes (portable to iOS later): `Frame.kt` (codec, `PROTOCOL`),
  `Control.kt` (messages, `ErrorCode`, palettes with the One Dark/Light fallback), `Qr.kt`, `Reconnect.kt`
  (pure policy: no clock, no sleeping).
- `term/` plain Kotlin/JVM, the VT emulator (`Terminal`, a grid of `Line`s, `Width`): bytes in, cells out, no Android
  classes. Its tests feed streams to it and compare with the screen the daemon's own vt100 leaves
  (`src/test/resources/golden`; `regen.sh` rebuilds the expected files with `crates/jw/examples/cells.rs`). What
  vt100 lacks or does differently from xterm (insert mode, tab stops, line drawing, REP, no-wrap, erase attributes) is
  in `XtermTest`. It never replies to a program: the daemon answers those.
- `app/` `Bridge.kt` (OkHttp WebSocket as a `Flow`), `Session.kt` (pairing, reconnect, the `UiState` the screens show),
  `KeystoreTokenStore.kt` (AES-GCM, key in the Android Keystore), `Scanner.kt` (CameraX + ZXing), `Theme.kt`,
  `MainActivity.kt` (screens). Tests use `MockWebServer` as a fake bridge (`Fakes.kt`).

## Rules

- No tokens, pairing codes or terminal bytes in logs, exceptions or `toString` (REQ-27). Types that hold one redact it.
- `unknown` and `revoked` are final: drop the device and scan again, never retry (REQ-5).
- Never send an `Origin` header: the bridge answers 403.
- Bump `PROTOCOL` in `proto/Frame.kt` when the Rust `PROTOCOL` moves.
