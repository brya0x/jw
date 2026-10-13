# Mobile QA checklist (phase 1, for review)

Branch `feat/mobile-qa`, main at `10ba9f2` (app 0.0.26, PROTOCOL 25). Report only: no product code is changed.
Phase 2 starts when this file is approved. Results go to `QA-REPORT.md` (phase 3); evidence to `docs/qa/evidence/<case-id>/`.

Sources read for this list: `docs/specs/mobile.md` (REQ-1..103), `android/AGENTS.md`, `crates/daemon/src/chat.rs`,
`crates/daemon/src/lib.rs` (Working/Waiting, `esc to interrupt`), `crates/daemon/src/commands.rs`, and the app files
`HomeUi.kt` (workspaces), `ChatUi.kt`/`ChatState.kt`/`AskText.kt`/`AskUi.kt` (agent), `ShellUi.kt`/`TerminalUi.kt` (shell),
`WorkspaceUi.kt` (which view a pane gets). Not read: `mobile-prototype.html` (UI look is only compared by eye in phase 2).

## 0. Set-up notes

**Driving the phone** (adb only; maestro and scrcpy are not installed):
- `adb -s <serial> exec-out screencap -p > evidence/<id>/NN.png`; `adb shell uiautomator dump /sdcard/w.xml && adb pull` for text and bounds.
- `adb shell input tap|swipe|text|keyevent`. Compose text fields: `input text` with `%s` for spaces; paste via `adb shell input keyevent 279`.
- Network loss: `adb shell svc wifi disable|enable` and `svc data disable|enable`. Theme: `adb shell cmd uimode night yes|no`. Rotation: `settings put system accelerometer_rotation 1` plus `user_rotation`.
- Phone shown by adb now: serial `94141b9e` (task says model CPH2417, 1080x2412; confirm with `getprop ro.product.model`).
- Build/install: `cd android && ./gradlew :app:installDebug`. Logs: `adb logcat -s` filtered to the app, checked for tokens/terminal bytes (REQ-27).

**Isolating the daemon**:
- Only `scripts/jw-test` (state in `/tmp/jwt`, own socket and `jw.pid`); `fresh` wipes it, `mobile` opens the pairing window, `stop` kills only the pid in `/tmp/jwt/run/jw/jw.pid`.
- Every `jw` call in phase 2 runs with the test env (`JW_*`/`XDG_RUNTIME_DIR` as the script sets them) so `jw ls`, `jw read`, `scripts/drive.py` hit the test daemon, never the real one. First command of phase 2 is to print which socket `jw ls` uses and compare it with the user's real one.
- Never `pkill`; kill only a pid I started and recorded. Wifi listener: set `JWT_LAN=off` for tunnel-only cases, leave it for wifi cases (macOS may show a firewall question).
- Pairing needs the user to scan the QR (they agreed). I will ask when the window is up, and tell them to re-pair with their real daemon when I stop mine. Pairing the phone with the test daemon replaces the phone's stored device, so the real one must be re-paired afterwards.
- Real Claude panes (cost): cheapest model (`--model haiku`), prompts of a few words, `/clear` between cases.

**Compare tools**: `jw ls --json` (state, marks, branch, PR), `jw read` (pane text), transcript JSONL at `~/.claude/projects/<folder>/<id>.jsonl`, TUI via `scripts/drive.py`.
Haiku subagents do screenshots, dumps and the table diffs; they return paths and short tables.

**Skills found** (find-skills; nothing installed, I will ask first):
- `vercel-labs/agent-browser@agent-browser` (1M installs): browser QA, web only; not useful for the Android app.
- `software-mansion/argent@argent-test-ui-flow` (23.4K): UI flow testing for React Native/mobile; may give a pattern for step/expect scripts; unverified for Android native.
- `raphaelbarbosaqwerty/maestro-dev-skills@maestro-e2e` (528): Maestro flows; needs the maestro CLI, which is not installed.
- `sickn33/agentic-awesome-skills@android_ui_verification` (146): adb screenshot/uiautomator verification recipe; closest to my approach, low reputation.
- `skydoves/android-testing-skills@capturing-screenshots-and-screenrecord` (61) and `@extracting-logs-with-logcat` (68): small adb helpers.
- `jeffallan/claude-skills@test-master` (5K) and `affaan-m/ecc@browser-qa` (9.1K): generic QA process; browser-oriented.
- Already installed and used where it fits: `android-review` (for reading app code during triage, not for QA runs).
My view: none is needed; the adb recipe above is enough. Say if you want one installed.

