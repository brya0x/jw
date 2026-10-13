# Mobile QA report

App 0.0.26 (PROTOCOL 25), branch `feat/mobile-qa` at main `10ba9f2`, phone CPH2417 (adb `94141b9e`, 1080x2412).
Every row's evidence is under `docs/qa/evidence/<id>/`; per-case verdicts are in `docs/qa/evidence/results.tsv`.

## How it was run, and what it covers

- **Workspaces and shell cases** ran against an isolated test daemon (`scripts/jw-test`, state `/tmp/jwt-qa`), killed by its own pid at the end.
- **Agent cases** ran in a throwaway session `qa-test` of the user's real jw, by the user's decision, because Claude was not logged in under the test HOME and linking the keychain was refused. Only that session was touched; the phone driver (`q.py`) refused to tap or type outside it.
- **91 of 106 checklist cases have a verdict** (pass, diff, fail, not-shown or inconclusive); 15 are `not-run` in `results.tsv`:
  - the owner excluded two groups: pairing/revoke/read-only/route cases (W-20, T-02, T-03, T-13, T-14, T-15) and anything that needs adb to paste, select or type on the PC (S-03, S-04, S-17, S-24, A-03);
  - not built: A-14 (a chat of 400+ blocks, costly), M-24 (MCP / WebFetch), M-28 (permission prompt in a Claude with no hooks);
  - inconclusive after trying: A-06 (text selection: adb long-press did not raise the toolbar; the code wraps the chat in a `SelectionContainer`), A-12 (queued message), M-25 (`!ls`), M-27 (`/resume` list on the PC side).
- Latencies are coarse (see "Workspaces sync"): the driver adds seconds, so they bound the sync time from above and do not measure it.
- Judgement calls the reader should know: the theme result (B-06) depends on the PC theme pair, which I did not inspect; B-12 (stray "-") may come from the keyboard.

## Scope decisions (owner, 2026-10-13)

- **This QA writes no code and starts no fixes.** Everything under "Suggested changes" is a list for a later fix worktree, to be started only when the owner asks; no product code, spec or version was changed here.
- **B-19:** remove the y/n buttons entirely (Home row and Avisos, plain permissions included) and show the tool or question that is waiting, with a button that opens the full-screen question.

## Bugs, by severity

