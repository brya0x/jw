---
name: jw-mobile-qa
description: Use when running the jw Android app against a jw test daemon and checking that what the phone shows is what the PC (jw ls, jw read, the TUI, the Claude transcript) says. Drives the phone with adb, keeps evidence per case, and records pass / fail / not-shown. Report only; it never fixes product code.
---

# jw mobile QA

What is under test: **the phone says what the PC says, fast and in the same words.** The cases are in
`docs/qa/CHECKLIST.md` (ids W-, A-, M-, S-, T-). Results go to `docs/qa/QA-REPORT.md`, evidence to
`docs/qa/evidence/<id>/`.

Built from a read of `android_ui_verification`, `android-ui-journey-testing` and `argent-test-ui-flow`
(not installed). Kept from them: calibrate first, find nodes in the `uiautomator` dump and tap their centre, wait on
the UI instead of sleeping, one verdict per step with the evidence that backs it. Changed: a failed case does not stop
the run (cases are independent), and every check has a PC side.

## Rules

- **Isolated daemon only.** `scripts/jw-test` (state `/tmp/jwt`). Before the first case print the socket `jw ls`
  uses and confirm it is the test one. Never `pkill`; kill only the pid in `/tmp/jwt/run/jw/jw.pid`. Never write
  in the user's real panes.
- **Real-daemon mode (user decision, 2026-10-13):** agent and feature cases run in a throwaway session of the user's real jw
  (`qa-test`), because a login for the test HOME was refused. Only that session: every `jw` call sets `JW_SESSION=qa-test`
  explicitly, the phone driver runs with `QA_SESSION=qa-test` and `QA_ALLOW=<regex of the QA workspace names>` so `q.py` refuses
  to tap, type or swipe on any other session's screen, and nothing is ever typed into a pane that is not a QA one.
  Never `jw server stop`, never `pkill`, never remove a worktree that is not a QA one.
- **Real Claude panes cost tokens:** `--model haiku`, prompts of a few words, `/clear` between cases.
- **No secrets in evidence:** no pairing token, no device token, no `adb logcat` lines carrying them (REQ-27).
- Report only. A bug found is a row in the report, never a patch.

## Setup (once)

```sh
export ANDROID_SERIAL=94141b9e                  # adb devices
adb shell wm size; adb shell getprop ro.product.model
Q=.claude/skills/jw-mobile-qa/q.py
(cd android && ./gradlew :app:installDebug)
```
Pairing needs the user to scan the QR from `scripts/jw-test mobile`. Ask, wait, then `python3 -I $Q wait "<home text>"`.

## One case

1. `mkdir -p docs/qa/evidence/<id>`; note the start time.
2. Do the step on the PC or the phone (`q.py tap|type|key|swipe`, or `jw ...` against the test daemon).
3. Wait on the phone, never sleep blind: `q.py wait "<text that must appear>" 10` prints the ms it took;
   `q.py gone "<text>"` for things that must leave. The ms is the sync latency: record it.
4. Capture both sides at the same moment:
   - phone: `q.py shot docs/qa/evidence/<id>/NN.png` and `q.py dump .../NN.xml` (`q.py texts` for a quick look)
   - PC: `jw ls --json`, `jw read <pane>`, the transcript JSONL tail, or `scripts/drive.py` for the TUI, saved as `pc.txt`
5. Compare: same content? same words? same order? Record one verdict: **pass** (same), **diff** (shown, but
   differently: say how), **not-shown** (PC has it, phone does not), **fail** (wrong or crash).
6. Append one line to `docs/qa/evidence/results.tsv`: `id  verdict  latency_ms  evidence_dir  note`.

Compose nodes sometimes carry no text. If `texts` is empty for something the screenshot shows, read the
screenshot and say so in the note (it is an accessibility finding too).

## Haiku subagents

Cheap and repetitive: screenshots and dumps for a list of ids, `jw read` / transcript reads, and the phone-vs-PC
diff table. They write files and return the path and a table of at most 20 lines. Judging a verdict and writing the
report stays with the orchestrator.

## Hazards

- `input text` drops most non-ASCII and some symbols; for those use the app's own paste or a key row.
- `uiautomator dump` fails while a view animates: retry once before calling it a bug.
- Dumps freeze long lists to what is on screen: scroll, then dump again.
- Wifi cases need the phone on the PC's LAN; `svc wifi disable` kills adb over wifi if adb is wireless.
- Re-pair: pairing with the test daemon replaces the phone's stored device. Tell the user at the end to re-pair with
  their real daemon.

## Iterating on this skill

When a step turned out clumsy or a hazard bit twice, edit this file or `q.py` in the same commit as the findings and
say what changed in the report's last section.