## 1. Workspaces view sync

Phone: Home screen (`HomeUi.kt`, list of roots with nested worktrees, folders section, marks, sessions sheet, Avisos tab).
Compare: `jw ls` + TUI sidebar. For each case record "time until the phone shows it" (poll `uiautomator dump` every 0.5 s) and the end state.
Spec says (REQ-12, 18) the phone asks `workspaces`/`sessions` and `Panes` is polled (REQ-20: good for 6 s, polls every 2 s). Expect a lag of up to a few seconds; anything over ~6 s or a state that never arrives is a fail.

| id | Steps | Expected | PC comparison | REQ |
|---|---|---|---|---|
| W-01 | Fresh test daemon, pair, open Home | Roots, worktrees nested, folders in own section; same names and order as TUI | `jw ls`, TUI sidebar | 12, 18 |
| W-02 | On PC create a worktree (`jw worktree` or `^␣ w`) | New row nested under its root, within poll time | `jw ls` | 12, 18, 87 |
| W-03 | Create a worktree from the phone (`+`, `^␣ w`) | Pending state shown, then row; name-taken and branch-exists refusals shown in the sheet; survives reconnect (kill wifi during create) | `jw ls`, git worktree list | 87, 88 |
| W-04 | Rename a worktree/workspace on PC | Row shows new name; if the phone cannot see renames, record it | `jw ls` | 18 |
| W-05 | Close a pane / close a workspace on PC | Row marks and pane list update; no ghost row | `jw ls` | 18, 19 |
| W-06 | Delete a worktree on PC (remove) | Row disappears; open workspace screen of it on phone returns gracefully (no crash, no stale panes) | `jw ls`, `git worktree list` | 18, 23 |
| W-07 | Try remove/rename/sync/review from phone `^␣` | Shown greyed, tagged "en el PC" | n/a | 23 |
| W-08 | Add/remove a folder workspace (PC, then phone `open`) | Folder section follows both ways; second open of the same path changes nothing | `jw ls` | 85 |
| W-09 | Create a session from phone; switch session on PC; switch on phone | Sessions sheet lists both; list switches; names refused when taken/reserved/invalid | `jw sessions` | 86, 88 |
| W-10 | Agent in a pane goes working → idle | Mark `✻` appears and clears; time to follow | `jw ls` marks | 18 |
| W-11 | Agent goes waiting (permission prompt) | Mark `?`, row y/n only for a plain permission; Avisos lists it | `jw ls` | 18, 20, 28 |
| W-12 | Approve on PC while phone shows it waiting | y/n gone within 6 s, no double send | `jw ls` | 20 |
| W-13 | Tap y on phone | Daemon ends Waiting; row goes working; stays answered until `Panes` says not waiting | `jw ls` | 20 |
| W-14 | Change branch in a worktree (`git switch`) | Branch label follows | `jw ls` branch | 18 |
| W-15 | Counters (panes per workspace, changes) | Match PC | `jw ls` | 18 |
| W-16 | PR/CI state: worktree with an open PR and a failing/passing check (needs GitHub; skip and say so if the test state has none) | Phone shows what `jw ls` shows; spec has no REQ for CI, so any gap is a finding about missing coverage | `jw ls`, TUI | none (gap) |
| W-17 | Orphans: worktree whose folder was deleted on disk, or pane with dead process | Matches how `jw ls` lists them; no crash | `jw ls` | 18 |
| W-18 | 20+ workspaces, search in the finder | Filter works, scroll smooth, touch targets 48 dp | `jw ls` | 18, 26, 71 |
| W-19 | Phone idle 5 min with PC changing | List correct on wake, no drift | `jw ls` | 15 |
| W-20 | Read-only on (`jw mobile` → `r`) | List still answers; open/worktree/session refused with a message | n/a | 7, 82 |

