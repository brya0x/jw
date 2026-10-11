# android

The native phone app for `jw mobile`. It talks to the bridge over a WebSocket, never to the daemon.
Spec: `docs/specs/mobile.md` (UI: `docs/specs/mobile-prototype.html`). Notes for the spec: `SPEC-NOTES.md`.

## Build and test

```sh
export ANDROID_HOME=~/Library/Android/sdk   # or a git-ignored local.properties with sdk.dir=...
./gradlew assembleDebug
./gradlew :proto:test :app:testDebugUnitTest
```

JDK 17, Gradle wrapper 8.14.3, AGP 8.13.0, Kotlin 2.2.21, minSdk 26, compileSdk 36. One test waits 15 s on purpose
(the PC can take about a minute to answer a pairing).

## Where things live

- `proto/` plain Kotlin/JVM, no Android classes (portable to iOS later): `Frame.kt` (codec, `PROTOCOL`),
  `Control.kt` (messages, `ErrorCode`, palettes with the One Dark/Light fallback), `Qr.kt`, `Reconnect.kt`
  (pure policy: no clock, no sleeping).
- `app/` `Bridge.kt` (OkHttp WebSocket as a `Flow`), `Session.kt` (pairing, reconnect, the `UiState` the screens show),
  `KeystoreTokenStore.kt` (AES-GCM, key in the Android Keystore), `Scanner.kt` (CameraX + ZXing), `Theme.kt`,
  `MainActivity.kt` (screens). Tests use `MockWebServer` as a fake bridge (`Fakes.kt`).

## Rules

- No tokens, pairing codes or terminal bytes in logs, exceptions or `toString` (REQ-27). Types that hold one redact it.
- `unknown` and `revoked` are final: drop the device and scan again, never retry (REQ-5).
- Never send an `Origin` header: the bridge answers 403.
- Bump `PROTOCOL` in `proto/Frame.kt` when the Rust `PROTOCOL` moves.
