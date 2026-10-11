# Notes for the spec (from the app agent)

- **Prototype is out of date and lacks a screen.** It says "jw-proto v8" (real `PROTOCOL` is 9). REQ-16's update
  screen is not in it: M3 builds it from the connect screen's layout and plain text, so it is **not approved UI**.
  The reconnecting screen and the notices on the scan screen are in the same position.
- **`ready.palettes` shape is assumed** until W1 lands: `{"dark":{"bg":"#rrggbb",...},"light":{...}}` with the 12 roles
  of the prototype (bg panel fg dim line sel blue green yellow red magenta cyan). Missing roles fall back to One Dark/Light.
  `settings` is kept as raw JSON and unused.
- **`paired` can be followed by `error daemon` with no `ready`.** The app stores the device on `paired`, before `ready`.
- Silent closes exist (close before auth, the 16-connection cap, 60 s of silence): the app treats them as a network failure.
- Backoff is 5, 10, 20, 30 s (cap 30 s) so that five failures never fit in the REQ-5 60 s window; `limited` waits 5 min.
- Pairing failures are never retried with the same token, except DNS failures (the token is not consumed).
