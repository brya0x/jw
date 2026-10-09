---
status:      agreed (v2, 2026-10-09: the simple-flow redesign; P0–P9 built against v1)
scope:       [Cargo.toml, src/**, .github/workflows/ci.yml, internal/core/config/config.go (one relaxation), README.md]
depends_on:  [git, gh on PATH]
supersedes:  [herdr backend: internal/backends/terminal, internal/connectors/herdr]
---
# jw in Rust: a TUI that is its own multiplexer (replaces herdr)

## Contract

- One binary `jw`, unix only (macOS + Linux). With no arguments it opens the TUI client; a hidden `jw daemon` runs the server. The only other CLI is `jw prompt <stream> <text>` and `jw new <name> [--task <text>]`, for agents (no `--json`).
- **Two concepts.** A *workspace* is an open folder: a project (any folder, git or not) or one of its worktrees. A workspace holds *panes*.
- **One-shot leader.** `Ctrl-Space`, then one key, then back to the terminal (tmux-style). A pause of 600 ms after the leader shows every key; `?` shows them at once. There is no navigation mode.
- **Every action applies to the current workspace or the focused pane.** The sidebar is a list to read and click, numbered for `1–9`.
- **The daemon** owns every terminal (one PTY per pane, a VT parser with screen and scrollback) and each workspace's pane tree (splits, ratios, names). It survives the client and writes `session.json` on every change.
- **The client** draws the sidebar (projects with their worktrees indented), the current workspace's panes, a header (workspace · branch · PR) and a status bar. It owns only focus and the full view.
- **Viewers are panes the client draws:** a GitHub-style diff (unified when narrow, side by side when wide, sticky file headers, viewed, changed words) and a Markdown reader. Code files open in nvim; there is no built-in editor or file tree.
- **Theme:** Atom One Dark / One Light, following the terminal or the OS.
- The checks of today's commands (`new rm done sync setup dev info init`) stay; confirmations are modals. Exit code 3 and `--json` go away.
- On-disk compatibility: same config TOML and `registry.json` as Go until the cutover. Alt is never bound (AeroSpace owns it).

### Keymap (after `^␣`)

| Go | Worktree | Panes |
|---|---|---|
| `␣` switch workspace (fuzzy, most recent first, preview) | `w` new worktree from the current branch: `ws-N`, branch from the `branch` template, setup runs | `hjkl` focus |
| `tab` previous workspace | `r` rename: name, branch and folder | `t` new shell beside the focused pane |
| `1–9` the sidebar's number | `s` sync (on a project root: `pull --ff-only`) | `x` close the pane; on the last one, close the workspace |
| `o` open a folder (browser: arrows, type to filter, `~`, a new name creates it) | `d` diff pane against the base | `n` name the pane (empty = automatic title) |
| `/` open a file (`.md` → reader pane, else nvim) | `X` remove: done checks if the PR is merged, rm checks otherwise | `f` full · `HJKL` swap with the neighbour |
| `?` keys · `q` detach | | |

## Interfaces

| Piece | Where / shape |
|---|---|
| Crate | `Cargo.toml` next to `go.mod`; library `src/lib.rs` with `src/{core,connectors,daemon,proto,tui,view}` (driven by `tests/`) and a thin `src/main.rs` |
| core | port of `internal/core/config` and `registry` with serde, `deny_unknown_fields`. Worktrees at `root/<name>` (default `<repo>-wt`), branch template default `feat/{name}` (`src/core/config.rs`) |
| connectors | git (1:1 with git.go, plus `worktree_move` and `branch_rename`), gh (`gh pr list`), shell/setup/ports |
| Socket | `$XDG_RUNTIME_DIR/jw/jw.sock`, else `~/.local/state/jw/jw.sock` |
| Protocol | `src/proto`: u32 length + serde_json frames; the first frame on attach carries the binary version and a mismatch fails loudly. C→D: `Attach{ws}`, `Detach`, `Input{pane,bytes}`, `Resize{pane,cols,rows}`, `Open{ws,dir,env,tree}`, `Split{pane,dir,leaf}`, `Kill{pane}` (removes the leaf), `Swap{a,b}`, `Name{pane,name?}`, `Close{ws}`, `List`, `Prompt{ws,text}`. D→C: `Snapshot{pane,…}`, `Output{pane,bytes}`, `Exited{pane,status}`, `Tree{ws,tree}` (after every change), `Title{pane,title}` (OSC 0/2 via `vt100::Callbacks`), `Panes{…}` (with foreground process), `Prompted{pane}`, `Error{msg}`. Pane ids are `u64` |
| Tree | `src/layout.rs`: leaves carry `{id, run, name?}`; `run = "shell"\|"agent"\|"editor"\|"dev:<svc>"\|"view:diff"\|"view:md:<path>"\|"<cmd>"`; `view:*` leaves have no PTY. Ops: `insert(beside, dir, leaf)`, `remove(id)` (the sibling takes the space), `swap(a,b)`, `neighbour(id,dx,dy)` (nearest rect that overlaps on the other axis), `rects` |
| Layout config | `[layout]` in the project TOML as in v1 (tree of `split`, `ratio`, `a`/`b`, leaves `run`). It is the starting tree when a workspace opens; without it, one shell for a plain folder and the default tree for a project. Go ignores the tree and `[tui]` (`rustOnly`, config.go) |
| Session | `~/.local/state/jw/session.json`, written by the daemon with temp + rename: `{workspaces: [{id, dir, tree, used}], recent_dirs}` |
| Folders | `src/folders.rs` (replaces `src/free.rs`): open project folders `{id, dir, opened}` in `~/.local/state/jw/folders.json`; the first run migrates `free.json`. A git folder's worktrees come from the registry by project name |
| Pane env | the JW_* vars of `actions::jw_env` (a plain folder gets JW_ID, JW_NAME, JW_PROJECT) + `JW_PANE_ID` |
| Theme | `src/tui/theme.rs`: a `Palette` of One Dark / One Light values used by every draw module. Pick: `JW_THEME=dark\|light\|auto` (OPEN-5), else the terminal's background (OSC 11 at start; mode 2031 updates on Ghostty/kitty), else `defaults read -g AppleInterfaceStyle` |
| Stack | ratatui (its crossterm re-export), portable-pty, vt100 0.16 (drawn by `src/tui/draw.rs`), serde/toml/serde_json, syntect, pulldown-cmark, anyhow |
| Diff | `src/diff.rs` (model: `git diff -M --merge-base origin/<base>` + untracked) and `src/tui/diffview.rs` (drawing, keys `j k ] [ v t ␣ b ↵ q`) |
| MD reader | `src/view/md.rs` (pulldown-cmark → wrapped lines + headings) and `src/tui/mdview.rs` |
| Pickers | `src/tui/finder.rs`: one fuzzy scorer for the switcher (`␣`) and files (`/`); the folder browser (`o`) lists one directory at a time |

## Acceptance

- REQ-1 THE jw binary SHALL read the existing config files and `registry.json` written by the Go binary without migration.
- REQ-2 WHEN the registry is saved, the jw binary SHALL write it in a format the Go binary can read, via a temp file + rename.
- REQ-3 WHEN `jw` starts and no daemon is listening on the socket, the client SHALL start `jw daemon` detached and connect to it.
- REQ-4 WHEN the client exits or dies, the daemon SHALL keep every pane's processes running.
- REQ-5 WHEN a client attaches to a workspace, the daemon SHALL send its tree and each pane's current screen before any new output.
- REQ-10 WHEN a pane changes size, the daemon SHALL resize its PTY.
- REQ-11 WHEN an action (new/sync/done/rm/setup/dev/info/init) runs, the TUI SHALL apply the same checks as its Go file and ask for confirmation in a modal where Go used `confirm`.
- REQ-12 IF closing or removing would stop a non-shell foreground process, THEN the TUI SHALL require confirmation, listing each pane and its process. IF removing would lose work (uncommitted files, or unpushed commits on a branch it deletes), THEN the TUI SHALL require the worktree's name typed.
- REQ-13 WHEN `prompt` runs, the daemon SHALL write the text to the agent pane (bracketed paste if the app enabled it) followed by Enter.
- REQ-14 IF a pane's process exits, THEN the daemon SHALL keep its last screen and exit status visible until the pane is closed.
- REQ-15 WHEN the daemon starts and finds a `session.json` from a previous daemon, it SHALL recreate its workspaces and trees, launching agents with their `resume` command.
- REQ-16 THE jw binary SHALL NOT invoke `herdr`.
- REQ-21 WHILE a diff pane is focused, the TUI SHALL show every changed file in one scroll with `j/k` per file, `]`/`[` per hunk, `v` viewed, `t` unified ⇄ side by side, and changed words highlighted; side by side by default when the pane is wide.
- REQ-22 WHEN a `.md` file is opened, the TUI SHALL show it rendered and read-only in a reader pane: headings, lists, tables, code with highlighting, links.
- REQ-30 WHEN the user presses the leader and then one key, the TUI SHALL run that key's action and return to the terminal.
- REQ-31 WHILE the leader has been pending for 600 ms, the TUI SHALL show every key, grouped go / worktree / panes.
- REQ-32 THE TUI SHALL forward every key except the leader to the focused pane, or to the viewer when the focused pane is a viewer.
- REQ-33 THE sidebar SHALL list projects with their worktrees indented, numbered 1–9, marked `●` open / `○` closed, `✻` agent working, `?` agent waiting, `⚡` dev running, `⚑` PR merged, and scroll when longer than the screen.
- REQ-34 WHEN `t`, `x`, `HJKL` or `n` changes the panes, the daemon SHALL update the workspace's tree, send `Tree` to every attached client, and write `session.json`.
- REQ-35 WHEN the last pane of a workspace is closed, the TUI SHALL ask first, then close the workspace; a worktree stays listed as `○`, a project leaves the sidebar.
- REQ-36 WHERE a pane has a name, the TUI SHALL show it as the pane's title; otherwise the title SHALL be the program's OSC title, else its foreground process.
- REQ-37 WHEN the user picks a folder with `o`, the TUI SHALL open it as a workspace with its `[layout]` (or one shell for a plain folder); IF it is already open, THEN the TUI SHALL switch to it.
- REQ-38 WHEN `w` runs, the TUI SHALL create the worktree `ws-N` (lowest free N) from the current workspace's branch, run setup, and switch to it.
- REQ-39 WHEN `X` runs on a worktree, the TUI SHALL apply the done checks if its PR is merged and the rm checks otherwise; IF the workspace is a project root, THEN the TUI SHALL refuse and say that jw never deletes a project folder.
- REQ-40 WHEN `r` runs on a worktree, the TUI SHALL validate the name, then move the folder, rename the branch, update the registry, and restart the workspace's panes in the new folder (asking first if an agent or editor runs).
- REQ-41 THE switcher SHALL rank workspaces by last use, filter them fuzzily, and preview path, branch, PR, agent state and panes.
- REQ-42 WHEN a file is picked with `/` or `↵` in the diff, the TUI SHALL open a `.md` in the workspace's reader pane and any other file in its nvim pane, creating either on the right when missing.
- REQ-43 THE TUI SHALL draw with One Dark or One Light, chosen as Interfaces → Theme says.
- REQ-19 WHEN the workspace, a modal or the pane focus changes, the TUI MAY animate the transition, never delaying input to the panes. *(Later; optional.)*
- REQ-20 WHERE `[tui] animations = false`, the TUI SHALL apply every change without animation.

---

## Rationale

- RAT-1 **Daemon + client instead of a single process:** user decision ("copy herdr, they should survive"). The cost is the protocol and reconnection; it pays for itself because agents run for hours.
- RAT-2 **Snapshot + raw bytes** instead of rendered frames: the client runs its own vt100. On attach, `state_formatted()` rebuilds the screen exactly.
- RAT-3 **vt100 instead of alacritty_terminal:** simpler API, a serializable snapshot, truecolor, and title callbacks (0.16).
- RAT-5 **TUI only:** user decision. It retires `--json` and exit 3; the `jw` skill shrinks to `jw prompt` / `jw new --task`.
- RAT-6 **Same files on disk:** Go and Rust run in parallel until the cutover.
- RAT-7 **Rejected: keep herdr or use tmux/zellij as the backend.** The goal is to own the multiplexer and its UX.
- RAT-8 **v2, the simple flow.** Using v1 showed too many concepts: a sticky NAV mode, a sidebar cursor as the target of every action, "free" sessions beside projects, and panes fixed by the layout. The user iterated an HTML prototype (https://claude.ai/artifact/YRDmJ9MScb95oQNALDeBtR) down to two concepts and 18 keys, with capitals only for removing or moving. Merging `done` into `X` (by PR state) and closing the workspace on its last pane each removed a key.
- RAT-9 **The tree moves into the daemon** because panes are now dynamic: if the client kept it, a detach would lose every `t`/`x`/`HJKL`, and two clients could disagree. Focus and the full view stay in the client because they are per-viewer.
- RAT-10 **Rejected: a GUI (Tauri + xterm.js).** Better terminal fidelity and diff rendering, but it leaves the terminal (no SSH, another window under AeroSpace), discards the TUI code, and needs signing and updates. The daemon keeps that door open: a GUI would be another client. Revisit only if RISK-1 fails the checkpoint.
- RAT-11 **Rejected: a built-in editor and file tree** (prototyped). Typing is easy; large files, wide characters and edits racing the agent are not, and it never reaches nvim.

### UI (reference, v2)

```
┌ WORKSPACES   ^␣ o ┬─ jitsubai/auth-flow · feat/auth-flow · PR #42 open ─────┐
│1 ● jitsubai       │ editor ── nvim app/login/actions.ts ┬ agent ── claude ──┤
│2 ↳● auth-flow ✻ ⚡│  1 import { signIn } from "@/lib/auth"│ ✻ Reading …       │
│3 ↳● billing ⚑     │  2                                  │ > _               │
│4 ● kanvas         ├ logs ── zsh ────────────────────────┴───────────────────┤
│5 ↳● export-pdf ?  │ ~/ws/jitsubai-wt/auth-flow feat/auth-flow ❯ _           │
├───────────────────┴─────────────────────────────────────────────────────────┤
│ TERM  ^␣ then a key · ␣ switch · o open · w worktree · t pane · ? all keys  │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Risks

- RISK-1 **Emulation fidelity** (Claude Code, nvim: alt screen, mouse, OSC 52, bracketed paste) decides whether the TUI works. Checkpoint after Q2: a week of daily use; failing it reopens RAT-10.
- RISK-2 **Go rejects unknown keys.** Resolved in P0: `rustOnly` skips `[layout]` children and `[tui]`.
- RISK-3 **"Agent ready" without `herdr agent wait`:** the daemon waits for 1.2 s of quiet (at most 20 s). An agent stopped at a dialog is quiet too, so the prompt lands in the dialog.
- RISK-4 **Daemon crash = all terminals die.** REQ-15 limits the damage; in-flight state is lost.
- RISK-5 **Concurrent writes to `registry.json`** between Go and Rust, with no lock. Last writer wins until the cutover.
- RISK-6 **Scope creep toward tmux** (copy mode, scrollback search). Scope: wheel scroll and plain mouse selection.
- RISK-7 **`✻` / `?` are a heuristic** from output activity, BEL and the OSC title. Worst case they show `●` only.
- RISK-8 **Animations vs. PTY output** can raise CPU. Animate only during transitions.
- RISK-9 **Bytes go as JSON number arrays** and a slow client's queue has no bound. Switch to a binary codec and backpressure if dev logs make it show.
- RISK-10 **macOS leaks sockets into children** forked by other threads; tests wait for that child to exit.
- RISK-11 **Setup runs in the shell pane** of a new worktree, so a failed setup does not stop it from opening.
- RISK-12 **dev types into the shell pane** unless `[layout]` has a `dev:<svc>` leaf. With no dev key in v2, a project's dev servers should be `dev:<svc>` leaves.
- RISK-13 **syntect's bundled grammars have no TypeScript or TOML** (ts uses JavaScript; TOML is plain).
- RISK-14 **Protocol change (RAT-9):** an old daemon and a new client disagree. The version check on attach says "restart the daemon" instead of misbehaving.
- RISK-15 **Rename under running processes** leaves shells with a stale `$PWD`; REQ-40 restarts the panes.
- RISK-16 **`↵` in the diff sends `<Esc>:e path<CR>` to nvim**, assuming normal mode after Esc. Fine for v2; `nvim --server` later if it bites.

## Parts (each one ends with `cargo test` + `clippy` green and a local commit on `feat/rust-tui`; nothing is pushed)

P0–P9 were built against v1: core, connectors, daemon, layout, the first TUI, actions, free space, diff and Markdown viewers.

| Part | What | REQs | Main files |
|---|---|---|---|
| Q1 | One-shot leader + which-key; actions on the current workspace; drop NAV and the sidebar cursor; `X` merges done/rm; status bar | 30–32, 39 | `tui/mod.rs`, `tui/keys.rs`, `tui/draw.rs`, `tui/modal.rs` |
| Q2 | The daemon owns the tree: layout ops, the new messages, version check, `session.json`; `t x HJKL f n`; last pane closes the workspace; OSC titles | 5, 34–36 | `layout.rs`, `proto/mod.rs`, `daemon/mod.rs`, `stream.rs`, `tui/mod.rs` |
| — | **Checkpoint:** a week of daily use with Claude Code and nvim (RISK-1) | | |
| Q3 | Folders: `free.rs` → `folders.rs` + migration; `o` folder browser; a project root is a workspace; sidebar grouping; `w` = `ws-N` | 33, 37, 38 | `folders.rs`, `tui/modal.rs`, `actions.rs`, `tui/mod.rs` |
| Q4 | Switcher `␣`, `tab`, recency; `/` file picker | 41, 42 | `tui/finder.rs` |
| Q5 | Viewers as panes (`view:diff`, `view:md`); `↵` in the diff opens the file | 21, 22, 42 | `tui/diffview.rs`, `tui/mdview.rs`, `tui/draw.rs` |
| Q6 | Rename `r` | 40 | `connectors/git.rs`, `actions.rs`, `core/registry.rs` |
| Q7 | Themes | 43 | `tui/theme.rs` |
| Q8 | Restore from `session.json` on daemon start | 15 | `daemon/mod.rs` |
| Later | Animations (optional); cutover: delete Go + herdr, README, trim the `jw` skill | 16, 19, 20 | |

Reuse: `focus_towards` (`tui/mod.rs`) becomes `layout::neighbour`; `actions::{new_stream, rm_plan, rm, sync, done_plan}`, `src/diff.rs`, `src/view/md.rs` and `Modal::Rm` (type the name) stay as they are.

## Decided (were open questions)

- OPEN-1 → `Ctrl-Space`, configurable.
- OPEN-2 → `docs/specs/rust-tui.md`.
- OPEN-3 → keep `jw prompt` and `jw new --task` as socket clients.
- OPEN-4 → unix only; `shell_windows.go` is not ported.
- OPEN-6 → TUI, not a GUI (RAT-10).

## Open questions

- OPEN-5 `[tui]` lives in the project TOML, but leader, theme and animations are global to the client. Until there is a global file, they come from `JW_LEADER` (`C-Space`) and `JW_THEME` (`auto`). A global `~/.config/jw/tui.toml` would be read by Go's personal-config glob, so it needs a name or place Go skips.

## Corrections

- The first proposal kept the CLI underneath the TUI; the user chose TUI only.
- The first proposal assumed the TUI would be a client of herdr; the user asked for it to replace herdr.
- v1 put tui-term in the stack; P5 draws vt100 screens itself.
- RISK-2 assumed `[layout]` was a new table. It already existed with `editor`.
- v1's "spaces → streams" with a free space first, a sticky NAV mode (REQ-17), `F` focus-all (REQ-25) and a Telescope finder with `> @ / :` prefixes (REQ-26/27) were replaced by v2's workspaces, one-shot leader and three plain pickers.
- The 2026-10-09 prototype's `.worktrees/<name>` on `jw/<name>` was wrong for this repo: worktrees live at `<repo>-wt/<name>` on the `branch` template (`feat/{name}`).

## Tests

- core: parse config/registry from Go fixtures; registry.json round-trip read back by Go.
- layout: tree → rects for several sizes; `insert`/`remove`/`swap`/`neighbour` as tables.
- daemon (tempdir, `tests/`): `cat` pane, detach, kill the client, reattach, same screen; `Split`/`Kill`/`Swap`/`Name` produce `Tree` and update `session.json`; `printf '\e]2;hi\a'` arrives as `Title`; a version mismatch is an error.
- actions: fakes behind traits; `X` takes the done or rm path by PR state; rename against a temp repo; `free.json` → `folders.json` migration.
- finder: fuzzy ranking tables; folder browser over a temp dir.
- TUI: ratatui `TestBackend` for the which-key popup, sidebar and modals.
- manual (after Q2 and Q5): claude and nvim in panes; `w`, `t`, `x`, `HJKL`, `n`, `d`, `↵` in the diff, `q`, then `jw` again shows the same layout.
