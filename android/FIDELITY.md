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
| workspaces, workspace, avisos, settings | **not compared yet** | search, "1 workspace", legend, current row, quick answers still to do |
