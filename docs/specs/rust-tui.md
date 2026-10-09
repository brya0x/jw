---
status:      agreed (v2 + the screen addendum + addendum 3 "sessions", 2026-10-09; P0–P9 built against v1)
scope:       [Cargo.toml, src/**, .github/workflows/ci.yml, internal/core/config/config.go (one relaxation), README.md]
depends_on:  [git, gh on PATH]
supersedes:  [herdr backend: internal/backends/terminal, internal/connectors/herdr]
---
# jw in Rust: a TUI that is its own multiplexer (replaces herdr)

## Contract

- One binary `jw`, unix only (macOS + Linux). With no arguments it opens the TUI client; a hidden `jw daemon` runs the server. The other commands are socket clients for sessions and for agents (addendum 3): `jw new <session>`, `jw [session]`, `jw sessions`, `jw ls`, `jw read`, `jw worktree`, `jw prompt`, `jw hook`.
- **Two concepts.** A *workspace* is an open folder: a project (any folder, git or not) or one of its worktrees. A workspace holds *panes*.
- **One-shot leader.** `Ctrl-Space`, then one key, then back to the terminal (tmux-style). A pause of 600 ms after the leader shows every key; `?` shows them at once. There is no navigation mode.
- **Every action applies to the current workspace or the focused pane.** The sidebar is a list to read and click, numbered for `1–9`.
- **The daemon** owns every terminal (one PTY per pane, a VT parser with screen and scrollback) and each workspace's pane tree (splits, ratios, names). It survives the client and writes `session.json` on every change.
- **The client** draws the sidebar (projects with their worktrees indented), the current workspace's panes, a header (workspace · branch · PR) and a status bar. It owns only focus and the full view.
- **Viewers are panes the client draws:** a GitHub-style diff (unified when narrow, side by side when wide, sticky file headers, viewed, changed words) and a Markdown reader. Code files open in nvim; there is no built-in editor or file tree.
- **Theme:** Atom One Dark / One Light, following the terminal or the OS.
- The checks of today's commands (`new rm done sync setup dev info init`) stay; confirmations are modals. Exit code 3 and `--json` go away.
- On-disk compatibility: same config TOML as Go; Rust keeps its own `workspaces.json` (addendum 3). Alt is never bound (AeroSpace owns it).

### Keymap (after `^␣`)

| Go | Worktree | Panes |
|---|---|---|
| `␣` switch workspace (fuzzy, most recent first, preview) | `w` new worktree from the current branch: `ws-N`, branch from the `branch` template, setup runs | `hjkl` focus |
| `tab` previous workspace | `r` rename: name, branch and folder | `t` new shell beside the focused pane |
| `1–9` the sidebar's number | `s` sync (on a project root: `pull --ff-only`) | `x` close the pane; on the last one, close the workspace |
| `o` open a folder (browser: arrows, type to filter, `~`, a new name creates it) | `d` diff pane against the base | `n` name the pane (empty = automatic title) |
| `/` open a file (`.md` → reader pane, else nvim) | `X` remove: done checks if the PR is merged, rm checks otherwise | `f` full: the focused pane fills the panes area, the sidebar and bars stay · `HJKL` swap with the neighbour |
| `?` keys · `q` detach | | |

### Screen (what each area shows; replaces "UI (reference, v2)")

```
┌ WORKSPACES  ^␣ o open ┬ jitsubai/auth-flow  feat/auth-flow  from main  PR #42 open      ~/ws/jitsubai-wt/auth-flow ┐
│ 1 ● jitsubai          │╭ editor  nvim app/login/actions.ts ─╮╭ agent  claude ──────────╮                        │
│ 2   ↳● auth-flow ✻ ⚡ ││ …                                  ││ ✻ Reading …             │                        │
│ 3   ↳● billing   ⚑    ││                                    ││ > _                     │                        │
│ 4 ● kanvas            │├ logs  zsh ─────────────────────────┴┴─────────────────────────┤                        │
│ 5   ↳● export-pdf ?   ││ ~/ws/jitsubai-wt/auth-flow feat/auth-flow ❯ _                 │                        │
│ 6 ○ notes             │╰──────────────────────────────────────────────────────────────╯                        │
│ ● open ○ closed ✻ agent ? waiting ⚡ dev ⚑ merged                                                               │
├ TERM  ^␣ then a key · ␣ switch · o open · w worktree · t pane · ? all keys                   created ws-1 on … ┤
```

| Area | Rule |
|---|---|
| Sidebar title | `WORKSPACES` (dim, bold), with `^␣ o open` right-aligned |
| Sidebar rows | One row per workspace, numbered in order (1–9 shown). A project row: `N ● name`. Its worktrees follow it, indented: `N   ↳● name`. `●` green when open, `○` dim when closed (and the name dim). The current row has a `sel` background and a blue bold name. Marks are right-aligned in the row. No blank lines, no project headings, no "free" section |
| Sidebar legend | Last line: `● open ○ closed ✻ agent ? waiting ⚡ dev ⚑ merged`, dim, the symbols in their colours |
| Marks | `✻` magenta: the agent pane printed in the last 2 s. `?` yellow: the agent pane rang the bell (BEL or OSC 9) since its last input. `⚡` green: a `dev:*` pane is running. `⚑` cyan: the branch's PR is merged |
| Header | `project/` dim + name blue bold (a project root shows just its name), branch magenta, `from <base>` dim (worktrees only), `PR #N <state>` dim when there is one, path dim right-aligned with `~`. A plain folder says `not a git repo` in place of the branch |
| Pane title | `name` if set, else `role` bold + what runs dim (`nvim app/x.ts`, the OSC title, or the foreground process). Focused: blue border and title. Exited: `exited N` red. Viewers: `changes  auth-flow vs main · 4 files`, `md  docs/x.md` |
| Status bar | Mode chip: `TERM` green, `^␣` yellow (leader pending), `DIFF`/`MD` blue (a viewer is focused), `CONFIRM` red (a yes/no modal). Then the hint for that mode. A message goes right-aligned: green for done, red for an error, cleared after 4 s |
| Which-key | Three columns as now, adding `␣ switch`, `o open`, `/ file` (go) and `r rename` (worktree) |
| Empty stage | `^␣ o opens a folder · ^␣ ␣ switches workspace`, centred, dim |

## Interfaces

| Piece | Where / shape |
|---|---|
| Crate | `Cargo.toml` next to `go.mod`; library `src/lib.rs` with `src/{core,connectors,daemon,proto,tui,view}` (driven by `tests/`) and a thin `src/main.rs` |
| core | port of `internal/core/config` and `registry` with serde, `deny_unknown_fields`. Worktrees at `root/<name>` (default `<repo>-wt`), branch template default `feat/{name}` (`src/core/config.rs`) |
| connectors | git (1:1 with git.go, plus `worktree_move` and `branch_rename`), gh (`gh pr list`), shell/setup/ports |
| Socket | `$XDG_RUNTIME_DIR/jw/jw.sock`, else `~/.local/state/jw/jw.sock` |
| Protocol | `src/proto`: u32 length + serde_json frames; a workspace is named by its registry id in `stream`. The client opens with `Hello{protocol}` and refuses a daemon that answers anything but the same `PROTOCOL` (RISK-14). C→D: `Hello`, `Attach{stream}`, `Detach`, `Input{pane,bytes}`, `Resize{pane,cols,rows}`, `Open{stream,tree}` (a `Tree<NewPane>`), `Split{pane,dir,new}`, `Kill{pane}` (removes the leaf), `Close{stream}`, `Swap{a,b}`, `Name{pane,name?}`, `List`, `Prompt{stream,text}`, and the older `Spawn` (one pane, placed on the right). D→C: `Hello`, `Tree{stream,tree}` (on attach and after every change), `Snapshot{pane,…}`, `Output{pane,bytes}`, `Title{pane,title}` (OSC 0/2 via `vt100::Callbacks`), `Exited{pane,status}`, `Panes{…}` (with foreground process), `Spawned{pane}`, `Prompted{pane}`, `Error{msg}`. Pane ids are `u64` |
| Tree | `src/layout.rs`: leaves carry `{id, run, name?}`; `run = "shell"\|"agent"\|"editor"\|"dev:<svc>"\|"view:diff"\|"view:md:<path>"\|"<cmd>"`; `view:*` leaves have no PTY. Ops: `insert(beside, dir, leaf)`, `remove(id)` (the sibling takes the space), `swap(a,b)`, `neighbour(id,dx,dy)` (nearest rect that overlaps on the other axis), `rects` |
| Layout config | `[layout]` in the project TOML as in v1 (tree of `split`, `ratio`, `a`/`b`, leaves `run`). It is the starting tree when a workspace opens; without it, one shell for a plain folder and the default tree for a project. Go ignores the tree and `[tui]` (`rustOnly`, config.go) |
| Session | `session.json` in the state dir (next to the socket for any socket but the default one, so tests never touch it), written by the daemon with temp + rename after every tree change: `{workspaces: [{id, tree}]}`, each leaf with role, name, cmd, cwd and env. Recency and recent folders arrive with Q3/Q4 |
| Folders | `src/folders.rs` replaces `src/free.rs`. `folders.json` in the state dir holds `[{id, dir, opened, created}]`: the project roots and plain folders the user opened. The first load migrates `free.json` (each session becomes a folder; the file is renamed `.migrated`). A folder's `Entry` uses `project` = the git project name, or the folder name for a plain folder |
| Pane env | the JW_* vars of `actions::jw_env` (a plain folder gets JW_ID, JW_NAME, JW_PROJECT) + `JW_PANE_ID` |
| Theme | `src/theme.rs`: One Dark / One Light `Palette`s that every draw module (and the syntect theme) asks for through `theme::p()`. Pick: `JW_THEME=dark\|light` pins one (OPEN-5); otherwise macOS's `AppleInterfaceStyle`, re-read every 3 s so a switch repaints; dark elsewhere. Querying the terminal (OSC 11, mode 2031) is left for later |
| Workspaces list | `App::reload` builds the rows. Each opened folder becomes a project row, and each registry worktree goes under its project's row, the project matched by `Repo::open(worktree).root` (`connectors/git.rs:56`). A project that has worktrees but no folder entry still gets a row, closed (`○`); opening it adds it to `folders.json` |
| Project root opening | `Stream::resolve` (`stream.rs:51`) for a folder: git with a jw config → `cfg.layout.tree()`; git without a config, or a plain folder → one shell. Env `JW_ID`, `JW_NAME`, `JW_PROJECT`; no slot and no ports |
| Marks | `PaneInfo` (`proto/mod.rs`) gains `busy: bool` (output in the last 2 s) and `bell: bool`. In the daemon, a `vt100::Callbacks::audible_bell` (and OSC 9) sets `bell`, and an `Input` to that pane clears it. While a workspace is open the client sends `List` every 2 s. PR state: `gh pr list` per worktree branch in the background every 60 s, cached in `App::prs` |
| Viewer panes | A leaf whose role starts with `view:` (`view:diff`, `view:md:<path>`) has no PTY. The daemon gives it an id and keeps it in the tree and `session.json`, nothing more (`Split` with such a role skips `spawn`; `Kill` finds its workspace by tree). The client holds the content in `App::views: BTreeMap<PaneId, View>`, rebuilt from the role when it attaches (a diff is reloaded, a `.md` is re-read) |
| Diff pane | Opens on the right at 0.36 of the stage's width (`Split{dir: Right}` beside the root's right edge: the rightmost top leaf). Unified below 110 columns, side by side at 110 or more; `t` overrides this. The file list shows at 100 columns or more. `↵` on a file: `.md` goes to the reader pane, anything else to nvim |
| Open in nvim | If the workspace has an `editor` pane: send `\x1b:e <path>\r` (RISK-16). If not: `Split{Right}` with `role: "editor"`, `cmd: "nvim <path>"`. `.md` files go to a `view:md:<path>` pane (one per workspace, reused) |
| Rename | `git worktree move <old> <new>` + `git branch -m <old> <new>` (the new branch from the `branch` template) + the registry `name`, `path` and `branch` (same `id`). Then `Close` and `Open` of the workspace, after asking if an agent or editor runs |
| Status | `App::status: Option<(String, Tone)>`, `Tone::{Info, Done, Error}`; `Job::Failed` and refusals are `Error` |
| Stack | ratatui (its crossterm re-export), portable-pty, vt100 0.16 (drawn by `src/tui/draw.rs`), serde/toml/serde_json, syntect, pulldown-cmark, anyhow |
| Diff | `src/diff.rs` (model: `git diff -M --merge-base origin/<base>` + untracked) and `src/tui/diffview.rs` (drawing, keys `j k ] [ v t ␣ b ↵ q`) |
| MD reader | `src/view/md.rs` (pulldown-cmark → wrapped lines + headings) and `src/tui/mdview.rs` |
| Pickers | `src/tui/finder.rs`: a `Finder` modal (query, results, selection, preview) with one fuzzy scorer (consecutive matches score more), three sources. `o`: folder browser, one directory at a time, `→`/`tab` in, `←`/backspace on empty up, `~` home, `↵` opens; a name that doesn't exist offers `+ create`. It starts in the parent of the current project. `␣`: every workspace, by last use (`recent.json` in the state dir), with a preview of path, branch, PR, agent state and panes. `/`: `git ls-files -co --exclude-standard`, or a walk capped at 5000 files without dot dirs |

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
- REQ-33 *(Replaced by REQ-50.)* THE sidebar SHALL list projects with their worktrees indented, numbered 1–9, marked `●` open / `○` closed, `✻` agent working, `?` agent waiting, `⚡` dev running, `⚑` PR merged, and scroll when longer than the screen.
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
- REQ-50 THE sidebar SHALL show one numbered row per workspace, with project rows unindented and their worktrees indented under them with `↳`, no headings, and the legend on its last line.
- REQ-51 THE sidebar SHALL mark a workspace `✻` WHILE its agent pane has printed in the last 2 s, `?` WHEN its agent pane rang the bell since its last input, `⚡` WHILE a `dev:*` pane runs, and `⚑` WHERE its PR is merged.
- REQ-52 THE header SHALL show name, branch, base, PR and path as in Screen.
- REQ-53 THE pane title SHALL show the name, or the role and what runs.
- REQ-54 THE status bar SHALL show the mode chip of Screen and the last message, coloured by its tone, for 4 s.
- REQ-55 WHEN a project root or plain folder is opened, the TUI SHALL start its panes as Interfaces → Project root opening says.
- REQ-56 WHEN jw starts and finds `free.json`, it SHALL move its sessions into `folders.json`.
- REQ-57 WHEN `d` runs, the TUI SHALL open the diff as a pane on the right. WHEN `↵` is pressed on a file in it, the TUI SHALL open that file as REQ-42 says.
- REQ-58 IF the pane closed with `x` is a viewer, THEN the TUI SHALL close it without asking.
- REQ-59 WHEN the user clicks a sidebar row or a pane, the TUI SHALL open that workspace or focus that pane; WHERE the pane's program turned the mouse on, the TUI SHALL pass it every click, drag and wheel in its encoding; otherwise the wheel SHALL scroll (arrows in a full-screen program, the history in a shell). Shift + drag stays the terminal's own selection.
- REQ-60 WHEN the user drags the line between two panes, the TUI SHALL resize them as it moves, and the daemon SHALL keep the new share in the tree and `session.json`.
- REQ-19 WHEN the workspace, a modal or the pane focus changes, the TUI MAY animate the transition, never delaying input to the panes. *(Later; optional.)*
- REQ-20 WHERE `[tui] animations = false`, the TUI SHALL apply every change without animation.

### Addendum 3: sessions, settings, and the risks closed

| Area | What it does |
|---|---|
| Session | A name (`[a-z0-9-]`, ≤ 24) with its own workspaces (folders and worktrees), `session.json` and `recent.json`, under `~/.local/state/jw/sessions/<name>/`. One daemon serves every session; workspace ids are `<session>/<id>` |
| `jw new <name> [--dir D]` | Creates the session and attaches. It starts with one workspace (D or the cwd) holding one shell, never a layout. If the name exists, exit 1 |
| `jw [name]` | Attaches to `name` or the last session used; with none, creates `main` |
| `jw sessions` | Name, workspaces open, agents working/waiting |
| In a session | The sidebar title is the session name; only its workspaces show. `o` and `w` add to it. Later projects use their `[layout]` |
| `^␣ a` | Session picker: fuzzy, `↵` switches, a new name offers `+ create` in the current folder |
| Agent CLI | `jw ls [--json]`, `jw read <ws> [--pane role] [--lines N]`, `jw worktree <name> [--in project] [--task …]` (was `jw new --task`), `jw prompt <ws> …`, on `$JW_SESSION` |
| Migration | The first session-aware run moves `folders.json`, `session.json` and every registry worktree into `main` |
| Registry | `workspaces.json`, seeded once from a copy of `registry.json`; entries gain `session` and `root`. Go's file is never written |
| Settings | `~/.config/jw/settings.json`: `leader`, `theme` (`system\|dark\|light`), `dark`, `light`, `which_delay_ms`, `keys{action: key}`; all optional; env `JW_LEADER`/`JW_THEME` win; re-read on mtime change. JSON, so Go's `*.toml` glob skips it |
| `^␣ ,` | Settings screen: general, keys, themes. Rebind swaps a key in use; `1-9 hjkl HJKL ? q` fixed; no Ctrl/Alt after the leader; the leader is Ctrl + a key. Themes: `↵` use, `e` edit in nvim, `c` copy. Writes at once |
| Themes | `one-dark`, `one-light` built in; `~/.config/jw/themes/<name>.json` = `{name, dark, colors{bg panel line fg dim sel blue green yellow red magenta cyan, add_bg? del_bg? add_word? del_word?}}`; missing diff colours are mixed; a bad file keeps the last good palette |
| `.md` reuse | `ClientMsg::Role{pane, role}` retargets the workspace's `view:md:*` pane |
| Scrollback | 2 MiB raw ring per pane in the daemon, sent in `Snapshot`, saved to `sessions/<name>/scrollback/<pane>.bin`, replayed on restore under `── restored <time> ──` |
| Agent state | `JW_PANE`, `JW_SESSION` in panes; claude starts with `--session-id <uuid>` and `--settings` hooks running `jw hook <event>` (`UserPromptSubmit` working, `Notification` waiting, `Stop` idle) → `ClientMsg::Agent` → `PaneInfo.agent`; restore runs `claude --resume <uuid>` |
| nvim | `--listen <state>/nvim/<pane>.sock`; files open with `nvim --server <sock> --remote <path>`, keys as fallback |
| Frames | u32 length + u8 kind (0 JSON, 1 Output = u64 pane + bytes); per-client queue bounded at 8 MiB, then dropped and resynced by `Snapshot` under 1 MiB. `PROTOCOL = 5` |

- REQ-61 WHEN `jw new <name>` runs, jw SHALL create the session and open it with one workspace in the cwd, holding one shell.
- REQ-62 THE sidebar SHALL show only the current session's workspaces, under the session's name.
- REQ-63 WHEN `^␣ a` picks another session, the TUI SHALL show that session without stopping anything in the one it left.
- REQ-64 WHEN `jw` runs with no name, it SHALL attach to the last session used. WHERE no session exists, it SHALL create `main`.
- REQ-65 WHEN the first session-aware jw starts, it SHALL move today's folders, trees and worktrees into `main`.
- REQ-66 `jw ls --json` SHALL list `$JW_SESSION`'s workspaces with state, marks, branch and PR. `jw read` SHALL print a pane's last N lines.
- REQ-67 WHEN `^␣ ,` is pressed, the TUI SHALL open the settings screen, and each change SHALL be written to `settings.json` at once.
- REQ-68 WHEN a rebind uses a key bound to another action, the TUI SHALL swap them. IF the key is fixed, or has Ctrl or Alt, THEN the TUI SHALL refuse it.
- REQ-69 WHEN `settings.json` or a theme file changes, the TUI SHALL apply it within 2 s. IF it does not parse, THEN the TUI SHALL keep the last good values and show the error.
- REQ-70 THE Rust jw SHALL use `workspaces.json` only, seeded from `registry.json` when it is missing.
- REQ-71 WHEN a `.md` opens and a reader pane exists, the TUI SHALL show it in that pane.
- REQ-72 WHEN a client attaches, a pane's scrollback SHALL include up to 2 MiB from before the attach. WHEN the daemon restarts, it SHALL replay the saved scrollback.
- REQ-73 WHILE a claude pane's hooks report a state, the sidebar SHALL show it (`✻` working, `?` waiting).
- REQ-74 WHEN the daemon restores an agent pane, it SHALL resume that pane's own session id.
- REQ-75 WHEN a file opens in an nvim pane with a socket, the TUI SHALL use `nvim --server`.
- REQ-76 IF a client's pending output passes 8 MiB, THEN the daemon SHALL drop it and resync that client, without blocking other clients or the PTY.

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
- RISK-17 **Mapping worktrees to their project** with `Repo::open` runs one `git` per worktree on every reload. Cache it by path; it changes only when worktrees are created or removed.
- RISK-18 **The `?` mark depends on the agent ringing the bell.** Claude Code does so only when its notifications use the terminal bell. Without that, `?` never shows (the same worst case as RISK-7).
- RISK-19 **`gh` polling** costs a network call per worktree a minute. Only open worktrees are polled, and only while jw runs.
- RISK-20 **`claude --settings` hooks** may replace the user's hooks instead of merging. Then they go to `~/.claude/settings.json`, guarded by `[ -n "$JW_PANE" ]`. S5 checks first.
- RISK-21 **A replayed ring at a new width** wraps differently; fine for scrollback.
- RISK-22 **A folder open in two sessions** is two workspaces sharing nothing. A worktree belongs to the session that made it.
- RISK-23 **`workspaces.json` drifts from Go's registry** after the seed; `jw import` if it is ever needed.
- RISK-24 **`jw new` changes meaning** (worktree → session). `jw new --task` errors with a pointer to `jw worktree`.
- Closed by addendum 3: RISK-5 (own registry), RISK-7 (hooks), RISK-9 (frames), RISK-16 (`nvim --server`), RISK-17 (`root` in the registry).

## Parts (each one ends with `cargo test` + `clippy` green and a local commit on `feat/rust-tui`; nothing is pushed)

P0–P9 were built against v1: core, connectors, daemon, layout, the first TUI, actions, free space, diff and Markdown viewers.

| Part | What | REQs | Main files |
|---|---|---|---|
| Q1 ✓ `4fdcd14` | One-shot leader + which-key; actions on the current workspace; drop NAV and the sidebar cursor; `X` merges done/rm; status bar | 30–32, 39 | `tui/mod.rs`, `tui/keys.rs`, `tui/draw.rs`, `tui/modal.rs` |
| Q2 ✓ `cdbc651` | The daemon owns the tree: layout ops, the new messages, version check, `session.json`; `t x HJKL f n`; last pane closes the workspace; OSC titles | 5, 34–36 | `layout.rs`, `proto/mod.rs`, `daemon/mod.rs`, `stream.rs`, `tui/mod.rs` |
| Q7 (done) | Themes: One Dark / One Light and the prototype's look (`33c46f5`) | 43 | `theme.rs`, `tui/draw.rs` |
| Q9 ✓ `0e869ca` | This addendum: Screen, Interfaces rows, REQ-50…58 | — | docs |
| Q3 ✓ `58c2301` | Folders + migration; project rows; the new sidebar (rows, indent, legend, title); header; pane titles; status tones and the mode chip; `o` folder browser; the empty stage | 37, 50, 52–56 | `folders.rs`, `stream.rs`, `tui/mod.rs`, `tui/draw.rs`, `tui/finder.rs` |
| Q4 ✓ `db90220` | `␣` switcher with recency and preview, `tab` from the same recency, `/` file picker; the which-key additions | 41, 42 | `tui/finder.rs`, `tui/mod.rs` |
| Q5 ✓ `6ad1d6a` | Viewer panes: `view:diff` / `view:md`, the daemon's PTY-less leaves, the diff's width rules, `↵` to nvim/md, `x` on a viewer | 21, 22, 42, 57, 58 | `daemon/mod.rs`, `tui/diffview.rs`, `tui/mdview.rs`, `tui/mod.rs` |
| Q6 ✓ `ea09023` | Marks: `busy`/`bell` in the daemon, the 2 s `List`, PR polling, `⚑` and the header's PR | 51, 52 | `daemon/mod.rs`, `proto/mod.rs`, `tui/mod.rs` |
| Q10 ✓ `afaf4a2` | Rename `r` | 40 | `connectors/git.rs`, `actions.rs`, `core/registry.rs` |
| Q8 ✓ `765a7af` | Restore from `session.json` when the daemon starts | 15 | `daemon/mod.rs` |
| — | **Checkpoint:** a week of daily use with Claude Code and nvim (RISK-1), once Q3–Q5 make it look and act like the prototype | | |
| S0 | Addendum 3 in this spec; prototype v7 (sessions) before the UI parts | — | docs |
| S1 | `workspaces.json` with `session` and `root` | 70 | `core/registry.rs`, `tui/mod.rs` |
| S2 | `.md` reuse, `ClientMsg::Role` | 71 | `proto`, `daemon`, `tui/mod.rs` |
| S3 | Binary Output frames, bounded queue, `PROTOCOL 5` | 76 | `proto`, `daemon`, `client.rs` |
| S4 | Scrollback ring: `Snapshot`, saved, replayed | 72 | `daemon`, `tui/mod.rs` |
| S5 | Claude session ids, hooks, `jw hook`, `ClientMsg::Agent` | 73, 74 | `stream.rs`, `daemon`, `cli.rs`, `tui/mod.rs` |
| S6 | nvim `--listen` / `--server --remote` | 75 | `stream.rs`, `tui/mod.rs` |
| S7 | Sessions: storage, daemon, `jw new/[name]/sessions`, migration, title, `^␣ a` | 61–65 | `daemon`, `proto`, `folders.rs`, `cli.rs`, `tui/**` |
| S8 | `settings.rs`, JSON themes, reload, keymap by action | 68, 69 | `settings.rs`, `theme.rs`, `tui/**` |
| S9 | Settings screen `^␣ ,` | 67, 68 | `tui/settings_view.rs`, `tui/draw.rs` |
| S10 | `jw ls`, `jw read`, `jw worktree`; the jw skill rewritten | 66 | `cli.rs`, `main.rs`, skill |
| Later | Animations (optional); cutover: delete Go + herdr, README, trim the `jw` skill | 16, 19, 20 | |

Reuse: `focus_towards` (`tui/mod.rs`) becomes `layout::neighbour`; `actions::{new_stream, rm_plan, rm, sync, done_plan}`, `src/diff.rs`, `src/view/md.rs` and `Modal::Rm` (type the name) stay as they are.

## Decided (were open questions)

- OPEN-1 → `Ctrl-Space`, configurable.
- OPEN-2 → `docs/specs/rust-tui.md`.
- OPEN-3 → keep `jw prompt` and `jw new --task` as socket clients.
- OPEN-4 → unix only; `shell_windows.go` is not ported.
- OPEN-6 → TUI, not a GUI (RAT-10).
- OPEN-5 → `~/.config/jw/settings.json` (JSON, outside Go's `*.toml` glob) and the `^␣ ,` screen (addendum 3).

## Corrections

- The first proposal kept the CLI underneath the TUI; the user chose TUI only.
- The first proposal assumed the TUI would be a client of herdr; the user asked for it to replace herdr.
- v1 put tui-term in the stack; P5 draws vt100 screens itself.
- RISK-2 assumed `[layout]` was a new table. It already existed with `editor`.
- v1's "spaces → streams" with a free space first, a sticky NAV mode (REQ-17), `F` focus-all (REQ-25) and a Telescope finder with `> @ / :` prefixes (REQ-26/27) were replaced by v2's workspaces, one-shot leader and three plain pickers.
- `33c46f5` kept project headings and a "free" section in the sidebar; the prototype has neither. REQ-50 replaces REQ-33.
- v1 said "TUI only, no CLI; the jw skill becomes obsolete". An agent overseeing a session needs the CLI, so it stays as socket clients.
- RISK-17 said one `git` ran per worktree per reload; `root_of` already cached it per project in memory. S1 makes it persistent.
- An addendum-3 draft read `.jw/` as a per-project config folder (like `.vscode/`), then as a pinned "home" brain workspace. The user meant a scope for which projects show: named sessions. The brain is a usage pattern, not a jw concept.
- The 2026-10-09 prototype's `.worktrees/<name>` on `jw/<name>` was wrong for this repo: worktrees live at `<repo>-wt/<name>` on the `branch` template (`feat/{name}`).

## Tests

- core: parse config/registry from Go fixtures; registry.json round-trip read back by Go.
- layout: tree → rects for several sizes; `insert`/`remove`/`swap`/`neighbour` as tables.
- daemon (tempdir, `tests/`): `cat` pane, detach, kill the client, reattach, same screen; `Split`/`Kill`/`Swap`/`Name` produce `Tree` and update `session.json`; `printf '\e]2;hi\a'` arrives as `Title`; a version mismatch is an error.
- actions: fakes behind traits; `X` takes the done or rm path by PR state; rename against a temp repo; `free.json` → `folders.json` migration.
- finder: fuzzy ranking tables; folder browser over a temp dir.
- TUI: ratatui `TestBackend` for the which-key popup, sidebar and modals.
- manual (after Q2 and Q5): claude and nvim in panes; `w`, `t`, `x`, `HJKL`, `n`, `d`, `↵` in the diff, `q`, then `jw` again shows the same layout.
