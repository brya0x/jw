# Fidelity: app screens against `docs/specs/mobile-prototype.html`

Phone shots come from `ConnectScreensTest` (CPH2417, Android 15, dark and light, 2026-10-11), compared with headless-Chrome renders of the prototype.

| Screen | State | Differences |
|---|---|---|
| connect | checked, dark and light | none seen |
| permission (ask, blocked) | checked | the blocked screen is drawn from a flag: adb cannot revoke the permission on this ROM, so it was not reached by hand |
| scan (camera A), paste sheet | checked with a stand-in feed | the real camera preview and the torch were not tried by hand; the prototype's "maqueta: leer QR" is not shipped |
| pairing | checked | "(60 s)" is not drawn (no number to take it from); "PC alcanzado" only once the socket opened; the spinner glyph is smaller than the prototype's |
| denied, expired, limited | checked | none seen |
| no encuentro tu PC | checked | rows drawn only when measured (see `SPEC-NOTES.md`); "reintentar" is not drawn |
| protocol | checked, both sides | the numbers are the real ones |
| workspaces (home) | checked, dark | search built (name, branch, path); singular "workspace · 1"; legend glyphs in their colours. **Not done:** `+` (needs server messages `open{path}` and `worktree{...}`, "W4"), the row's second line is the branch and PR, not the agent's last message (the bridge does not send it), the current-row highlight (no notion of "current" on the wire), ⚑ merged (needs GitHub data), the diff tab (M6) |
| settings | checked, dark | quick answers as three presets with the preview line; themes as a **read-only** grid (the user chose: the app follows the system theme and nothing writes `settings.json`); connection rows show the host, the real protocol number and access, no latency (not measured), no PC name (`ready` does not send one); the prototype's animation and vibration toggles are not built (no feature behind them) |
| workspace | checked on a phone (Android 16): terminal, key row, composer, leader sheet, split | selection, fit-to-phone and a y/n on a real agent prompt not tried yet |

| wifi and tunnel routes | checked, dark | status bar `wifi`/`túnel`, both routes in settings with the one in use marked, the wifi row in "no encuentro tu PC" (only when tried), "PC alcanzado · por wifi". The prototype's "sin respuesta" on the wifi row of settings is drawn only when the connection is on the tunnel |