## 2. Agent view (chat)

Phone: `ChatView` (`ChatUi.kt`), shown for a pane with role `agent` or `claude` in the foreground once `ChatWatch` answers `known:true` (REQ-94). Otherwise the text surface (REQ-21).
Block kinds the daemon builds from the transcript (`chat.rs`): `user`, `text`, `think` (duration only, no thought text), `tool` (+ result/body/error), `queued`, `command`, `commandout`, `notice`; plus `Chat.working`, `Chat.turn`, `Chat.bg`, `Chat.plan`, `name`, `title`. Dropped: `isMeta`, `isSidechain` (subagent) entries, unknown types.
Phone-side from the **screen** (`AskText.from` over `Text` rows): the question and options of a selection prompt, the input box text, the `esc to interrupt` working signal (daemon-side), the Waiting state itself comes from Claude hooks (not for a Claude started by hand in a shell).

### 2.1 Cases

| id | Steps | Expected | Compare with PC | REQ |
|---|---|---|---|---|
| A-01 | Open an agent pane that has no session yet | Text surface, not chat (`known:false`) | `jw read` | 94 |
| A-02 | Send "say hi" from the phone composer | User bubble right, Claude text left, header shows name then `/rename` name | transcript JSONL | 93, 94 |
| A-03 | Typing sync: type on phone, then on PC | One text; PC change replaces phone's after 800 ms still; Enter sends | TUI input box | 96 |
| A-04 | Start `claude` by hand in a plain shell pane | Becomes chat once it has a transcript; back to shell cards on exit | transcript | 93, 94, 96 |
| A-05 | Long answer (~100 lines), scroll up, new block arrives | View stays where the person scrolled; at bottom it follows | transcript | 94 |
| A-06 | Select and copy text from a Claude text | Android selection works | clipboard | 94 |
| A-07 | `**bold**`, `` `code` ``, fenced code, lists, tables, links in a reply | Matrix row M-02 | transcript | 94 |
| A-08 | `/` in the input | Slash list over input, filter order, tap puts `/name `; empty match sends as is; project/user commands and skills listed | `ls .claude/commands` | 101, 102 |
| A-09 | Run `/model`, `/cost`, `/help`, `/clear`, `/compact`, `/status` | `command` bubble + `commandout`; menus drawn only on screen: record | TUI | 93, 94 |
| A-10 | Switch pane away and back; kill wifi and return | Re-asks ChatWatch/Commands, same blocks, no duplicates (`i` numbering) | transcript | 15, 94, 102 |
| A-11 | Stop with `■ parar` while working | Esc sent, row gone | TUI | 99 |
| A-12 | Send a message while Claude works | `queued` dashed bubble becomes normal in place | transcript `queue-operation` | 100 |
| A-13 | Two Claudes in one workspace (split) | Each pane its own chat, tabs pulse for working | `jw ls` | 99 |
| A-14 | Chat of 400+ blocks | Only last 150 loaded then 400 kept; scroll-back limit is understood and recorded | transcript | 93 |

### 2.2 Matrix: what Claude can show vs the phone

Cells "same / different / not shown" are filled in phase 2; the "source" column is what the code says today, to be confirmed or refuted. "T" = transcript carries it, "S" = only on the PC screen.

