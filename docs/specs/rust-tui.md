---
status:      agreed
scope:       [Cargo.toml, src/**, .github/workflows/ci.yml, internal/core/config/config.go (one relaxation), README.md]
depends_on:  [git, gh on PATH]
supersedes:  [herdr backend: internal/backends/terminal, internal/connectors/herdr]
---
# jw in Rust: a TUI that is its own multiplexer (replaces herdr)

## Contract

- One binary `jw`, unix only (macOS + Linux). With no arguments it opens the TUI client; a hidden `jw daemon` runs the server. The only other CLI is two subcommands for agents that talk to the socket: `jw prompt <stream> <text>` and `jw new <name> [--task <text>]` (no `--json`).
- Leader: `Ctrl-Space`, configurable in `[tui] leader` (macOS: disable "Select previous input source").
- **The daemon** owns every terminal: one PTY per pane, with a VT parser that keeps the screen and scrollback. It listens on a Unix socket and survives the client closing (the way herdr/tmux do).
- **The client** draws: a sidebar of **spaces** (= projects, from config) → **streams** (= worktrees, from the registry), plus the open stream's panes laid out in **configurable splits**.
- Layout "A" (chosen; mocks under the fold, "UI"): **fixed sidebar** on the left, the active stream's splits on the right, a header with stream·branch·PR and a status bar showing the mode.
- Two modes: **terminal** (every key goes to the focused pane) and **navigation** (entered with a leader key: move around and run actions). The mouse focuses panes.
- **Animations** everywhere: stream switching, modals, pane focus, state changes in the sidebar, action progress. They never block input.
- **Built-in viewers** (pane types the TUI draws, not PTYs): a **GitHub-style diff viewer** (side by side by default, a file tree, every file stacked in one scroll with sticky headers, "viewed" to collapse, changed words highlighted within the line) and a **Markdown reader** (rendered, read-only).
- **Free space** (`free`), always first in the sidebar: sessions with no repo, branch or ports — a shell (optionally with an agent) in any directory, with their own splits. They live in the daemon like everything else.
- The actions of today's commands (`new open close done rm sync setup dev prompt info init`) live as TUI actions, keeping the same checks as the Go code. Confirmations are modals. Exit code 3 and `--json` go away.
- On-disk compatibility: same config TOML and same `registry.json`. Go and Rust coexist until the cutover.

## Interfaces

| Piece | Where / shape |
|---|---|
| Crate | `Cargo.toml` at the root next to `go.mod`; a library `src/lib.rs` with `src/{core,connectors,daemon,proto,tui}` (the integration tests in `tests/` drive it) and a thin `src/main.rs` |
| core | port of `internal/core/config` (config.go:23-60, expand.go:18-45, ports config.go:263) and `registry` (registry.go:16-81) with serde, `deny_unknown_fields` |
| connectors | git (port 1:1 of the calls in git.go:25-252), gh (`gh pr list`, github.go:43-89), shell/setup/ports (`internal/connectors/system`) |
| Socket | `$XDG_RUNTIME_DIR/jw/jw.sock`, else `~/.local/state/jw/jw.sock` |
| Protocol | `src/proto`: u32 length + serde_json frames. C→D: `Attach{stream}`, `Detach`, `Input{pane,bytes}`, `Resize{pane,cols,rows}`, `Spawn{stream,role,cmd?,cwd,env,cols,rows}` (`cmd` via `sh -c`, none = `$SHELL`), `Kill{pane}`, `List`, `Prompt{stream,text}` (waits for 1.2s of quiet in the `agent` pane, at most 20s); later `Open{stream}`, `Close{stream,force}`. D→C: `Snapshot{pane,role,cols,rows,bytes}` (vt100 `state_formatted`), `Output{pane,bytes}`, `Exited{pane,status}`, `Spawned{pane}`, `Panes{panes}` (with each pane's foreground process), `Prompted{pane}`, `Error{msg}`; later `State{streams}`. Pane ids are `u64` |
| Session | `~/.local/state/jw/session.json`: open streams + layout + per-pane role/cmd, so they can be restored |
| Layout | `[layout]` in the project TOML: tree of `split = "down"\|"right"`, `ratio`, children `a`/`b`, leaves `run = "editor"\|"agent"\|"shell"\|"dev:<svc>"\|"<cmd>"`. The existing `editor` key stays. The default reproduces terminal.go:46-138. Go ignores the tree and `[tui]` (`rustOnly`, config.go) |
| Pane env | the same JW_* that are passed today via `herdr --env` (herdr.go:156) + `JW_PANE_ID` (replaces `HERDR_PANE_ID`, herdr.go:271) |
| Stack | ratatui (its crossterm re-export), portable-pty, vt100, serde/toml/serde_json, anyhow. vt100 screens are drawn by our own widget (`src/tui/draw.rs`) instead of tui-term, to keep one vt100 version |
| Animations | tachyonfx (fade, slide, sweep, dissolve over ratatui buffers); `[tui] animations = true\|false` |
| Diff viewer | `git diff <base>...HEAD` + working tree, parsed into hunks; highlighting with syntect |
| MD reader | pulldown-cmark → styled ratatui `Text`; code blocks with syntect |

## Acceptance

- REQ-1 THE jw binary SHALL read the existing config files and `registry.json` written by the Go binary without migration.
- REQ-2 WHEN the registry is saved, the jw binary SHALL write it in a format the Go binary can read, via a temp file + rename.
- REQ-3 WHEN `jw` starts and no daemon is listening on the socket, the client SHALL start `jw daemon` detached and connect to it.
- REQ-4 WHEN the client exits or dies, the daemon SHALL keep every pane's processes running.
- REQ-5 WHEN a client attaches to a stream, the daemon SHALL send each pane's current screen before any new output.
- REQ-6 THE TUI SHALL show spaces → streams with: branch, open/closed, PR, dev running.
- REQ-7 WHEN a stream is opened, the daemon SHALL create its panes from the layout tree, with cwd = worktree and the JW_* env vars.
- REQ-8 WHERE the project has no `[layout]`, the daemon SHALL use the default layout.
- REQ-9 WHILE in terminal mode, the client SHALL forward every key except the leader to the focused pane.
- REQ-10 WHEN a pane changes size, the daemon SHALL resize its PTY.
- REQ-11 WHEN an action (new/sync/done/rm/setup/dev/info/init) runs, the TUI SHALL apply the same checks as its Go file and ask for confirmation in a modal where Go used `confirm`.
- REQ-12 IF close or rm targets a stream with a non-shell foreground process in any pane, THEN the TUI SHALL require confirmation, listing each pane and its process. IF rm would lose work (uncommitted files, or unpushed commits on a branch it deletes), THEN the TUI SHALL require the stream's name typed (Go's `--force`).
- REQ-13 WHEN `prompt` runs, the daemon SHALL write the text to the agent pane (bracketed paste if the app enabled it) followed by Enter.
- REQ-14 IF a pane's process exits, THEN the daemon SHALL keep its last screen and exit status visible until the pane is closed or relaunched.
- REQ-15 WHEN the daemon starts and finds a `session.json` from a previous daemon, it SHALL recreate the open streams, launching agents with their `resume` command (config.go:42-60).
- REQ-16 THE jw binary SHALL NOT invoke `herdr`.
- REQ-17 WHILE in navigation mode, the TUI SHALL accept `j/k` (move in the sidebar), `↵` (open/attach), `h/j/k/l` with Shift (focus between panes), `n s d x c p` (new, sync, done, rm, close, prompt), `r` (dev), `S` (setup), `i` (info), `I` (add a project, drafting its config like `jw init`), `R` (reload) and `Esc` (back to terminal mode).
- REQ-19 WHEN the active stream, a modal, the pane focus or a stream's state changes, the TUI SHALL animate the transition without delaying input to the panes.
- REQ-20 WHERE `[tui] animations = false`, the TUI SHALL apply every change without animation.
- REQ-21 WHEN the user presses `D` in navigation mode, the TUI SHALL open the stream's diff side by side, with every file in one scroll, `j/k` per file, `]`/`[` per hunk, `v` viewed, `t` ⇄ unified, and changed words highlighted.
- REQ-25 WHEN the user presses `f` (or double-clicks a pane's title), the TUI SHALL show only the focused pane across the whole terminal, hiding the sidebar and header; `F` does the same with all of the stream's panes; pressing the same key again restores the view; `tab` switches pane while the focus is active.
- REQ-26 WHEN the user presses the leader twice (`^␣ ^␣`, or `␣`/`/` in navigation), the TUI SHALL open a Telescope-style finder: fuzzy search over streams, free sessions, files, actions and panes, with matched characters highlighted, a preview of the selection, and the prefixes `>` actions, `@` streams, `/` files, `:` panes.
- REQ-27 WHEN the user picks a result in the finder, the TUI SHALL open it where it belongs: a stream → attach, a `.md` → reader, another file → nvim in that stream, an action → run it, a pane → focus it.
- REQ-23 WHERE a session belongs to the free space, the TUI SHALL create it with no worktree, branch or ports, in the directory the user chooses.
- REQ-24 IF the user runs sync, done, dev or diff on a free session, THEN the TUI SHALL do nothing and say that the action needs a repo.
- REQ-22 WHEN the user opens a `.md` file (from the diff viewer or with `M`), the TUI SHALL show it rendered and read-only: headings, lists, tables, code with highlighting, links.
- REQ-18 THE sidebar SHALL mark each stream with: `●` open / `○` closed, `✻` agent working, `?` agent waiting for input, `⚡` dev running, `⚑` PR merged.

---

## Rationale

- RAT-1 **Daemon + client instead of a single process:** user decision ("copy herdr, they should survive"). The cost is the protocol and reconnection; it pays for itself because agents run for hours.
- RAT-2 **Snapshot + raw bytes** instead of the daemon sending rendered frames: the client runs its own vt100 and draws with tui-term. The daemon doesn't know the client's size per widget, and the protocol stays small. On attach, `state_formatted()` rebuilds the screen exactly.
- RAT-3 **vt100 instead of alacritty_terminal:** simpler API, it has a serializable snapshot, and it supports truecolor. It goes behind a trait in case fidelity falls short (RISK-1).
- RAT-4 **Space = project, stream = worktree:** reuses config (project) and registry (entry) as they are. It replaces herdr's workspace → tab without inventing a new model.
- RAT-5 **TUI only:** user decision. It retires `--json` and exit 3; the `jw` skill shrinks to `jw prompt` / `jw new --task` (OPEN-3).
- RAT-6 **Same files on disk:** this lets Go and Rust run in parallel and compare behavior before deleting the Go code.
- RAT-7 **Rejected — keep herdr or use tmux/zellij as the backend:** the goal is precisely to own the multiplexer and its UX.

### UI (reference mocks, layout A)

Terminal mode — every key goes to the focused pane (bold border):
```
┌ jw ──────────────┬─ jw/backends-tui · feat/tui · PR #31 open ──────────────┐
│ ▾ jw             │ editor (nvim)                                          │
│   ● backends-tui │  1 package main                                        │
│   ○ readme-fix   │  2                                                     │
│   ○ json-dev  ⚑  │  3 import "fmt"                                        │
│ ▾ kanvas         │                                                        │
│   ● auth-flow ⚡ ├─ agent (claude) ──────────────┬─ dev:web :20103 ────────┤
│   ○ billing      │ ✻ Reading ls_tui.go…          │ ▲ ready in 412ms        │
│ ▸ mctekk-web     │ > _                           │ GET / 200 12ms          │
├──────────────────┴───────────────────────────────┴─────────────────────────┤
│ TERM  ^␣ nav · ● open ○ closed ✻ working ? waiting ⚡ dev ⚑ merged         │
└────────────────────────────────────────────────────────────────────────────┘
```

Navigation mode (`^␣`) — sidebar cursor, the status bar lists the actions:
```
┌ jw ──────────────┬─ jw/backends-tui · feat/tui · PR #31 open ──────────────┐
│ ▾ jw             │ (stream panes, dimmed)                                 │
│ ▶ ● backends-tui │                                                        │
│   ○ readme-fix   │                                                        │
│ ▾ kanvas         │                                                        │
│   ● auth-flow ⚡ │                                                        │
├──────────────────┴────────────────────────────────────────────────────────┤
│ NAV  ↵ open  n new  s sync  d done  x rm  c close  p prompt  ⇧hjkl pane  esc│
└────────────────────────────────────────────────────────────────────────────┘
```

Modal `n` (new stream in the selected space):
```
            ┌ New stream · jw ─────────────────────────────┐
            │ Name    [ tui-sidebar_____________ ]         │
            │ From    [ origin/main ▾ ]                    │
            │ Branch  [ feat/tui-sidebar ] (auto)          │
            │ Task    [ add the sidebar to the TUI____ ]   │
            │         (optional: sent to the agent)        │
            │ [x] run setup     [x] open now               │
            │                     ↵ create     esc cancel  │
            └──────────────────────────────────────────────┘
```

Modal `c` with a process running (REQ-12):
```
            ┌ Close auth-flow? ────────────────────────────┐
            │ Still running:                               │
            │   dev:web   pnpm dev   :20103                │
            │   agent     claude     (working)             │
            │ Closing kills them. The worktree is kept.    │
            │                     y close      esc cancel  │
            └──────────────────────────────────────────────┘
```

Action output (sync, setup, done) — temporary pane at the bottom of the stream, it closes with `esc` when finished:
```
├─ sync · rebase onto origin/main ────────────────────────── ✓ done ─────────┤
│ Successfully rebased and updated refs/heads/feat/tui.                      │
```

### Milestones (each one closes with green tests)

1. **Core + connectors:** config, registry, expand, ports, git, gh, shell. Tests with fixtures written by the Go binary.
2. **Daemon + PTY + protocol:** one pane, attach/detach, survives the client, resize, exit.
3. **TUI:** sidebar, layout tree → rects, focus, modes, mouse.
4. **Actions:** new, open, close, sync, done, rm, setup, dev, prompt, info, init (port of the matching `internal/commands/*.go`).
5. **Restore + cutover:** session.json/resume, delete the Go code and herdr, README, retire the `jw` skill.

## Risks

- RISK-1 **Emulation fidelity** (Claude Code, nvim: alt screen, mouse, OSC 52, bracketed paste). A partial emulator breaks the main experience. Mitigation: a trait over the parser, plus manual tests with claude and nvim in M2.
- RISK-2 **Go rejects unknown keys** (`decodeFile`, config.go): adding `[layout]` children breaks the Go binary during the parallel phase. Resolved in P0: `rustOnly` skips them, and a typo at the top level still fails.
- RISK-3 **"Agent ready" without `herdr agent wait`** (herdr.go:265): writing before the agent finishes booting loses the prompt. Built in P7: the daemon waits for 1.2s without output (at most 20s). An agent stopped at a dialog (trust, login) is quiet too, so the prompt lands in the dialog; Go detected that through herdr, Rust doesn't yet.
- RISK-4 **Daemon crash = all terminals die.** REQ-15 limits the damage (resume), but the in-flight state is lost.
- RISK-5 **Concurrent writes to `registry.json`** between Go and Rust during the parallel phase, with no lock. Last writer wins. Accepted until the cutover.
- RISK-7 **`✻ working` / `? waiting` (REQ-18) are a heuristic:** without herdr there's no agent API. Infer them from recent output activity, the bell (BEL) and the window title (OSC 0/2, which Claude Code updates). They can be wrong; worst case they show `●` and nothing else.
- RISK-12 **dev and setup type into the shell pane** (refused while it runs something else) instead of a pane of their own. Several commands of one service run as `(trap 'kill 0' INT TERM; a & b & wait)`, so ctrl+c stops them together. A `[layout]` leaf `run = "dev:<svc>"` gives a service its own pane instead.
- RISK-11 **Setup runs in the shell pane** of a new stream (`printf` of the line, the commands, then `exec $SHELL`) instead of before opening, as Go did. Its output stays visible, but a failed setup doesn't stop the stream from opening; the shell is right there to retry.
- RISK-6 **Scope creep toward tmux:** copy mode, text selection, search in scrollback. v1 scope: scroll with the wheel and plain mouse selection; nothing else.

- RISK-9 **Bytes go as JSON number arrays** (3–4× the raw output) and a slow client's queue has no bound. Fine for P3; switch to a binary codec and add backpressure if dev logs make it show.
- RISK-10 **macOS leaks sockets into children** forked by other threads (close-on-exec is set after `socket()`). Tests that check a port or socket is gone must wait for that child to exit, not assert at once.

- RISK-8 **Animations vs. PTY output:** a 60fps render loop competing with lots of output (logs from dev) can raise CPU usage. Animate only during transitions, and when idle, render on demand.

## Parts (one PR each, each closes with green tests)

Order of dependencies: P0 → {P1, P3} in parallel → P2 → P4 → P5 → P6…P12. Go keeps working throughout.

| Part | What | REQs | Done when |
|---|---|---|---|
| P0 | Spec at `docs/specs/rust-tui.md`; `Cargo.toml` + `src/main.rs` skeleton (unix only); `cargo test` + `clippy` in CI; Go relaxation to ignore `layout.*`/`tui.*` (config.go:132-150) | — | `cargo build`, `go test ./...` green |
| P1 | `core`: config + expand + ports + registry (serde, temp+rename) | 1, 2 | parses fixtures written by Go; Go reads what Rust writes |
| P2 | `connectors`: git, gh, shell/setup/ports behind traits + fakes | — | git against a temp repo; fakes like fakes_test.go |
| P3 | `proto` + `daemon`: socket, frames, one PTY pane, attach/detach/snapshot, resize, exit, autostart | 3, 4, 5, 10, 14 | integration test with `cat`: kill client, reattach, same screen |
| P4 | Layout tree → rects; open a stream = spawn its panes with JW_* env | 7, 8 | layout tests for several sizes; default = terminal.go:46-138 |
| P5 | TUI client: sidebar, splits with tui-term, modes, leader, mouse, `f`/`F` focus, status heuristics | 6, 9, 17, 18, 25 | `TestBackend` + insta snapshots; manual nvim + claude |
| P6 | Actions I: new, open, close, rm + modals | 11, 12 | same checks as the Go commands, fakes |
| P7 | Actions II: sync, done, setup, dev, prompt, info, init + `jw prompt`/`jw new` subcommands | 11, 13 | prompt reaches a running agent (RISK-3) |
| P8 | Free space | 23, 24 | create/close a free session; repo actions refuse |
| P9 | Viewers: GitHub-style diff + MD reader | 21, 22 | snapshots for split/unified and a rendered md |
| P10 | Finder | 26, 27 | fuzzy ranking table tests + snapshot |
| P11 | Animations (tachyonfx) + off switch | 19, 20 | input latency unchanged with animations on |
| P12 | Restore (session.json/resume) + cutover: delete Go + herdr, README, trim the `jw` skill | 15, 16 | restart daemon → streams come back; no `herdr` in the tree |

P0 is the PR that adds this file.

## Decided (were open questions)

- OPEN-1 → `Ctrl-Space`, configurable.
- OPEN-2 → `docs/specs/rust-tui.md`.
- OPEN-3 → keep `jw prompt` and `jw new --task` as socket clients.
- OPEN-4 → unix only; `shell_windows.go` is not ported.

## Open questions

- OPEN-5 `[tui]` lives in the project TOML, but the leader and animations are global to the client. Until there is a global file, the leader comes from `JW_LEADER` (`C-Space` by default). A global `~/.config/jw/tui.toml` would be read by Go's personal-config glob, so it needs a name or place Go skips.

## Corrections

- The first proposal kept the CLI underneath the TUI; the user chose TUI only.
- The first proposal assumed the TUI would be a client of herdr; the user asked for it to replace herdr.
- The spec put tui-term in the stack; P5 draws vt100 screens itself (see Interfaces, Stack).
- RISK-2 assumed `[layout]` was a new table. It already exists with `editor`, so Go keeps decoding `layout.editor` and ignores only the nested keys and `[tui]`.

## Tests

- core: parse config/registry from Go fixtures; round-trip of registry.json read back by the Go binary (`go run . ls --json` in an integration test during the parallel phase).
- expand/ports: table-driven, ported from `config_test.go`.
- connectors: fakes behind traits, like `internal/commands/fakes_test.go`; git against a temp repo.
- daemon: integration with a daemon in a tempdir — pane running `cat`, input, detach, kill the client, reattach, verify the snapshot.
- layout: tree → rects for different sizes.
- TUI: ratatui `TestBackend` + insta snapshots of the sidebar, modals and splits.
- manual: claude and nvim inside a pane, resize, attach from two terminals.