| id | sev | title | steps | expected | actual | evidence | REQ |
|---|---|---|---|---|---|---|---|
| B-19 | high | "y · aprobar" / "n · rechazar" buttons approve a plan or answer a question the person has not seen | a Claude pane waits on a plan (or a choice); look at the Home row and at Avisos | no y/n: REQ-20 allows them only for a plain yes/no permission, REQ-98 says "no approve or reject buttons" for plans and choices | the Home row shows live green "y · aprobar" and red "n · rechazar" under the waiting workspace, and Avisos shows y / n (dim) with the pane's name and no content. The default "Claude Code" quick-answer preset makes **y** send Enter, which accepts whatever option the cursor is on. Tapping **y** on a pending plan accepted option 1, "Yes, and use auto mode": the plan ran and Claude was switched into auto mode, all without the plan ever being on the phone | AV-01/, AV-02/ | 20, 28, 98 |
| B-01 | high | App crashes when the system day/night mode changes inside a workspace | open any shell or agent workspace; `adb shell cmd uimode night yes` | theme change, no crash | `IllegalArgumentException: Mismatching scroller orientation; single-line, non-wrap text fields can only scroll horizontally` at `WorkspaceUi.kt:438` (Composer `BasicTextField`); the app closes. Reproduced 3 times (shell and agent); the home list survives. Auto day/night switching would trigger it in normal use | T-04/crash.txt | 17 |
| B-02 | high | First-run Claude prompt is invisible and answered blind | new agent worktree whose folder Claude does not trust yet; open it on the phone | the "trust this folder" question and its options shown | agent chat reads "Sin mensajes todavía."; only the key row works. A blind Up, Down, Enter chose "No, exit" and the agent pane exited. Not seen under the real daemon (folder already trusted) | M-08/, M-08b/ | 94, 98 |
| B-03 | medium | Screen-only Claude panels are not shown, and the pane stays inside them | send `/status` or `/config` from the phone | something shows the panel, or says Claude shows one | the chat stays as it was, no command bubble, no panel; the PC pane stays in the panel until Esc is sent from the key row | M-26/ | 94, 98 |
| B-04 | medium | Tapping an option in the `/model` picker changes the user's default model | `/model` on the phone, tap an option | a choice that cannot persist, or a warning | the picker is read off the screen and shown ("Toca una opción: se envía al instante"), but its footer says "Enter to set as default"; the `s: this session only` key and the effort slider are unreachable | M-13/ | 98 |
| B-05 | medium | `ls` columns are re-laid-out wrongly | shell block: `ls` in a folder of 32 names (PC: 6 columns) | names in clean columns for the phone width, same order as `ls` | rows of two names with an orphan third, irregular order | S-06/01.png | 97 |
| B-06 | medium | Full-screen program in a tall pane shows nothing useful | `less file` when the pane is 206x112 | the content | the view shows the bottom 24 rows: only `~` and `(END)`; the text is off screen at the top; no full-screen notice and the normal key row | S-15/ | 21, 97 |
| B-07 | medium | "Seguir al sistema" never goes dark; status bar becomes unreadable | system night mode on, Settings has "seguir al sistema" | dark palette | stays light, white status icons on a light bar; choosing catppuccin-mocha explicitly works. (Unchecked whether the test PC's theme pair has a dark member) | T-04/night-home.png, night-relaunch.png | 17 |
| B-08 | medium | Reconnect drops the person on the home list | in a shell workspace turn wifi and data off 12 s, then on | back in the same pane | reconnects 18 s after the network is back, on the Home list; while offline the screen offers "olvidar este PC" next to the retry | S-22/ | 15 |
| B-09 | low | Permission and question prompts lose context | Write permission in manual mode; AskUserQuestion with a heading; multi-select | what the PC shows | the file preview ("1 hi") is not shown; option 2's text is cut ("…common file"); the heading "☐ Color" is dropped; after a multi-select Claude's review ("→ a, c") is not shown, only "Ready to submit?" | M-07/, M-09/, M-10/ | 98 |
| B-10 | low | Phone cannot leave auto/plan mode | `/plan` from the phone | a way back | the key row has `tab` but no Shift+Tab; the pane stays in plan mode (also seen "auto mode on" with no way to change it) | M-12/ | 22 |
| B-11 | low | Markdown is mostly raw | ask for a table, a heading and a fenced `js` block | formatted | table as raw pipes, "## Fruits" raw, the fence language printed as its own line; REQ-94 promises only code and bold | M-02/ | 94 |
| B-12 | low | Keyboard hides the primary button; chat area collapses | new worktree form: type a name; chat/shell with the keyboard open | button reachable | "crear y abrir" sits under the keyboard (needs Back first); with the keyboard open the output area is ~3 lines. A trailing "-" appeared in the typed name ("phone-wt-"), keyboard suspected | W-03/03.png | 88, 96 |
| B-13 | low | Enter key leaves the text in the editor | type `echo hi`, press Enter (adb `keyevent 66`) | editor cleared | command runs, editor keeps "echo hi⏎"; the send button clears it. (Soft-keyboard send action not tried) | S-02/ | 96 |
| B-14 | low | Output text is not in the accessibility tree | `uiautomator dump` of a shell block | text nodes for output | only the command and header are nodes; output is drawn without semantics (TalkBack and UI tests cannot read it) | S-02/ | 26 |
| B-15 | low | Empty list reads as loading | PC session with 0 workspaces | "no workspaces" | "workspaces · 0" and "Esperando la lista del PC…" | W-01/ | 12, 18 |
| B-16 | low | A fresh agent at its idle prompt is "waiting ?" | open a new agent worktree | idle | PC (`jw ls`) and phone both say waiting (Notification hook); consistent, but the mark means "needs you" and nothing is asked | W-11/ | 18, 20 |
| B-17 | low | Phone and PC name workspaces differently | worktree `qa-wt1` of project `qa-origin` | same name | PC `qa-origin/qa-wt1`, phone `qa-wt1` under root `demo` (folder name, not the project name) | W-02/ | 18 |
| B-18 | low | Box-drawing characters are clipped | `printf '┌──┐'` in a shell block | full glyphs | drawn as a bracket under the baseline | S-10/01.png | 97 |
| B-20 | medium | PR merged is never shown on the phone | a worktree whose PR is merged: the TUI draws `⚑` | the same mark, as REQ-18 lists `✻ ? ⚡ ⚑` | the app's `Mark` is only Working, Waiting, Dev (`proto/Workspaces.kt:58`) and the code says the bridge "cannot know" the merged-PR flag; the phone legend has no ⚑. Found by reading the code, not run (no GitHub in the test state) | android/proto/.../Workspaces.kt:58,136 | 18 |
| B-21 | low | "Finished" is not a state the phone shows or announces | an agent ends its turn | something that says it finished (and the PC's `rang` bell mark, if that is the signal) | the working mark and the "Pensando…" row simply disappear; no "terminó" mark, row text or alert (Avisos lists only waiting panes; its idle text "en reposo" appears only there). Background alerts when the app is closed were not tested | W-10, M-17 | 18, 28 |
| B-22 | high | A Claude started by hand shows "working" on the PC and nothing on the phone | in a `qa-test` workspace open a pane that runs `claude` by hand (no jw hooks; role `run`) and give it a prompt that takes ~10 s; watch `jw ls` and the phone's Home row | the same mark on both | `jw ls`: `demo working ✻` for the first ~4 s, then `waiting ?`; the phone's `demo` row carries no mark at all during the whole turn (no ✻, no ?). Reproduces what the owner saw ("one chat was working but no status in the app"). The phone derives marks only from the hooks' `PaneInfo.agent`; the PC also counts the pane's screen activity, and the phone only learns "working" from `Chat.working` once the chat view of that pane is open | ST-01/ | 18, 99 |
| B-23 | low | A very long output is cut with no notice | shell: `seq 1 150000` (~1 MB) | the block says it was cut (REQ-95 keeps 512 KiB and flags `truncated`) | the block header says "3023 líneas" and shows the end; the first ~147,000 lines are gone and nothing says so | S-09/ | 95, 97 |
| B-24 | low | Branch label does not follow `git switch` | in a worktree `git switch -c feat/qa-branch2`; watch `jw ls` and the Home row for 3+ minutes | the new branch | both `jw ls` and the phone keep the branch the worktree was created with. PC and phone agree, both stale | W-14/ | 18 |
| B-25 | low | `/compact` shows Claude's summary as the person's message and leaks ANSI | `/compact` in a chat | a notice or the summary as Claude's; clean output | the continuation summary ("This session is being continued…") is drawn as a right-hand user bubble; the output reads ".[2mCompacted (ctrl+o to see full summary).[22m" | M-23/ | 93, 94 |
| B-26 | low | App reopens on another session and resets its place after a reconnect | close the app and reopen it (it opened on the user's `sales-assist` session, not `qa-test`); drop the network inside a chat | the last session and pane | opening on a different session is a hazard when several sessions hold real work; after a reconnect the app is back in the workspace but on its first tab, not the pane it was in | A-10/ | 15 |
| B-27 | low | Image tool results show as "sin salida" | ask Claude to read a PNG | the image or a note that there is one | the card says "sin salida"; the image is not shown | M-20/ | 94 |
| B-28 | medium | An exited agent pane looks alive | restart jw (or let an agent exit); open the workspace | something saying the agent ended | the chat keeps the old conversation, the composer reads "escribe a agent", the tab has a red dot; typing "hello" and sending shows nothing and goes nowhere | B-exited/ | 94, 99 |
| B-29 | medium | After a jw restart, agent panes exit instead of resuming (PC side) | restart the daemon with agent workspaces open | the agents come back | the agent panes of `demo` and `qa-perm` were `exited 1`; the pane text says "Session ID … is already in use". Caveat: the project's start/resume commands were my wrapper script, which may have contributed | W-05/, B-nav/ | 15 |
| B-30 | medium | Two hand-started Claudes in one folder show the same conversation | two `claude` panes (role `run`) in one workspace, each with its own prompt | each tab its own chat, tabs told apart | both tabs show the newest transcript (the one for the second pane), and both tabs are labelled "run" | A-13/ | 93, 94 |
| B-31 | low | The `^␣` sheet has no greyed "en el PC" rows | open `^␣` in a workspace | rename, sync, remove, review listed greyed with "en el PC" | those four are not listed at all | W-07/ | 23 |

## Workspaces sync

- Follows, in order of confirmation: open folder, create worktree (PC and phone), rename (name and branch), remove (the phone was inside it and returned to the list without crashing), session list and a refused duplicate session, waiting (`?`) and working (`✻`) marks. Each matched `jw ls` in content.
- **Speed:** worktree create ≤ ~5 s end to end; rename ≤ ~11 s with ~5 s of that being my driver. The daemon's own polling is 2 s (`Panes`), so a change shows between 0 and ~6 s later. A clean measure needs a timestamp from the PC side; I did not build one. Treat these as upper bounds.
- Not covered: close pane, branch and counter changes, orphans, 20+ workspaces, read-only, folder add/remove by the PC.
- Gaps against the spec: there is no CI or PR state anywhere on the phone, and no REQ for it (removed from scope by the user).

## Agent view: matrix

"same" = phone shows what the PC shows; "diff" = shown differently; "not shown" = PC has it, phone does not. Source: T = transcript, S = PC screen.

| # | Claude shows | Source | same | diff | not shown | notes |
|---|---|---|---|---|---|---|
| M-01 | assistant text | T | x | | | |
| M-02 | markdown (table, heading, fence) | T | | x | | B-11 |
| M-03 | thinking | T | x | | | one dim line "pensó N s", text not available in the transcript either |
| M-04 | tool call and result | T | x | | | |
| M-05 | Write / Edit diff | T | x | | | card and `+1 −1` |
| M-06 | tool error | T | x | | | |
| M-07 | permission prompt | S + hook | | x | | works; loses the file preview, truncates option 2 (B-09) |
| M-08 | trust dialog (no transcript yet) | S | | | x | B-02 |
| M-09 | single-choice question | S | | x | | heading dropped |
| M-10 | multi-choice question | S | | x | | review step lost |
| M-12 | plan | S + plan file | x | | | complete and scrollable |
| M-13 | slash command output / picker | T for `command`, S for pickers | | x | | `/model` works, B-04 |
| M-15 | background task | T | x | | | row while it runs, notice when done |
| M-16 | subagent | T | | x | | only the notice; its own steps are dropped by design |
| M-17 | working / idle | S (`esc to interrupt`) + hook | x | | | |
| M-18 | interruption | T | x | | | |
| M-19 | long output | T | not tested for truncation | | | |
| M-21 | status / mode line | S | | | x | no way to see "manual / plan / auto mode" or context % |
| M-26 | `/status`, `/config`, other panels | S | | | x | B-03 |
| M-11 | two questions in one prompt | S | | x | | one after the other; the PC's tab bar and the final review are not shown |
| M-14 | custom `/command` | T + disk | x | | | listed as "comando", runs |
| M-20 | image (Read of a PNG) | T | | | x | card says "sin salida" (B-27) |
| M-22 | todo list | T | n/a | | | no TodoWrite in this Claude |
| M-23 | `/compact` | T | | x | | summary shown as the person's message, ANSI leaked (B-25) |
| M-25 | `!` shell escape | T | inconclusive | | | |
| not run | M-24, 27–30 | | | | | |

## States: what the phone shows for each

| state of the agent / workspace | PC shows | phone shows | tested |
|---|---|---|---|
| thinking / working | `✻`, `esc to interrupt` | `✻` on the row and tab; chat ends in "Pensando…" (then "Escribiendo…") with seconds and "■ parar"; tool cards with a ring | yes, matches (W-10, M-17) |
| tool running | spinner on the call | card with a turning ring and seconds | partly (Bash seen running) |
| question or plan waiting | `?` | `?` on the row and tab, full-screen question in the workspace, and the y/n buttons of B-19 on Home and Avisos | yes, with the B-19 problem |
| background task / subagent | task count | "N tareas en segundo plano" row, notice when done | yes |
| finished (turn done) | `✻` clears, idle | mark clears, live row disappears; nothing says "finished" | yes (B-21) |
| exited agent | pane exited | chat/pane left as it was | seen once (B-02) |
| PR merged | `⚑` | nothing (B-20) | by code only |
| dev server | `⚡` | `⚡` in the legend | not run |

## Shell findings

Pass: new shell block view, command and output, failed command (red, `salió 1`), running command (`corriendo N s`, red stop button), stop (Ctrl-C, `salió 130`), 3000-line output with `N líneas`, colours (red, bold, RGB, background), CJK and emoji.
Also pass: history with the arrow key, tab completion, a program that asks for input (including a silent password prompt), tapping the output closes the keyboard, block actions (copy command, copy output, repeat, share sheet), a nested bash treated as a running command, `cd` updating the folder chip.
Fail or diff: B-05, B-06, B-13, B-14, B-18, B-23. Not run: paste, selection, wide PC pane, `ls` colours.

## Transversal

Pass: portrait lock holds against a landscape request (REQ-103); diff tab matches `git status` (3 files, counts right); Files and Markdown viewer; sessions sheet.
Also pass: touch targets (no clickable under 48 dp on the main screens) and logcat hygiene on the fresh process. Also pass: quick-answer presets (T-10), 5-minute idle sync (W-19), 16-row list, scroll and search (W-18). Fail: B-01, B-07, B-08, B-26.

## Recommendation

**Fix first (in this order):**
0. **B-19 blind approve** — the Home-row and Avisos y/n buttons let a person approve or reject something they cannot see. Replace them with a single "abrir" that opens the full-screen question (the one that shows the plan, the options and the file preview), or show the request's data on the row itself before offering any button. Whatever is chosen, y must never be offered for plans and choices; today it is.
1. **B-01 crash** — a theme change must never close the app; one text-field setting at `WorkspaceUi.kt:438`. Add a UI test that flips `uiMode` inside a workspace.
2. **B-02 / B-03 / B-10** — the chat has one blind spot: anything Claude draws that is not a numbered list or a plan. Give the person a way to see the PC screen for the pane (a "ver pantalla" toggle onto the text surface, which the app already draws) and a Shift+Tab key. That closes B-02, B-03 and B-10 without any protocol change.
3. **B-08 reconnect** — return to the pane and drop "olvidar este PC" from the retry screen.
4. **B-05, B-06** — layout of `ls` columns and tall full-screen programs.
5. **B-07, B-04** — theme and the default-model side effect.

**Is `stream-json` the answer?** No, not by itself.
- Everything the transcript carries is already shown and mostly correct (text, tools, results, diffs, notices, background count, titles). The "diff" rows are rendering, not missing data.
- The "not shown" rows are things Claude draws only in its terminal UI: panels (`/status`, `/config`), the mode line, the context meter, the trust dialog and picker footers. `stream-json` is a headless mode: it would replace Claude's terminal UI, so those panels would not exist at all. It would help the prompts (permission requests and `AskUserQuestion` arrive as structured events, so the screen scraping in `AskText` and the "esc to interrupt" match could go), but only for a pane jw starts in that mode, and the PC would no longer see a Claude TUI in that pane. That is a product choice, not a bug fix.
- The cheaper path that covers more rows today is: keep the PTY, keep the transcript, add the screen as an escape hatch (item 2 above), and tighten the screen parsers (heading, preview, truncated options, footer keys).

**On the idea of one virtual-DOM-like protocol shared by the TUI and the phone (compact JSON diffs):**
- What QA found that it would fix: the phone rebuilds state after every reconnect and gets it wrong (B-08); two sources of truth for the same fact (hook state and `esc to interrupt`; transcript and screen); naming differences (B-17).
- **Evidence for it, found after the owner's report:** B-22 (the phone's Home row ignores a working Claude the PC marks as working), B-20 (merged-PR flag missing) and B-16 (waiting at an idle prompt) are one class: the phone re-derives each mark itself from `PaneInfo` (REQ-18) while the PC derives it from more inputs (hooks, screen activity, PR state). The smallest fix is not a widget tree: let the daemon compute the marks once (working, waiting and what for, finished, merged, dev) and send them in the `workspaces`/`Panes` answer, and let the phone draw them. That gives the "phone reflects what the PC shows" guarantee for the list without replicating the TUI.
- What it would not fix: the "not shown" rows are about what Claude exposes, not how jw ships it; a tree of the TUI's widgets would not contain Claude's `/status` panel either, because that is inside a pane's terminal.
- What it would cost: the phone's look (bubbles, blocks, full-screen questions) is not the TUI's look, so a widget tree would need a second renderer on the phone or a semantic layer on top, and every TUI change becomes a protocol bump (PROTOCOL is already at 25).
- A safer shape is **semantic state, not widget state**: one replicated document per workspace (panes, agent state, chat blocks, pending question, plan, background count) sent as versioned diffs, with the screen text as a fallback field. Much of it exists (`Chat`, `Shell`, `Panes`, `Text`); the real gain is a single revision number and resync after reconnect. This QA did not measure message sizes, so the "ultra compact" benefit is unevidenced; the screen text is the only large payload I saw.

## Suggested changes (all of the above, with the owner's design for B-19)

This is a list for a fix worktree; QA changed no product code.

**B-19, the owner's direction: the row says what is waiting, not "y / n", and y/n goes away.** Replace the y/n buttons on the Home row and in Avisos with a label for the thing Claude is blocked on, and one way in:
- plan: `plan · añadir d.txt` (from `ExitPlanMode`), button **ver plan**;
- one or several questions: `pregunta · Red or blue?` / `elige varias · Which of a, b, c`, button **responder**;
- permission for a tool: `permiso · Write b.txt` / `permiso · Bash: rm -rf x`, button **revisar**;
- the screen-only cases (trust dialog, picker, `/config`): `Claude espera en pantalla`, button **abrir**.
Every button opens the full-screen question already built (REQ-98), which has the whole plan, options and preview. Avisos lists the same line per pane.
- Where the data comes from: the transcript already holds it. The pending call is the last `tool_use` with no `tool_result` (the chat card already says "espera permiso"), and `chat.rs` `summarise()` already makes its one-line summary (`ExitPlanMode` carries the plan body). The daemon would send it per pane (for example `PaneInfo.waiting_for {kind, tool, summary}`), which needs a `PROTOCOL` bump. For a pane with no transcript (the trust dialog) the kind is `screen`.
- **No y/n anywhere** (owner's decision): remove `y · aprobar` / `n · rechazar` from the Home row and the y/n from Avisos, plain permissions included; the row shows the waiting tool or question and its button opens the full-screen question. That makes Settings "respuestas rápidas" (REQ-55) and the y/n parts of REQ-20 and REQ-28 obsolete: drop them with the change, and update `mobile.md` in the same commit.

**The rest, in order:** B-01 crash; B-02, B-03, B-10 with a "ver pantalla" toggle and a Shift+Tab key; B-08 reconnect back to the pane; B-05, B-06 layout; B-07, B-04 theme and the `/model` default; then the low rows B-09 to B-18.

## Setup left behind

- Test daemon: stopped (pid 18492). `/tmp/jwt-qa` holds the demo repo, worktrees and a Claude wrapper script; delete it when done.
- Your real jw: session `qa-test` with the folder `mobile-qa`, folder `demo` and worktree `qa-perm` (a plan-mode haiku pane). Close with `jw session rm qa-test` when you want. Worktrees on disk: `/tmp/jwt-qa/home/demo-wt/qa-perm` and the others from the test daemon.
- The phone is paired to your real daemon (the pairing with the test daemon is gone); nothing to re-pair.
- Claude was run on Haiku with short prompts; the haiku wrapper is `/tmp/jwt-qa/claude-haiku.sh`.
- Skill `jw-mobile-qa` (`.claude/skills/jw-mobile-qa/`) and its `q.py` helper were written for this run; changes made while running: session guard (`QA_SESSION`, `QA_ALLOW`, `QA_PATH`), exact-match tap (`=esc`, because a substring tap on "esc" once hit "escribe a claude").