| # | Claude shows | How to provoke (cheap, haiku) | Source today | Expectation from code/spec | Same | Diff | Not shown |
|---|---|---|---|---|---|---|---|
| M-01 | Assistant text | "say hi in 3 words" | T `text` | same, markdown subset | | | |
| M-02 | Markdown: headings, lists, tables, code fences, links | "list 3 fruits as a table, then a js code fence" | T | only `code` and **bold** are formatted per REQ-94; others likely shown raw | | | |
| M-03 | Thinking | enable thinking (`/model`, or "think hard about 17*23") | T `think` | one dim line with duration, no text (Claude's TUI also hides it by default) | | | |
| M-04 | Tool call + result: Read, Bash, Grep, Glob | "read README.md first line", "run ls" | T `tool` | card: name, one-line arg, result; opens to body | | | |
| M-05 | Edit/Write diff | "create a.txt with hi", then "change hi to yo" | T `tool` body | diff `+` green `-` red inside card | | | |
| M-06 | Tool error | "read /nonexistent" | T `tool.error` | red card | | | |
| M-07 | Permission prompt (Bash/Edit not allowed) | run without bypass; "run `date`" | Waiting from the `Notification` hook (`core/stream.rs:458`), question text from the screen (`AskText`); the daemon has no choice-question detection, only the plan (`ExitPlanMode`) from the transcript | full-screen ask (REQ-98); y/n only for plain permission; reconcile with card "espera permiso" | | | |
| M-08 | Permission with 3 options (always allow...) | same | S | AskText numbered options | | | |
| M-09 | Single-choice question (`AskUserQuestion`) | "ask me with AskUserQuestion: red or blue" | S + T tool | one row per option, tap sends number | | | |
| M-10 | Multi-choice question | "ask me multiselect: which of a,b,c" | S | check boxes, `Enviar` | | | |
| M-11 | Question with several tabs/questions | "ask me two questions" | S | likely not handled; record | | | |
| M-12 | Plan mode plan | Shift-Tab to plan mode, "plan adding a file" | S + plan file | plan text + 3 decisions, `Chat.plan` | | | |
| M-13 | Slash commands and their output | see A-09 | T `command`/`commandout`; menus S | picker menus (`/model`, `/config`, `/resume`, `/permissions`) draw only on screen | | | |
| M-14 | Custom `/commands` and skills | create `.claude/commands/x.md` | T + disk | listed, run shows as command block | | | |
| M-15 | Background task (`run_in_background`) | "run `sleep 60` in background" | T | ring row "N tareas en segundo plano", notice when done | | | |
| M-16 | Subagent (Task tool) | "use a subagent to count files" | T (sidechain dropped) | only the Task tool card and the end notice; subagent's own steps not shown | | | |
| M-17 | Working/idle | any prompt | `esc to interrupt` S, `Chat.working` | `Pensando…`/`Escribiendo…` + seconds + tokens | | | |
| M-18 | Errors: API error, rate limit, overloaded, interrupted ("[Request interrupted]") | Esc mid-turn; invalid model | T text or S only | check which land in transcript | | | |
| M-19 | Long output | "print numbers 1..500" via Bash | T (cut at 4 KiB / 16 KiB) | truncation visible? record whether the person is told | | | |
| M-20 | Image in a message / image output | paste an image path; Read a png | T `image` part | transcript part type unknown to daemon → likely skipped | | | |
| M-21 | Status line / footer (model, context %, cost, mode "accept edits on", `? for shortcuts`) | default | S only | not shown | | | |
| M-22 | Todo list (TodoWrite) | "make a 3-item todo then do it" | T tool | card with raw summary? record | | | |
| M-23 | Compaction / `/compact` summary, context warnings | `/compact` | T + S | record | | | |
| M-24 | MCP tool calls, WebFetch/WebSearch | if available | T tool | generic card | | | |
| M-25 | Hooks output, `!` shell lines, `#` memory | `!ls` | T `command` | `!ls` block + commandout | | | |
| M-26 | `/resume`, `/rewind`, `/login`, `/mcp` menus, `Ctrl-O` transcript view, `Ctrl-R` | open each | S only | not shown; phone should fall back to text (see M-27) | | | |
| M-27 | Claude screen with no ask detectable (REQ-98 "prompt that cannot be read") | any unknown menu | S | pane's text with keys; toggling to terminal view is "not offered" for chat panes: confirm the person is not trapped | | | |
| M-28 | Claude Waiting but hook did not fire (hand-started Claude) | start `claude` by hand, trigger permission | S only | `agent` empty; REQ-98 claims the screen alone opens it; confirm | | | |
| M-29 | Resumed/old conversation | `claude --resume` | T | last 150 blocks | | | |
| M-30 | Two sessions in same folder, new `/clear` | `/clear` | T new file | `Chat{reset:true}` and phone switches | | | |

**What the matrix suggests (to confirm after the run):**
- Everything the transcript carries (text, tools, results, diffs, queue, task notices, titles, plan file path) is already built by `chat.rs` and the rows that go wrong there are daemon/app rendering bugs, not data gaps.
- The rows that need the PTY screen are the prompts and menus (M-07..M-13 menus, M-21, M-26): the app reads them from the screen with `AskText`, which is a scrape and breaks when Claude's layout changes. Working state also uses a screen string (`esc to interrupt`).
- `stream-json` (Claude run headless by the daemon, `--output-format stream-json --input-format stream-json`) would give structured events for text, tool calls, permission requests (`control_request`) and AskUserQuestion, with no scraping. But jw runs the interactive TUI in a PTY that the PC user sees and types in; `stream-json` replaces the TUI instead of supplementing it, so it is a different product choice (a headless agent pane for the phone vs. the one the PC shows). Decision waits for the filled matrix: if the "not shown" rows are mostly screen-only menus and status line, stream-json does not close them either (they do not exist in that mode). Final answer in `QA-REPORT.md`.

## 3. Shell flow

Phone: `ShellView` (`ShellUi.kt`) for a zsh pane jw marked (REQ-95); `TerminalView` text surface for any other pane; full-screen programs take the pane (`full`). Compare: `jw read` of the pane, and the same command in the PC pane.

| id | Steps | Expected | REQ |
|---|---|---|---|
| S-01 | Open a new shell pane (`^␣` new pane) | Blocks view, folder and branch chips, empty editor | 95, 97 |
| S-02 | Type `echo hi` on phone, Enter | Block: command bold, output `hi`, duration, status; PC pane shows the same | 96, 97 |
| S-03 | Type on the PC, watch the phone editor | Replaced after 800 ms still | 96 |
| S-04 | Edit mid-line on phone (move cursor, delete) | Ctrl-U + whole text sent; PC line matches | 96 |
| S-05 | History: arrow up / down keys on key row | Previous command appears in editor and PC line | 22, 96 |
| S-06 | `ls` in a directory with 30 names | Columns read down, set again for phone width, same order as `ls` | 97 |
| S-07 | `ls -l`, `ls --color=always` / `ls -G` | Colours kept, no wrapping garbage | 97 |
| S-08 | Long output (`seq 1 5000`) | Pieces of 60 rows, `N líneas` header sticks, `↓ ir al final` works, memory ok | 97 |
| S-09 | Output 600 KiB+ | Start dropped and "truncated" shown | 95 |
| S-10 | Coloured output (`printf '\e[31mred\e[0m'`, `git log --color`, `ls`/`bat`) | Colours, bold, 256 and RGB | 97 |
| S-11 | Failing command (`false`, `ls /nope`) | Red bar and tint, `salió 1` | 97 |
| S-12 | Running command (`sleep 30`) | Yellow bar, `corriendo N s`, chip is the command, red stop button | 97 |
| S-13 | Ctrl-C key and red stop on `sleep 30` / `yes` | Stops, block ends, prompt returns | 22, 97 |
| S-14 | Ctrl-D, Ctrl-R, tab completion, `~`, `|` keys | Reach the shell | 22 |
| S-15 | Interactive program: `vim`, `htop`, `less`, `man ls`, `top` | `full` block takes the pane with notice + esc/tab/arrows/Ctrl-C; blocks return on exit; text drawn within phone width | 21, 97 |
| S-16 | Program that asks input (`read x`, `python3`, `ssh` password prompt) | Line goes to the program; stop button red | 97 |
| S-17 | Paste: long single line, multi-line, with tabs, 5 KB | Arrives intact; multi-line behaviour recorded (runs each line? bracketed paste?) | 96 |
| S-18 | Keyboard: show/hide, tap on pane closes it, scroll does not; keyboard over composer; key row repeat on arrows/backspace | As spec | 22, 96 |
| S-19 | Block tap actions: copy command, copy output, run again, send to app | Work; clipboard content correct | 97 |
| S-20 | Non-zsh shell (bash/fish) or `JW_SHELL_BLOCKS=0` | Text surface with keys | 95, 97 |
| S-21 | `cd` to another folder, new branch | Chips update | 97 |
| S-22 | Command run when phone reconnects: start `sleep 20`, drop wifi 10 s, restore | Block continues, no duplicates, output complete | 15, 95 |
| S-23 | Resize: phone never resizes; PC wider than phone | Wrapping rules of REQ-21 (long words cut, runs of spaces shrink, box lines cut) | 21, 24 |
| S-24 | Text selection in the terminal surface | Copy to clipboard works | 22 |
| S-25 | Unicode: emoji, CJK, combining, box drawing | No misalignment | 21 |

## 4. Transversal (already on main)

| id | Steps | Expected | REQ |
|---|---|---|---|
| T-01 | Wifi off then on (and airplane mode 30 s) while on a chat and on a shell | Reconnect with backoff, re-attach, new Snapshot is truth, route chip changes `wifi`/`túnel` | 15, 63, 64 |
| T-02 | Route: pair over wifi, then leave the network (data only) | Falls to tunnel (needs cloudflared; skip with note if missing); returns to wifi within 30 s | 63 |
| T-03 | Stop daemon (test pid) while connected, start again | Error `daemon`/reconnect; protocol mismatch screen only if versions differ | 11, 16 |
| T-04 | Theme: Settings → light/dark/system; `cmd uimode night` | Palette from `ready`, pane colours follow, PC TUI theme unchanged | 17 |
| T-05 | Diff tab: Uncommitted, Commits, General; tree folding, viewed marks, big diff (> DIFF_CAP), no base | As spec | 25, 72–75, 89–91 |
| T-06 | Files screen and viewer: `..`, hidden switch, Markdown view/source, binary, big file, symlink | As spec | 77–84 |
| T-07 | CI status: only if surfaced anywhere (W-16); else note "no REQ" | n/a | none |
| T-08 | Delete worktree: PC-side removal while phone has it open (W-06); phone remove greyed | No crash | 23 |
| T-09 | Portrait lock: rotate with auto-rotate on, upside down, keyboard open | Stays portrait | 103 |
| T-10 | Settings: quick-answer presets bytes (`y`, `n`, `y + Enter`) | Preview matches what is sent to the pane | 55 |
| T-11 | Mode chip, status bar, 48 dp targets (dump bounds) | Chip NORMAL/TERM/DIFF/SET/?; all clickables ≥ 48 dp | 26 |
| T-12 | Logs: `adb logcat` during a session | No tokens, no terminal bytes, no file contents | 27 |
| T-13 | Revoke the device on the PC (`jw mobile revoke` on the test daemon) | Phone closed within ~2 s plus a tick; shows a final screen, no retry loop | 6 |
| T-14 | Read-only toggle while connected | Applies to new messages; phone's UI is not told (spec) — record what it does | 37 |
| T-15 | Pair denied / code expired / too many attempts / no connection screens | One failure screen each | 51–54 |

Out of scope for QA (not exercisable on a phone, or covered by `cargo test`): REQ-1..11, 29..50, 56..62, 65..70 (daemon/bridge/TUI internals) except as touched above.

## 5. Order and cost of phase 2

1. Setup, pair, W-01..W-20 (cheap, no Claude tokens except W-10..13).
2. S-01..S-25 (no tokens).
3. A-01..A-14 and matrix M-01..M-30 (haiku, short prompts; about 40 prompts in total).
4. T-01..T-15.
Evidence per case: `NN.png`, `NN.xml`, and a `pc.txt` with the PC-side output. Result values: pass / fail / not-shown.

## 6. Questions for you

- OK to run the phone against the test daemon only, and to ask you to re-pair with the real one at the end?
- Do you want any of the skills above installed (my view: no)?
- Is a GitHub remote with PR/CI available in the test state, or do I mark W-16 as untestable?
- Claude panes: is `--model haiku` fine, and does the test environment have a logged-in `claude`?
