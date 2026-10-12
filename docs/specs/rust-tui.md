---
status:      agreed (v2 + the screen addendum + addendum 3 "sessions", 2026-10-09; P0–P9 built against v1); addendum 12 (X0) drafted; addendum 17 (A1, A2) built; addendum 19 (V0–V4) built
scope:       [Cargo.toml, src/**, .github/workflows/ci.yml, internal/core/config/config.go (one relaxation), README.md]
depends_on:  [git, gh on PATH]
supersedes:  [herdr backend: internal/backends/terminal, internal/connectors/herdr]
---
# jw in Rust: a TUI that is its own multiplexer (replaces herdr)

## Contract

- One binary `jw`, unix only (macOS + Linux). With no arguments it opens the TUI client; a hidden `jw daemon` runs the server. The other commands are socket clients for sessions and for agents (addendum 3): `jw new <session>`, `jw [session]`, `jw sessions`, `jw ls`, `jw read`, `jw pane`, `jw worktree`, `jw prompt`, `jw hook`, plus `jw help` and `jw skill [install]`.
- **Two concepts.** A *workspace* is an open folder: a project (any folder, git or not) or one of its worktrees. A workspace holds *panes*.
- **One-shot leader.** `Ctrl-Space`, then one key, then back to the terminal (tmux-style). A pause of 150 ms after the leader shows every key (600 ms before addendum 13); `?` shows them at once. The one exception is an arrow after the leader: arrows keep moving through the workspaces until another key gives the keyboard back (addendum 7).
- **Every action applies to the current workspace or the focused pane.** The sidebar is a list to read and click, numbered for `1–9`, and walked with `^␣` + arrows.
- **The daemon** owns every terminal (one PTY per pane, a VT parser with screen and scrollback) and each workspace's pane tree (splits, ratios, names). It survives the client and writes `session.json` on every change.
- **The client** draws the sidebar (projects with their worktrees indented), the current workspace's panes, a header (workspace · branch · PR) and a status bar. It owns only focus and the full view.
- **Viewers are panes the client draws:** a GitHub-style diff in three views, General, Uncommitted and Commits (unified when narrow, side by side when wide, sticky file headers, viewed, changed words) and a Markdown reader. Code files open in nvim; there is no built-in editor or file tree.
- **Theme:** Atom One Dark / One Light, following the OS or the settings. Programs in panes are told it: the daemon answers their colour queries and tells the ones that subscribed (mode 2031) when it switches (S14).
- The checks of today's commands (`new rm done sync setup dev info init`) stay; confirmations are modals. Exit code 3 and `--json` go away.
- On-disk compatibility: same config TOML as Go; Rust keeps its own `workspaces.json` (addendum 3). Alt is never bound (AeroSpace owns it).

### Keymap (after `^␣`)

| Go | Worktree | Panes |
|---|---|---|
| `␣` switch workspace (fuzzy, most recent first, preview) | `w` new worktree from the current branch: asks its name (the next free `ws-N` filled in), branch from the `branch` template, setup runs | `hjkl` focus |
| `tab` previous workspace | `r` rename: name, branch and folder; its panes keep running | `t` new shell beside the focused pane |
| `1–9` the sidebar's number | `s` sync (on a project root: `pull --ff-only`) | `x` close the pane; on the last one, close the workspace |
| `o` open a folder (browser: arrows, type to filter, `~`, a new name creates it) | `d` diff pane against the base, on a worktree or the project root (addendum 11) | `n` name the pane (empty = automatic title) |
| `/` open a file (`.md` → reader pane, else nvim) | `X` remove: done checks if the PR is merged, rm checks otherwise | `f` full: the focused pane fills the panes area, the sidebar and bars stay · `HJKL` swap with the neighbour · `p` save the panes as the project's layout (addendum 10) · `v` select text (addendum 12) |
| `↑↓` next/previous workspace, `←→` next/previous project; more arrows keep moving | `R` review a pull request: pick an open PR, get a worktree with its diff and a briefed agent (addendum 15) | |
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
| Status bar | Mode chip: `TERM` green, `^␣` yellow (leader pending), `DIFF`/`MD` blue (a viewer is focused), `CONFIRM` red (a yes/no modal), `MOVE` blue (arrows walk the sidebar). Then the hint for that mode. A message goes right-aligned: green for done, red for an error, cleared after 4 s |
| Which-key | Three columns as now, adding `␣ switch`, `o open`, `/ file` (go) and `r rename` (worktree) |
| Empty stage | `^␣ o opens a folder · ^␣ ␣ switches workspace`, centred, dim |

## Interfaces

| Piece | Where / shape |
|---|---|
| Crate | `Cargo.toml` next to `go.mod`; library `src/lib.rs` with `src/{core,connectors,daemon,proto,tui,view}` (driven by `tests/`) and a thin `src/main.rs` |
| core | port of `internal/core/config` and `registry` with serde, `deny_unknown_fields`. Worktrees at `root/<name>` (default `<repo>-wt`), branch template default `feat/{name}` (`src/core/config.rs`) |
| connectors | git (1:1 with git.go, plus `worktree_move` and `branch_rename`), gh (`gh pr list`), shell/setup/ports |
| Socket | `$XDG_RUNTIME_DIR/jw/jw.sock`, else `~/.local/state/jw/jw.sock` |
| Protocol | `src/proto`: u32 length + serde_json frames; a workspace is named by its registry id in `stream`. The client opens with `Hello{protocol}` and refuses a daemon that answers anything but the same `PROTOCOL` (RISK-14). C→D: `Hello`, `Attach{stream}`, `Detach`, `Input{pane,bytes}`, `Resize{pane,cols,rows}`, `Open{stream,tree}` (a `Tree<NewPane>`), `Split{pane,dir,new}`, `Kill{pane}` (removes the leaf), `Close{stream}`, `Swap{a,b}`, `Name{pane,name?}`, `Theme{dark,fg,bg}` (S14), `List`, `Prompt{stream,text,pane?}`, and the older `Spawn` (one pane, placed on the right). D→C: `Hello`, `Tree{stream,tree}` (on attach and after every change), `Snapshot{pane,…}`, `Output{pane,bytes}`, `Title{pane,title}` (OSC 0/2 via `vt100::Callbacks`), `Exited{pane,status}`, `Panes{…}` (with foreground process), `Spawned{pane}`, `Prompted{pane}`, `Error{msg}`. Pane ids are `u64` |
| Tree | `src/layout.rs`: leaves carry `{id, run, name?}`; `run = "shell"\|"agent"\|"editor"\|"dev:<svc>"\|"view:diff"\|"view:md:<path>"\|"<cmd>"`; `view:*` leaves have no PTY. Ops: `insert(beside, dir, leaf)`, `remove(id)` (the sibling takes the space), `swap(a,b)`, `neighbour(id,dx,dy)` (nearest rect that overlaps on the other axis), `rects` |
| Layout config | `[layout]` in the project TOML as in v1 (tree of `split`, `ratio`, `a`/`b`, leaves `run`). It is the starting tree when a workspace opens; without it, one shell (addendum 10). Go ignores the tree and `[tui]` (`rustOnly`, config.go) |
| Session | `session.json` in the state dir (next to the socket for any socket but the default one, so tests never touch it), written by the daemon with temp + rename after every tree change: `{workspaces: [{id, tree}]}`, each leaf with role, name, cmd, cwd and env. Recency and recent folders arrive with Q3/Q4 |
| Folders | `src/folders.rs` replaces `src/free.rs`. `folders.json` in the state dir holds `[{id, dir, opened, created}]`: the project roots and plain folders the user opened. The first load migrates `free.json` (each session becomes a folder; the file is renamed `.migrated`). A folder's `Entry` uses `project` = the git project name, or the folder name for a plain folder |
| Pane env | the JW_* vars of `actions::jw_env` (a plain folder gets JW_ID, JW_NAME, JW_PROJECT) + `JW_PANE_ID` |
| Theme | `src/theme.rs`: One Dark / One Light `Palette`s that every draw module (and the syntect theme) asks for through `theme::p()`. Pick: `JW_THEME=dark\|light` pins one (OPEN-5); otherwise macOS's `AppleInterfaceStyle`, re-read every 3 s so a switch repaints; dark elsewhere. Panes (S14): on every tick the TUI sends `ClientMsg::Theme` (`theme::wire()`) when `{dark, fg, bg}` changed. The daemon keeps it in `session.json` and answers from `Titles` (`daemon/src/lib.rs`): OSC 10/11 `?` → `rgb:RRRR/GGGG/BBBB` + ST, `?996n` → `?997;1\|2n`, `?2031$p` → `?2031;1\|2$y`, `6n` → cursor, `c` → `?1;2c`. A pane that set `?2031h` gets `?997;1\|2n` on a dark↔light switch. `COLORFGBG` is `15;0` / `0;15` at spawn. Querying the outer terminal is left for later (OPEN-9) |
| Workspaces list | `App::reload` builds the rows. Each opened folder becomes a project row, and each registry worktree goes under its project's row, the project matched by `Repo::open(worktree).root` (`connectors/git.rs:56`). A project that has worktrees but no folder entry still gets a row, closed (`○`); opening it adds it to `folders.json`. Rows also rebuild when another process rewrites `workspaces.json` or `folders.json` (addendum 14) |
| Project root opening | `Stream::resolve` (`stream.rs:51`) for a folder: git with a jw config → `cfg.layout.tree()`; git without a config, or a plain folder → one shell. Env `JW_ID`, `JW_NAME`, `JW_PROJECT`; no slot and no ports |
| Marks | `PaneInfo` (`proto/mod.rs`) gains `busy: bool` (output in the last 2 s) and `bell: bool`. In the daemon, a `vt100::Callbacks::audible_bell` (and OSC 9) sets `bell`, and an `Input` to that pane clears it. While a workspace is open the client sends `List` every 2 s. PR state: `gh pr list` per worktree branch in the background every 60 s, cached in `App::prs` |
| Viewer panes | A leaf whose role starts with `view:` (`view:diff`, `view:md:<path>`) has no PTY. The daemon gives it an id and keeps it in the tree and `session.json`, nothing more (`Split` with such a role skips `spawn`; `Kill` finds its workspace by tree). The client holds the content in `App::views: BTreeMap<PaneId, View>`, rebuilt from the role when it attaches (a diff is reloaded, a `.md` is re-read) |
| Diff pane | Opens on the right at 0.36 of the stage's width (`Split{dir: Right}` beside the root's right edge: the rightmost top leaf). Unified below 110 columns, side by side at 110 or more; `t` overrides this. The file list shows at 100 columns or more. `↵` on a file: `.md` goes to the reader pane, anything else to nvim |
| Open in nvim | If the workspace has an `editor` pane: send `\x1b:e <path>\r` (RISK-16). If not: `Split{Right}` with `role: "editor"`, `cmd: "nvim <path>"`. `.md` files go to a `view:md:<path>` pane (one per workspace, reused) |
| Rename | `git worktree move <old> <new>` + `git branch -m <old> <new>` (the new branch from the `branch` template) + the registry `name`, `path` and `branch` (same `id`). Then `Close` and `Open` of the workspace, after asking if an agent or editor runs |
| Status | `App::status: Option<(String, Tone)>`, `Tone::{Info, Done, Error}`; `Job::Failed` and refusals are `Error` |
| Stack | ratatui (its crossterm re-export), portable-pty, vt100 0.16 (drawn by `src/tui/draw.rs`), serde/toml/serde_json, syntect, pulldown-cmark, anyhow |
| Diff | `src/diff.rs` (model: one `Scope` per view, addendum 19; before it, `git diff -M --merge-base origin/<base>` + untracked) and `src/tui/diffview.rs` (drawing, keys `j k ] [ v t ␣ b ↵ q`) |
| MD reader | `src/view/md.rs` (pulldown-cmark → wrapped lines + headings) and `src/tui/mdview.rs` |
| Keyboard protocol | Addendum 5. `jw_proto::kitty::Kitty` holds a pane's kitty keyboard flag stacks (main and alternate, 16 deep). The daemon's `Titles` and the TUI's `keys::Flags` feed it from `vt100::Callbacks::unhandled_csi`. In the daemon a `CSI ? u` becomes `Query::Kitty(flags)` in the same queue as S14's queries, so answers go in the order asked; `snapshot` appends `Kitty::replay` (`CSI < 17 u`, `CSI = base u`, then one `CSI > f u` per entry); a restored pane's history ends with `kitty::CLEAR`. The TUI probes the outer terminal (`supports_keyboard_enhancement`, before its event reader starts), pushes `DISAMBIGUATE_ESCAPE_CODES` after `ratatui::init()` and pops it before `ratatui::restore()`. `keys::encode(k, app_cursor, kitty)`: with flag 1, a modified Enter / Tab / Backspace goes as `CSI 13\|9\|127;mods u` and Esc as `CSI 27[;mods] u`; anything else, or flags 0, as before. No message changes, so `PROTOCOL` stays (RAT-15) |
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
- REQ-21 WHILE a diff pane is focused, the TUI SHALL show every file of the active view (addendum 19) in one scroll with `j/k` per file, `]`/`[` per hunk, `v` viewed, `t` unified ⇄ side by side, and changed words highlighted; side by side by default when the pane is wide.
- REQ-22 WHEN a `.md` file is opened, the TUI SHALL show it rendered and read-only in a reader pane: headings, lists, tables, code with highlighting, links.
- REQ-30 WHEN the user presses the leader and then one key, the TUI SHALL run that key's action and return to the terminal.
- REQ-31 WHILE the leader has been pending for `which_delay_ms` (150 ms by default, REQ-141), the TUI SHALL show every key, grouped go / worktree / panes.
- REQ-32 THE TUI SHALL forward every key except the leader to the focused pane, or to the viewer when the focused pane is a viewer.
- REQ-33 *(Replaced by REQ-50.)* THE sidebar SHALL list projects with their worktrees indented, numbered 1–9, marked `●` open / `○` closed, `✻` agent working, `?` agent waiting, `⚡` dev running, `⚑` PR merged, and scroll when longer than the screen.
- REQ-34 WHEN `t`, `x`, `HJKL` or `n` changes the panes, the daemon SHALL update the workspace's tree, send `Tree` to every attached client, and write `session.json`.
- REQ-35 WHEN the last pane of a workspace is closed, the TUI SHALL ask first, then close the workspace; a worktree stays listed as `○`, a project leaves the sidebar.
- REQ-36 WHERE a pane has a name, the TUI SHALL show it as the pane's title; otherwise the title SHALL be the program's OSC title, else its foreground process.
- REQ-37 WHEN the user picks a folder with `o`, the TUI SHALL open it as a workspace with its `[layout]` (or one shell for a plain folder); IF it is already open, THEN the TUI SHALL switch to it.
- REQ-38 *(Replaced by REQ-110.)* WHEN `w` runs, the TUI SHALL create the worktree `ws-N` (lowest free N) from the current workspace's branch, run setup, and switch to it.
- REQ-39 WHEN `X` runs on a worktree, the TUI SHALL apply the done checks if its PR is merged and the rm checks otherwise; IF the workspace is a project root, THEN the TUI SHALL refuse and say that jw never deletes a project folder.
- REQ-40 *(Replaced by REQ-111.)* WHEN `r` runs on a worktree, the TUI SHALL validate the name, then move the folder, rename the branch, update the registry, and restart the workspace's panes in the new folder (asking first if an agent or editor runs).
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
- REQ-59 WHEN the user clicks a sidebar row or a pane, the TUI SHALL open that workspace or focus that pane; WHERE the pane's program turned the mouse on, the TUI SHALL pass it every click, drag and wheel in its encoding; otherwise the wheel SHALL scroll (arrows in a full-screen program, the history in a shell). Selecting is REQ-124–125.
- REQ-60 WHEN the user drags the line between two panes, the TUI SHALL resize them as it moves, and the daemon SHALL keep the new share in the tree and `session.json`.
- REQ-19 WHEN the workspace, a modal or the pane focus changes, the TUI MAY animate the transition, never delaying input to the panes. *Superseded by addendum 13 (REQ-136–148).*
- REQ-20 WHERE `[tui] animations = false`, the TUI SHALL apply every change without animation. *Amended by REQ-138: the key is `animations` in `settings.json`.*

### Addendum 3: sessions, settings, and the risks closed

| Area | What it does |
|---|---|
| Session | A name (`[a-z0-9-]`, ≤ 24, not a command) with its own workspaces: `folders.json` and `recent.json` under `~/.local/state/jw/sessions/<name>/`, and the registry entries whose `session` is its name (`main` is empty). One daemon serves every session without knowing them: outside `main` a folder's id carries its session (`dir:<path>#<session>`), and worktree ids are unique (`src/session.rs`) |
| `jw new <name> [--dir D]` | Creates the session and attaches. It starts with one workspace (D or the cwd) holding one shell, never a layout. If the name exists, exit 1 |
| `jw [name]` | Attaches to `name` or the last session used; with none, creates `main` |
| `jw sessions` | Name, workspaces open, agents working/waiting |
| In a session | The sidebar title is the session name; only its workspaces show. `o` and `w` add to it. Later projects use their `[layout]` |
| `^␣ a` | Session picker: fuzzy, `↵` switches, a new name offers `+ create` in the current folder |
| Header git | The repo and branch of where the focused pane is (its foreground process's cwd, `PaneInfo.cwd`), read from `.git` every 2 s like a shell prompt: the repo's name shows when it isn't the workspace's own, and `from <base>` and the PR only for the workspace's own checkout. The path on the right is that cwd |
| Renames | Pane: `^␣ n`. Worktree: `^␣ r` (name, branch, folder). Folder: `^␣ r` sets the name jw shows (`name` in `folders.json`); the folder on disk stays. Session: `ctrl-r` in `^␣ a` moves `sessions/<old>/`, the registry entries and the recent list, `Rekey`s the running folder workspaces in the daemon, and records `old → new` in `sessions/renamed.json` so panes started with the old `$JW_SESSION` still find it |
| Agent CLI | `jw ls [--json]`, `jw read <ws> [--pane role] [--lines N]`, `jw worktree <name> [--in project] [--task …]` (was `jw new --task`), `jw prompt <ws> …`, on `$JW_SESSION` |
| Help and skill | `jw help [cmd]`, `-h`/`--help`, `jw <cmd> --help`: the table in `crates/jw/src/help.rs`. `jw skill` prints the skill built in from `crates/jw/skill/SKILL.md`; `jw skill install` writes it to `$CLAUDE_CONFIG_DIR` (else `~/.claude`)`/skills/jw/SKILL.md` |
| Migration | The first session-aware run moves `folders.json`, `session.json` and every registry worktree into `main` |
| Registry | `workspaces.json`, seeded once from a copy of `registry.json`; entries gain `session` and `root`. Go's file is never written |
| Settings | `~/.config/jw/settings.json`: `leader`, `theme` (`system\|dark\|light`), `dark`, `light`, `which_delay_ms`, `which_sticky`, `animations`, `welcomed`, `keys{action: key}`; all optional; env `JW_LEADER`/`JW_THEME` win; re-read on mtime change. JSON, so Go's `*.toml` glob skips it |
| `^␣ ,` | Settings screen: general, keys, themes. Rebind swaps a key in use; `1-9 hjkl HJKL ? q` fixed; no Ctrl/Alt after the leader; the leader is Ctrl + a key. Themes: `↵` use, `e` edit in nvim, `c` copy. Writes at once |
| Themes | Every theme is JSON in one format, `crates/tui/themes/schema.json` (S16). Ten built in, compiled in from `crates/tui/themes/*.json` (`theme::BUILTIN`); the user's in `~/.config/jw/themes/<name>.json` = `{$schema?, name, dark, colors{bg panel line fg dim sel blue green yellow red magenta cyan, add_bg? del_bg? add_word? del_word?}}`, and one named like a built-in replaces it; missing diff colours are mixed from green/red over bg; a file that doesn't parse keeps the last good palette. `jw theme ls\|install <file\|url\|-> [--name] [--force]\|export <name>\|use <name>` (`crates/jw/src/theme.rs`); a URL goes through `curl` |
| `.md` reuse | `ClientMsg::Role{pane, role}` retargets the workspace's `view:md:*` pane |
| Scrollback | 2 MiB raw ring per pane in the daemon, sent in `Snapshot`, saved to `sessions/<name>/scrollback/<pane>.bin`, replayed on restore under `── restored <time> ──` |
| Agent state | `JW_PANE`, `JW_SESSION` in panes; claude starts with `--session-id <uuid>` and `--settings` hooks running `jw hook <event>` (`UserPromptSubmit` and `PreToolUse` working, `Notification` waiting, `Stop` idle) → `ClientMsg::Agent` → `PaneInfo.agent`; restore runs `claude --resume <uuid>` |
| nvim | `--listen <state>/nvim/$JW_PANE_ID.sock` (expanded by the pane's shell); files open with `nvim --server <sock> --remote <path>`, keys as fallback |
| Frames | u32 length + u8 kind: 0 JSON, or 1 = u32 head length + JSON head + the message's bytes raw (Output, Snapshot, Input); per-client queue bounded at 8 MiB, then dropped and resynced by `Snapshot` under 1 MiB. `PROTOCOL = 5`; 6 adds `PaneInfo.cwd` and `Rekey` |

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
- REQ-77 WHEN `jw help`, `jw -h` or `jw --help` runs, jw SHALL print every command with its usage. WHEN `jw help <cmd>` or `jw <cmd> --help` runs, it SHALL print that command's usage.
- REQ-78 WHEN `jw skill install` runs, jw SHALL write the skill built into the binary to Claude's `skills/jw/SKILL.md`, replacing the copy there.
- REQ-79 WHEN the daemon gets SIGTERM, SIGINT or SIGHUP, it SHALL write `session.json` and every pane's scrollback, then exit.
- REQ-80 WHEN `jw server status` runs, jw SHALL say whether the daemon runs, with its pid and running panes. WHEN `jw server stop` runs, jw SHALL send the daemon SIGTERM and wait until it stops answering.

### Addendum 4 (S14): programs in panes follow the theme

- REQ-81 WHEN a pane's program writes `OSC 10 ?` or `OSC 11 ?`, the daemon SHALL answer on that PTY with the current theme's fg or bg, whether or not a client is attached.
- REQ-82 WHEN a pane's program writes `CSI ? 996 n`, the daemon SHALL answer `CSI ? 997 ; 1 n` while dark and `CSI ? 997 ; 2 n` while light.
- REQ-83 WHEN the theme switches between dark and light, the daemon SHALL write the REQ-82 report to every pane whose program set mode 2031 and has not reset it.
- REQ-84 WHEN a pane's program writes `CSI ? 2031 $ p`, the daemon SHALL answer `CSI ? 2031 ; 1 $ y` if the pane has set the mode, and `; 2 $ y` if not.
- REQ-85 WHEN the TUI connects, and WHEN its dark mode or its palette's fg or bg changes, the TUI SHALL send `ClientMsg::Theme` (within a tick, 500 ms).
- REQ-86 THE daemon SHALL save the last theme in `session.json` and use it after a restart. IF it has never received one, THEN it SHALL use One Dark.
- REQ-87 WHEN the daemon replays a restored pane's history, it SHALL answer none of the queries in it, and SHALL start the pane with mode 2031 reset.
- REQ-88 THE daemon SHALL start each pane with `COLORFGBG` set to `15;0` when dark and `0;15` when light.
- REQ-89 WHEN a pane's program writes `CSI 6 n`, the daemon SHALL answer with the vt100 cursor position. WHEN it writes `CSI c` or `CSI 0 c`, the daemon SHALL answer `CSI ? 1 ; 2 c`.

### Addendum 5 (S15): modified keys reach the panes (kitty keyboard protocol)

Shift+Enter inserts a new line in claude, codex and nvim inside jw, as it does outside. Three links were missing: the TUI never asked the outer terminal for the kitty protocol, so Ghostty sent `\r` for both keys; nothing tracked or answered a pane program's `CSI > f u` / `CSI ? u`; and `encode` dropped Enter's modifiers. The flags reach clients in-band, as DECCKM does (RAT-15).

- REQ-90 WHERE the outer terminal supports the kitty keyboard protocol, the TUI SHALL push flag 1 at start and pop it before it restores the terminal.
- REQ-91 IF the outer terminal does not support the protocol, THEN the TUI SHALL send it nothing new, and keys SHALL behave as before.
- REQ-92 WHEN a pane's program sends `CSI > f u`, `CSI < n u` or `CSI = f ; m u`, the daemon and the TUI SHALL update that pane's stack for the screen in use (main or alternate).
- REQ-93 WHEN a pane's program sends `CSI ? u`, the daemon SHALL write `CSI ? f u` to its PTY, where f is the top of the active stack (0 if the stack is empty).
- REQ-94 WHEN a pane enters the alternate screen, its alternate stack SHALL start empty.
- REQ-95 WHILE the focused pane's active stack has flag 1, the TUI SHALL encode keys per the Keyboard protocol row in Interfaces.
- REQ-96 WHILE the focused pane's active flags are 0, the TUI SHALL send exactly the bytes it sent before (Shift+Enter is `\r`).
- REQ-97 WHEN a client attaches, the Snapshot SHALL leave its parser with the pane's whole active stack, so that a later pop gives the same flags as in the daemon, even after the ring dropped the original pushes.
- REQ-98 WHEN the daemon restores a pane from scrollback, the new process SHALL start with empty stacks in the daemon and in every client.
- REQ-99 THE daemon SHALL never write a reply while it holds a pane's state lock.
- REQ-100 WHEN a pane's program sends several queries, the daemon SHALL answer them in the order asked (nvim sends `CSI ? u` then `CSI c`, and takes the kitty answer only if it comes before REQ-89's).

### Addendum 6 (S16): themes are JSON, built in or installed

- REQ-101 THE TUI SHALL take every built-in palette from the JSON files compiled in from `crates/tui/themes/`, and SHALL have no palette constants in Rust.
- REQ-102 WHEN `cargo test` runs, every built-in SHALL parse, and its `name` SHALL equal its file stem.
- REQ-103 WHEN `~/.config/jw/themes/<name>.json` exists, the TUI SHALL use it instead of a built-in of the same name, and `names()` SHALL list that name once.
- REQ-104 THE schema SHALL list exactly the fields `ThemeFile`/`Colors` accept, with the same required set. A test SHALL compare the two.
- REQ-105 WHEN `jw theme install <file|url|->` runs on a valid theme, jw SHALL write it to `themes/<name>.json`. IF it does not parse, has an invalid name, or exists without `--force`, THEN jw SHALL write nothing and exit non-zero with the reason.
- REQ-106 WHEN `jw theme export <name>` runs, jw SHALL print the theme as JSON that `install` accepts back unchanged.
- REQ-107 WHEN `jw theme use <name>` runs, jw SHALL set the dark or the light setting by the theme's `dark` flag.
- REQ-108 WHEN `jw theme ls` runs, jw SHALL print each name with `built-in`/`file`/`file, replaces built-in`, `dark`/`light`, and which slot it is in.
- REQ-109 The settings screen SHALL list the ten built-ins, and its `c` SHALL write `export`'s JSON.

### Addendum 7: worktree names and moving with arrows

Prototype: https://claude.ai/artifact/QdLzKo3onBQFzeRvDrnh6g (approved 2026-10-10).

| Area | What it does |
|---|---|
| `^␣ w` | A `New worktree in <project>` modal: a name input holding the next free `ws-N`, selected, so `↵` takes it and the first key typed replaces it. Under it the branch, `from` and the folder, or in red why the name can't be (`actions::new_plan`: a valid name, free in the project, a folder that doesn't exist). `esc` creates nothing |
| `^␣ r` | The rename runs with the panes alive: `git worktree move`, the branch, the registry and `.jw.env` as before, then `ClientMsg::Moved{stream, from, to, env}` so the daemon's `session.json` starts them in the new folder after a restart. The TUI swaps its `Stream` for the renamed entry without touching the panes. The modal says the panes keep running and that a program that kept the old path may need a restart |
| `^␣` + arrow | Moves to the next (`↓`) or previous (`↑`) open workspace in the sidebar's order, or to the first open row of the next (`→`) or previous (`←`) project, and opens it. The status bar shows `MOVE`. More arrows keep moving with no leader. `↵` or `esc` ends it; any other key ends it and goes on as if typed outside it. Closed rows are skipped: walking past them would start their panes |
| Protocol | `PROTOCOL = 8` adds `Moved` |

- REQ-110 WHEN `w` runs, the TUI SHALL ask the worktree's name with the next free `ws-N` filled in, show the branch, base and folder it will create, refuse a name `new_plan` rejects, and on `↵` create it from the current workspace's branch, run setup and switch to it.
- REQ-111 WHEN `r` runs on a worktree, the TUI SHALL validate the name, then move the folder, rename the branch (when jw named it) and update the registry, keeping the workspace's panes, layout and scrollback running; the daemon SHALL then start them in the new folder after a restart.
- REQ-112 WHEN an arrow follows the leader, the TUI SHALL open the next or previous open workspace (`↓`/`↑`) or the next or previous project (`→`/`←`), and SHALL treat further arrows the same WITHOUT the leader UNTIL another key, which ends it and is handled as usual (`↵` and `esc` are swallowed).

### Addendum 8 (D1): `X` accepts work merged through another PR

An agent can leave the branch jw registered and open its PR from a new one (`theme-handler` registered `feat/theme-handler`, PR #27, then shipped `feat/theme-json` as #31). `done_plan` compared HEAD only with the registered branch's PR, so `X` refused with "commits that are not in PR #27" although every commit was in `main`. It now fetches and also accepts a HEAD that `origin/<default>` contains (`actions::done_plan`).

- REQ-113 WHEN `X` runs the done checks and the worktree's HEAD is not in its PR, the TUI SHALL still take the done path IF `origin/<default>` contains that HEAD.

### Addendum 9 (R1): a restored pane starts with its modes off

After `jw server stop`, the agent's pane came back from its scrollback with the modes claude had turned on: any-motion mouse tracking in SGR, bracketed paste, application cursor. The restore only left the alternate screen (`?1049l`) and cleared the kitty flags, so the TUI kept sending mouse moves to the new shell, which printed them (`35;120;40M…`). The daemon now turns every mode vt100 tracks off after the history (`MODES_OFF` in `crates/daemon/src/lib.rs`).

- REQ-114 WHEN the daemon restores a pane from scrollback, the new process SHALL start with application cursor and keypad, bracketed paste, mouse tracking and mouse encodings off and the cursor shown, in the daemon and in every client.

### Addendum 10 (L0–L4): the user builds the project's layout

Prototype: https://claude.ai/artifact/WRawxSdY2Edbp1CeD9ZdMQ (approved 2026-10-10).

jw no longer picks a layout for a project. The first worktree of a project with no config asks for one, built by hand, with the agent and the setup, and writes them to `.jw.toml`. Afterwards `^␣ p` saves the panes on screen as the new layout.

| Area | What it does |
|---|---|
| When it asks | `^␣ w` on a project whose `Config.source` is `None` (no `.jw.toml`, no personal file). After the name, `↵` turns `Modal::New` into `Modal::Setup` with the same name; `esc` goes back to the name. `jw worktree` never asks |
| Layout builder | A `layout::Node` drawn as boxes, starting with one `shell`. Arrows (`hjkl`) select a box by position; `⇧`+arrow (`HJKL`) swaps its content with the box on that side; `\|` splits it side by side and `-` one above the other (the new half is a `shell`); `x` closes it (never the last one); `<` `>` move its split by 0.1 within 0.2–0.8; `␣` picks what runs in it: `editor`, `agent`, `shell`, or a typed command |
| Agent | `claude` or `codex`, with `←` `→` |
| Setup | The `setup` commands and `[[env]]` files `init::detect` finds; `␣` turns one off, `e` edits it, `a` adds a command. `tab` / `⇧tab` move between the three sections |
| Writing | `↵` writes `<project root>/.jw.toml` (`init::render_project`): setup, env, `[agent] default`, and `[layout]` with `editor = "nvim"` and the tree. Then it reloads the project and creates the worktree with it. The file is new in the repo; jw doesn't commit it |
| Default layout | Without a `[layout]` tree a workspace opens with one `shell` (`Node::default_tree`) |
| Setup with no shell | When the tree has no `shell` leaf, setup runs in the `editor` pane, else the `agent` pane, else the first one, before its program: `printf '$ <line>'; <line>; exec <program>` |
| `^␣ p` | Saves the current workspace's panes as `[layout]` in the project's config file (`Config.source`, else a new `.jw.toml`). Each live leaf's role maps back to the `run` it came from in the workspace's opening tree (`shell-2` is `shell`; a pane added later is a `shell`). Only `split`, `ratio`, `a`, `b` and `run` of `[layout]` are rewritten (`config::save_layout`); `editor` and every other table stay. A modal shows the old and new `[layout]` first |
| Header | `layout changed` while the workspace's panes (shape and runs) differ from the project's layout |

- REQ-115 WHEN `w` runs on a project with no config file AND the name is accepted, the TUI SHALL show the setup step with the layout builder (one `shell` box), the agent, and the setup `init::detect` found.
- REQ-116 WHILE the setup step's layout section is focused, the TUI SHALL select boxes with the arrows, swap a box's content with `⇧`+arrow, split with `|` and `-`, close with `x` unless it is the last box, resize with `<` `>` within 0.2–0.8, and set what runs with `␣`.
- REQ-117 WHEN `↵` is pressed in the setup step, the TUI SHALL write `.jw.toml` at the project root with the chosen setup, env files, agent and layout, THEN create the worktree with that config.
- REQ-118 WHERE a project's config has no `[layout]` tree, THE workspace SHALL open with one `shell` pane.
- REQ-119 IF a new worktree has setup AND its layout has no `shell` pane, THEN the `editor` pane (else the `agent` pane, else the first) SHALL run the setup before its program.
- REQ-120 WHEN `p` runs, the TUI SHALL show the current and new `[layout]`, and on `↵` write the workspace's pane tree to the project's config file, creating `.jw.toml` if there is none, and leave the rest of the file as it was.
- REQ-121 WHILE a worktree's panes differ from its project's layout, the header SHALL show `layout changed`.
- REQ-122 `jw worktree` SHALL never ask for a layout.

### Addendum 11 (D2): `d` on the project root

`d` refused the project root ("is the project itself: diff is for its worktrees"), so the root's own work had no diff. It now runs on any git workspace: against `origin/<default>` like a worktree, or against `HEAD` when origin has no default branch (`diff::load` with an empty base).

- REQ-123 WHEN `d` runs on a project root that is a git repository, the TUI SHALL open its diff against `origin/<default>`, or against `HEAD` WHERE origin has no default branch.

### Addendum 12 (X0): selecting text in a pane

Prototypes: https://claude.ai/artifact/1rqnVPFC6xxy3nG1Rv1vkw (selection) and https://claude.ai/artifact/57RFfiX7qRAXUw4ZahD7uk (copied banner, option A), approved 2026-10-10.

jw owns the mouse (capture is on), so a plain drag in a shell did nothing and only Shift + drag, the host terminal's own selection, worked. jw now selects and copies itself with the mouse, and says so with a banner.

| Area | What it does |
|---|---|
| Drag | In a pane whose program has no mouse mode, a left drag highlights from the press to the pointer, in the theme's blue with the background colour as text, row by row, over the visible screen and the history. On release the text is copied |
| Shift + drag | In a pane whose program turned the mouse on (nvim, claude), plain drag still goes to the program; WHERE the outer terminal hands jw a Shift + drag (many keep it for their own selection, RISK-41), it selects as above |
| Words and lines | Double-click selects a word (letters, digits and `_ . / ~ : -`), triple-click a line |
| Edges | Dragging above or below the pane scrolls the history one line every 60 ms and keeps extending the selection |
| Copy | OSC 52 written to the outer terminal. Rows lose their trailing spaces and join with `\n`, except a soft-wrapped row, which joins to the next with nothing |
| Banner | `✓ Copied to clipboard · N characters` on the top edge of the pane at its right end, about 1.5 s (REQ-135) |

Viewer panes (diff, Markdown) keep their own clicks and are out of scope. No key is bound with Alt.

- REQ-124 WHERE the pane's program has not turned the mouse on, WHEN the left button is dragged inside the pane, the TUI SHALL highlight the cells between the press and the pointer, and on release SHALL copy them.
- REQ-125 WHERE the pane's program turned the mouse on, the TUI SHALL keep passing a plain drag to the program (REQ-59), and WHERE the outer terminal delivers a drag with Shift held, SHALL select as REQ-124 says.
- REQ-126 WHEN a double-click or triple-click lands in a pane that is not passing the mouse, the TUI SHALL select that word or line and copy it.
- REQ-127 WHILE a drag is held above or below the pane, the TUI SHALL scroll the history one line every 60 ms and extend the selection over it.
- REQ-128 WHEN the TUI copies, it SHALL send the selected text to the outer terminal as OSC 52, with trailing spaces removed from each row that does not wrap, rows joined by `\n`, and a soft-wrapped row joined to the next without one.
- REQ-129 *Withdrawn* (select mode on `^␣ v`; see Corrections). The id stays retired.
- REQ-130 *Withdrawn* with REQ-129.
- REQ-131 IF the pane's history can no longer keep a selection on its text (the screen switched, the history shrank, or it is full), THEN the TUI SHALL clear it, except while a drag is held.
- REQ-132 WHEN the user clicks in a pane that is not passing the mouse, or the focus changes, the TUI SHALL clear the selection.
- REQ-133 THE selection SHALL add no `ClientMsg` and no `PaneInfo` field, and `PROTOCOL` SHALL stay at 8.
- REQ-135 WHEN a selection is copied, the TUI SHALL show `✓ Copied to clipboard · N characters` for about 1.5 s on the top edge of the pane's border at its right end, drawn over the border and the title, and SHALL send no key or click to it.
- REQ-134 WHEN the wheel scrolls the history of a pane that has a selection, the selection SHALL stay on the same text.

### Addendum 13 (M0–M6): motion and first steps for new users

Prototype: https://claude.ai/artifact/B17WfXDXi8tRw2DrVyP7AP (nine proposals, all approved 2026-10-10).

jw is a terminal people live in, but a first-time user sees a grey line on an empty screen, a key hint that never changes, and a sidebar that jumps. This addendum makes it feel smooth (every change eased, frames tear-free) and teaches the leader on the first run. It replaces REQ-19 and REQ-20, which left animation "later, optional".

| Area | What it does |
|---|---|
| Frames | Event-driven as today, coalescing into `FRAME` (16 ms). While an animation runs the loop wakes every 16 ms; with none it blocks as it does now, so idle CPU stays at zero. Animations read `Instant`, never count frames |
| Synchronized output | Every draw is wrapped in `CSI ? 2026 h … CSI ? 2026 l`: the terminal paints the frame whole or not at all |
| Workspace switch | The sidebar's selection slides to the new row (240 ms, ease-out), the header fades, the panes' content enters from the side of travel. For arrows after the leader, `1–9`, a click and the picker |
| Agent mark | `✻` spins as a braille glyph between the 2 s polls (80 ms per step), only while the mark is on |
| Status bar | The hint changes with the mode and cross-fades; the mode chip's colour eases between modes |
| Messages | Status messages enter from the right and fade out; their total time stays 4 s |
| Popups | Which-key, pickers, modals and settings fade in over 160 ms; a picker's selected row slides |
| Which-key | Appears after `which_delay_ms` (default 150 ms, was 600), or at once with `which_sticky` |
| Focus | The focused pane's border colour eases over 180 ms; the wheel scrolls the history over at most 120 ms |
| First run | With an empty sidebar (no workspace registered) and `welcomed` unset, three steps appear one by one: `^␣` starts every shortcut, then `o` opens a folder, `?` lists the keys. Any key closes it and sets `welcomed` |

Terminal cells cannot blend, so a fade is a colour interpolated toward the background in truecolor and a slide moves content by whole cells, rounded from an eased position. Without truecolor (`COLORTERM` not `truecolor` or `24bit`) the colour fades and the panes' slide apply at once (RISK-42). On a switch the client clears the old workspace's screens and the new ones arrive from the daemon, so the panes only enter; nothing leaves. `settings.json` gains `animations` (default true), `which_sticky` (default false) and `welcomed` (default false); `^␣ ,` shows the first two. Env `JW_WELCOME=0` skips the welcome, like `JW_LEADER` and `JW_THEME` win over their keys.

- REQ-136 WHERE `animations` is not false, WHEN the current workspace changes, the TUI SHALL slide the sidebar selection to the new row over 240 ms, fade the header, and bring the panes' content in from the side of travel over 240 ms.
- REQ-137 WHILE any animation is running, the TUI SHALL redraw at most every `FRAME`, and WHEN none is running SHALL block on its channel as before. Each animation SHALL take its position from `Instant::elapsed`.
- REQ-138 WHERE `animations` is false, the TUI SHALL apply every change at once, draw `✻` as a still glyph (REQ-143) and show all three welcome steps together (REQ-146); WHERE the terminal has no truecolor, it SHALL apply the colour fades and the panes' slide at once, and keep the selection band's slide, the spinner and the welcome steps. REQ-20 is amended: the key is `animations` in `settings.json`, not a `[tui]` table (see Corrections).
- REQ-139 THE TUI SHALL write `CSI ? 2026 h` before each draw and `CSI ? 2026 l` after it, and SHALL write the closing sequence on exit, on a panic and on a failed draw.
- REQ-140 WHEN the mode changes, the status bar SHALL cross-fade to the new mode's hint over 160 ms and ease the chip's colour over 180 ms. The hints are: TERM, leader, MOVE, viewer and confirm.
- REQ-141 WHEN the leader has been pending for `which_delay_ms` (default 150 ms), or at once WHERE `which_sticky` is true, the TUI SHALL show the which-key popup, fading in over 160 ms.
- REQ-142 WHEN a status message appears, it SHALL enter from the right over 200 ms, and SHALL fade out over the last 200 ms of its `STATUS_FOR` (4 s).
- REQ-149 THE error messages that the changes of this addendum add or touch SHALL say what went wrong and how to fix it.
- REQ-143 WHILE a workspace's `✻` mark is on, the sidebar SHALL draw it as a braille spinner that advances every 80 ms, computed from `Instant` between `List` polls.
- REQ-144 WHEN a modal, picker, settings screen or which-key popup opens, it SHALL fade in over 160 ms; WHEN a picker's selection moves, the highlighted row SHALL slide to it over 120 ms.
- REQ-145 WHEN the focus moves to another pane, the old border colour SHALL ease to the new one over 180 ms; WHEN the wheel scrolls a pane's history, the scroll SHALL be spread over at most 120 ms.
- REQ-146 WHEN the TUI starts with an empty sidebar and `welcomed` is false (a workspace opening by itself closes it too, without a key), it SHALL show three steps, one every 650 ms, and WHEN any key is pressed SHALL close them, write `welcomed: true` and handle that key as usual. WHERE `JW_WELCOME=0` is set, the TUI SHALL NOT show it, so tests and `scripts/drive.py` (which sets it) see the plain empty stage. The empty-stage line (Screen table) stays for later runs.
- REQ-147 THE motion SHALL add no `ClientMsg` and no `PaneInfo` field, and `PROTOCOL` SHALL stay at 8.
- REQ-148 WHEN a key or a click arrives during an animation, the TUI SHALL handle it before the next frame; WHEN a change interrupts an animation, the new one SHALL start from what is on screen; IF a frame takes longer than `FRAME`, THEN the animation SHALL jump to its end state.

### Addendum 14 (W1): the sidebar follows the registry

A TUI notices changes to `workspaces.json` and the session's `folders.json` made by anyone else (`jw worktree`, `jw open`, another TUI) and rebuilds its rows without a restart. Until now only its own jobs called `App::reload`.

| Piece | Shape |
|---|---|
| Watched | `workspaces.json` (`registry::default_path`) and the session's `folders.json` (`folders::default_path`). Both are replaced by temp + rename, so the mtime moves on every write |
| `RegistryWatch` | Like `settings::Watch`: `seen: [(PathBuf, Option<SystemTime>)]`, `stat` only; `sync()` re-reads the paths and stamps at the start of every `reload`, which a session switch runs. Checked in `Msg::Tick` inside the `ticks % 4 == 0` block (every 2 s) |
| Rebuild | `reload` splits into `load_rows` (read-only: no `fill_roots`, no save) and applying the rows. A change runs `load_rows` on its own thread (not `App::background`: no busy mark) and comes back as `Msg::Rows` |
| Overlay | While `modal`, `finder` or `settings` is `Some`, the rows wait in `App::pending_rows` and apply when the last one closes |
| Own writes | `on_job` and the inline writers keep calling `reload`, then re-stamp the watch |

- REQ-150 WHEN `workspaces.json` or the session's `folders.json` changes on disk, the TUI SHALL rebuild the sidebar rows within 3 s without a keypress.
- REQ-151 THE TUI SHALL detect the change with `stat` only, on the existing `Msg::Tick`, and SHALL add no thread, timer or `next_wake` entry.
- REQ-152 WHILE the TUI is idle with no animation, THE event loop SHALL block on its channel as before; the watch SHALL cost no extra wake.
- REQ-153 WHEN rows are rebuilt, THE TUI SHALL keep the open workspace (`App::active`) by id, with its panes, focus and layout.
- REQ-154 WHILE a modal, picker or settings screen is open, THE TUI SHALL hold the rebuilt rows and SHALL apply them when it closes.
- REQ-155 THE live rebuild SHALL NOT write `workspaces.json` or `folders.json`.
- REQ-156 THE live rebuild SHALL run off the UI thread, with no busy mark.
- REQ-157 IF the file is half-written or does not parse, THEN the TUI SHALL keep the current rows and try again on the next change, without repeating an error in the status bar.
- REQ-158 WHEN the TUI writes the registry itself, it SHALL NOT rebuild a second time for that write.
- REQ-159 THE change SHALL add no `ClientMsg` and no `PaneInfo` field, and `PROTOCOL` SHALL stay at 8.

### Addendum 15 (R1–R3): review a pull request (`^␣ R`)

Prototype: https://claude.ai/artifact/7dJfvauPHjSBZ3jF5e8SJ7 (approved 2026-10-11).

`^␣ R` lists the project's open PRs and `↵` opens a **review workspace**: a worktree `review-<n>` on branch `review/pr-<n>` at the PR's head, with the diff viewer (the one `^␣ d` opens) on the left half and claude on the right half, whatever the project's `[layout]` says. jw writes the PR's title, description, comments, reviews and inline comments to a context file and sends claude a short prompt that points at it. The review is read-only by intent and jw never writes to GitHub.

| Piece | Shape |
|---|---|
| gh list | `Client::open_prs(dir, requested)` in `crates/core/src/connectors/github.rs` (beside `list`): `gh pr list --state open --limit 100 --json number,title,body,author,headRefName,baseRefName,isDraft,updatedAt,reviewDecision,changedFiles,files,additions,deletions,url`, plus `--search "review-requested:@me"` when `requested` |
| gh detail | `Client::detail(dir, n)`: `gh pr view n --json title,body,author,baseRefName,headRefName,comments,reviews,files,url` and `gh api repos/{owner}/{repo}/pulls/n/comments` (inline). gh resolves the repo from the cwd |
| Registry | `Entry.review_base: String` (`core/registry.rs`, `serde(default, skip_serializing_if = "String::is_empty")`); `Entry.pr` holds `<n>`. `Entry` ignores unknown keys |
| Stream | `Stream::resolve` (`stream.rs`): WHEN `entry.review_base` is set, `base` is it and `tree` is `review_tree()`: `split right 0.5 { a: view:diff, b: agent }`. `command()` returns no command for a `view:*` run |
| Core action | `review::review_stream(project, pr)` (`crates/core/src/review.rs`): `git fetch origin +pull/<n>/head:refs/jw/pr-<n>` (a ref of its own, so a checked-out review branch can be refreshed) and `git fetch origin <base>`, then `git worktree add -B review/pr-<n> root/review-<n> refs/jw/pr-<n>` and the registry entry with `branch`, `pr`, `review_base`. An existing review is moved to the new head with `reset --hard` unless it has changes. Not `new_stream`: its `from` is a start ref and it errors on an existing branch |
| Context file | `<state>/reviews/<session>/pr-<n>.md`: header, description, conversation, reviews, inline comments by file:line, files and stat; capped at 200 KB, the cut noted. The diff stays in git |
| TUI | `Kind::Reviews` and `Pick::Review` in `finder.rs`; action `review` (group worktree, key `R`) in `settings.rs`; `ask_review`; jobs through `App::background`; creation ends in `Job::Created` → `open_entry`, like `w` |
| Prompt | After `start_panes` sends `Open`, a pending `(stream, text)` on `App` goes out as `ClientMsg::Prompt`. The TUI never sent `Prompt` before; the daemon waits for the agent's output to be quiet and writes it (REQ-13) |

- REQ-160 WHEN `^␣ R` is pressed in a workspace of a git project, the TUI SHALL open the PR picker and load open PRs in a background job.
- REQ-161 WHILE the list loads, the picker SHALL show a loading row and accept `esc`.
- REQ-162 IF `gh` is missing, unauthenticated or the repo has no GitHub remote, THEN the TUI SHALL show gh's reason as an Error status and open no picker.
- REQ-163 THE picker SHALL list open PRs by most recent update and filter fuzzily over number, title, author and branch.
- REQ-164 WHILE a PR is selected, the picker SHALL preview its description and changed files.
- REQ-165 WHEN `↵` picks a PR, a background job SHALL fetch `pull/<n>/head` into `review/pr-<n>` and fetch `origin/<base>`.
- REQ-166 WHEN both fetches succeed, jw SHALL create worktree `review-<n>` at that branch, register it with `review_base` set to the PR's base, and run the project's setup as `w` does.
- REQ-167 IF a fetch fails, THEN jw SHALL create nothing and show the error.
- REQ-168 WHEN a workspace with `review_base` opens, first time or reopened, its panes SHALL be a diff left and an agent right, each half of the panes area, whatever `[layout]` says.
- REQ-169 WHILE a workspace has `review_base`, its diff SHALL compare against `origin/<review_base>`.
- REQ-170 WHEN a review workspace is created, jw SHALL write the context file before the agent starts.
- REQ-171 WHEN the review workspace's panes are open, the TUI SHALL send `Prompt` naming the PR, the context file and `git diff origin/<base>...HEAD`, and telling the agent to report findings by file:line and not to edit, commit or push.
- REQ-172 WHERE `review-<n>` already exists, WHEN its PR is picked, the TUI SHALL refetch the head and switch to it.
- REQ-173 WHEN the TUI switches to an existing review workspace, it SHALL ask before it sends the prompt again.
- REQ-174 THE jw binary SHALL call `gh` only as `pr list`, `pr view` and `api` reads during a review.
- REQ-175 WHEN `X` removes a review worktree, the existing done/rm checks SHALL apply, and a `review/pr-<n>` with no commits beyond the fetched head SHALL count as having no unpushed work.
- REQ-176 THE `R` key SHALL be rebindable and SHALL NOT use Alt.
- REQ-177 THE `PROTOCOL` constant SHALL stay 8 and no `ClientMsg`, `DaemonMsg` or `PaneInfo` shape SHALL change.
- REQ-178 WHEN `Entry.review_base` is empty, `workspaces.json` SHALL serialise exactly as before.
- REQ-179 WHEN `m` is pressed in the picker with an empty query, the TUI SHALL reload the list filtered to PRs whose review is requested from the user, and `m` again SHALL reload all open PRs.
- REQ-180 WHEN `o` is pressed in the picker with an empty query, the TUI SHALL open the selected PR's url in the browser.
- REQ-181 WHEN `Open` carries a leaf whose role starts with `view:`, the daemon SHALL give it an id and start no process, as `Split` does.
- REQ-182 IF a review worktree has changes when its PR is picked again, THEN jw SHALL leave it as it is and say so.
- REQ-183 WHILE a workspace has `review_base`, the header SHALL NOT say its layout changed or offer `^␣ p` to save it.

### Addendum 19 (V0–V4): the diff pane has three views (General, Uncommitted, Commits)

Prototype: https://claude.ai/artifact/9mXWH52Ujf9JjYeGkbMSsY (approved 2026-10-11; all navigation in the left sidebar, no tabs).

The diff pane (`^␣ d`, and the left half of a review workspace) mixed committed and uncommitted work in one scroll. It now has three views of the same pane, stacked in its left sidebar: **General** (the branch against its base, committed work only), **Uncommitted** (staged, unstaged and untracked) and **Commits** (the branch's commits and the diff of the selected one). Scope is client state; the pane stays `view:diff`.

| Piece | Shape |
|---|---|
| Scope | `diff::Scope { Branch{base}, Uncommitted, Commit(sha) }` in `crates/core/src/diff.rs`; `diff::load(dir, &Scope)` replaces `load(dir, base)` (now `diff.rs:163`); its caller is `load_diff` (`tui/mod.rs:2966`) |
| Loaded | `Loaded::Files(Vec<File>)` or `Loaded::TooLarge{files, total}`: the cap is `DIFF_CAP` = 2 MiB of git's stdout, then git is killed and `--name-status` lists the files |
| General | `git diff -M --merge-base origin/<base> HEAD`; empty base: nothing, with a note |
| Uncommitted | `git diff -M HEAD` + `ls-files --others --exclude-standard -z` (`diff::untracked`, `diff.rs:237`); no commit yet: against `EMPTY_TREE` (`diff.rs:76`) |
| Commits | `diff::commits(dir, base, 200)`: `git log -n201 --format=… origin/<base>..HEAD` (empty base: `HEAD`; the extra one says whether more exist) → `Commits{list: Vec<Commit{sha, short, subject, when}>, more}`; detail `git show -M --format= <sha>` (a merge: `-m --first-parent`) |
| State | `DiffView` (`tui/diffview.rs`) holds `which`, the selected commit, the sidebar's cursor, a `Load` (idle, loading with a number, ready, failed) per slot with a commit's diffs kept by sha, and per view or commit a `Pos{scroll, viewed: BTreeSet<String>}` (before, `viewed` held indices). `diff::File` gains `untracked` |
| Jobs | `Job::Diff` (`tui/mod.rs:109`) carries General for a pane that opens; `Job::DiffPart{pane, slot, seq, result}` answers any other piece. The view asks with `Action::Load(Request{slot, seq})` and takes the answer with `DiffView::apply`, which drops it unless `seq` is the number its slot waits for; `App::load_part` runs it through `App::background` |
| Keys | In `DiffView::key`, fixed like the other viewer keys, not in `settings::ACTIONS` (those are leader keys). Hints: `Mode::Diff`, `DiffCommits` and `DiffSide` in `draw.rs` |
| Sidebar | `DiffView::draw_sidebar` (`TREE` = 30 columns) draws the three sections; below `SIDEBAR_MIN` = 100 columns it is an overlay and `crumb` takes its place |
| Marks | `M` yellow, `A` green, `D` red, `R` cyan, `?` untracked magenta; the selected row `sel`; all through `theme::p()` |

- REQ-222 WHEN a diff pane opens, the TUI SHALL show General and load only General.
- REQ-223 THE General view SHALL list the files changed between `origin/<base>` and `HEAD` and SHALL NOT list uncommitted work.
- REQ-224 THE Uncommitted view SHALL list staged, unstaged and untracked changes against `HEAD`, each file with its mark `A M D R ?`; WHERE `HEAD` has no commit, against the empty tree.
- REQ-225 THE Commits view SHALL list `origin/<base>..HEAD` newest first, up to 200, each with short sha, subject and age, and SHALL show the diff of the selected commit, the newest first. IF more than 200 exist, THEN the list SHALL end with "older commits not shown after 200".
- REQ-226 IF origin has no default branch, THEN General SHALL say so and list nothing, and Commits SHALL list the last 200 commits of `HEAD`.
- REQ-227 THE sidebar SHALL show the three views stacked, each with its number key and its count; the active view `▾` in blue with its files below (in Commits: the commits, and under the selected one its files), the others `▸`.
- REQ-228 WHILE a view loads, its count SHALL be a spinner and its body a loading line, and every key SHALL stay live.
- REQ-229 WHEN `1`, `2` or `3` is pressed in the pane, the TUI SHALL show General, Uncommitted or Commits.
- REQ-230 WHEN `Tab` is pressed, the TUI SHALL move the cursor into the sidebar, where `↑↓` or `jk` move, `↵` chooses a view, a commit or a file, and `Tab` or `esc` return to the diff.
- REQ-231 WHILE Commits is the active view, `(` and `)` SHALL select the previous and the next commit.
- REQ-232 WHILE the pane is narrower than 100 columns, the sidebar SHALL be hidden, a line under the title SHALL name the view (in Commits: the sha, subject and `n/m`), and `Tab` SHALL open the sidebar over the diff.
- REQ-233 WHEN the pane changes view or commit, it SHALL keep each one's scroll position and viewed files, and restore them on return.
- REQ-234 WHEN a view is reloaded, the TUI SHALL keep its scroll position and viewed files by path and drop those no longer in it.
- REQ-235 THE first visit to Uncommitted or to Commits SHALL load it in a background job, and a commit's diff SHALL load when first selected and be kept for the life of the pane.
- REQ-236 WHEN a job's result arrives for a slot that has a newer request, the TUI SHALL discard it.
- REQ-237 THE TUI SHALL NOT run git in `draw`, in a key handler or in a click handler.
- REQ-238 IF a diff exceeds `DIFF_CAP`, THEN its view SHALL list the first 200 files with the total and say it is too large to draw, and `↵` on a file SHALL still open it.
- REQ-239 THE keys `j k ] [ v t g G`, `Ctrl-d`, `Ctrl-u`, `PgUp`, `PgDn`, the wheel and `q` SHALL act in every view as REQ-21 says, and `t` SHALL apply to the whole pane.
- REQ-240 WHEN `↵` is pressed on a file in any view, the TUI SHALL open the workspace's working-tree file as REQ-42 says, also from a commit.
- REQ-241 WHERE a workspace has `review_base`, the three views SHALL compare against `origin/<review_base>`, and General SHALL be the range REQ-171 names.
- REQ-242 THE pane's title SHALL read `changes  <name> vs <base> · committed`, `changes  <name> · not committed` or `changes  <name> · <n> commits`.
- REQ-243 THE new rows and marks SHALL take every colour from `theme::p()`, and no key SHALL use Alt.
- REQ-244 THE change SHALL add no `ClientMsg`, `DaemonMsg` or `PaneInfo` field, SHALL leave the `view:diff` role as it is, and SHALL NOT change `PROTOCOL`.
- REQ-245 WHEN a diff pane is restored, the TUI SHALL open General, whichever view it showed before.
- REQ-246 IF git fails for a view, THEN that view SHALL show git's first error line in place of its diff and the other views SHALL stay usable.

---

### Addendum 16 (F0–F4): switching workspace stays inside a 16.6 ms frame budget

Switching worktree is slow and its animation stutters. Fix the time, measure first, keep every frame ≤ 16.6 ms (60 fps).

#### Contract

On a switch the TUI sends `Detach`, **clears every pane** (`forget_workspace`), then `Attach`; the daemon replies `Tree` and one `Snapshot` per pane (2 MiB ring replay + screen); the TUI replays each into a new 5,000-row `vt100::Parser` on the UI thread. Until then the pane area is empty, and the slide (REQ-136) animates over nothing.

After this addendum: the UI thread never waits on disk, process or socket work during a switch; no frame exceeds the budget; the panes' first content appears within the switch budget; nothing is lost (scrollback, kitty/mode state).

#### Frame budget (unmeasured targets, F0 fixes the real numbers)

| Phase | Budget | Where |
|---|---|---|
| handle a message / key | ≤ 1 ms | `handle` `mod.rs:1029` |
| build buffer (`draw`, `screen_to`) | ≤ 6 ms at 240×60 | `draw.rs:15`, `draw.rs:879` |
| ratatui diff + write to the terminal | ≤ 3 ms, ≤ 64 KiB per frame | `mod.rs:549` |
| slack (parse of a burst, `drain`) | ≥ 6 ms | `mod.rs:173` |
| key → first frame with the new panes' content | ≤ 100 ms (OPEN-22) | whole path |

#### Suspects (S1–S8 from reading; the verdicts are F0's measurements, below)

| # | Where | Why it is slow |
|---|---|---|
| S1 | `open_entry` `mod.rs:1453` | `save_recent` (write+rename), `folders::edit`, `Stream::resolve` (`core/src/stream.rs:45`: config load, `default_branch` spawns `git`, `core/src/connectors/git.rs:84`), `open_specs`, `mark_opened`: disk and a **git process** on the UI thread |
| S7 | `on_daemon` `mod.rs:1103`, `fit` `mod.rs:1662` | every `Snapshot` and `Tree` runs `fit()`: N full re-layouts, a `Resize` to the daemon per changed pane; `Tree` also triggers `List` and `find_here` (canonicalize) |
| S9 | `event_loop` `mod.rs:793`, `next_wake` `mod.rs:561` | the next frame is due `FRAME` after the *end* of the last draw (`last_draw` is set after it), so the cadence is 16 ms + draw + wake slop |
| S11 | `reload` `mod.rs:1007`, `folders::entry` `core/src/folders.rs:176` | the sidebar's rows ran `git` (repo, remote, branch) for every folder on the UI thread, at every reload (each open, each job): 35 ms × folders |
| S8 | `daemon/src/lib.rs:774` | a *new* workspace's `Open` spawns PTYs and writes `session.json` + scrollback under a lock before `Attach` |
| S2 | `on_daemon(Snapshot)` `mod.rs:1101` | `Parser::new(…, SCROLLBACK)` + replay of up to 2 MiB per pane, on the UI thread, ahead of the next draw |
| S3 | `snapshot()` `daemon/src/lib.rs:383` | copies the whole ring per pane per attach, then `state_formatted` |
| S4 | `forget_workspace` `mod.rs:2457` | blank panes until S2 ends; the slide shows nothing |
| S5 | `screen_to` `draw.rs:879` | per-cell `Style` build every frame, no cache, O(area) |
| S6 | `synced` `anim.rs:187` + `Client::send` `client.rs:78` | extra flushes per frame; a blocking socket write on the UI thread |

#### Measurements (F0, release build, 60×240, one shell pane with a full 2 MiB ring, 30 switches, M-series Mac; `scripts/bench-switch.py`)

| Phase | p50 | p95 | Budget | Verdict |
|---|---|---|---|---|
| `Stream::resolve` (UI thread, before `Detach`; spawns `git`) | 39.8 ms | 62.6 ms | ≤ 1 ms | **S1 confirmed**, the first stall |
| `save_recent`, `folders_edit`, `forget`, every `send` | ≤ 0.5 ms each (p95 ≤ 2.4) | | ≤ 1 ms | S1's file I/O and S6's sends are fine here |
| `Attach` → `Tree` → `Snapshot` at the TUI (daemon + socket, 2 MiB) | 2.3 ms | | | **S3 not a problem** at 1 pane |
| `Snapshot` parse (`Parser::new` + 2 MiB replay), UI thread | 86.5 ms | 109.8 ms | ≤ 1 ms | **S2 confirmed**: ≈ 5 frames lost, per pane |
| `fit`, `set_tree` | < 0.1 ms | | | S7 not a problem at 1 pane (`Resize` count grows with panes) |
| draw: build the buffer / whole `term.draw` + sync | 1.0 / 1.9 ms (p95 7.8 / 9.5) | | ≤ 6 / ≤ 9 ms | **S5 not a problem**: no frame over 16 ms |
| bytes written to the terminal per switch | 142–187 KiB | max 250 KiB | ≤ 64 KiB per frame | fine per frame (≈ 15 frames) |
| key → first frame with the new content | 133 ms | 168 ms | ≤ 100 ms | over: 40 + 86 of the 133 are S1 and S2 |
| frame cadence while the slide runs | 20.4–20.8 ms (≈ 49 fps) | 21.3 ms | 16.7 ms | **S9 confirmed**: draw takes 1 ms, so ≈ 3 ms is the deadline |
| longest gap without a frame after the key | 120 ms | 146 ms | 16.7 ms | S1 + S2, back to back |

Timeline of one switch (ms from the key): `resolve` 0–42 (no frame) → `Attach` 42.6 → `Tree` and `Snapshot` at the TUI at 43–45 → one frame at 46 → `handle_snapshot` 46–129 (no frame) → frames every ≈ 20 ms. By the time the content is in, the 240 ms slide is already 55 % over, so it jumps in, it does not slide.

After F1 (same bench, two runs): `resolve` 0 ms (the stream comes from a cache `warm_streams` fills on a worker thread), frame cadence 16.0–16.1 ms (60 fps; S9 fixed by counting the deadline from the frame's start), key → content 119–124 ms p50, longest gap without a frame 108–114 ms: all of it is the `Snapshot` parse (103–108 ms in these runs, noisier than F0's 86), which is F2. A workspace that is not open still resolves fresh on the UI thread (its agent session and layout are read as its panes start), so REQ-185 holds for switching, not yet for the first open.

After F3 (`Screen` first, `History` behind it; two runs): **key → content 12–14 ms p50 (p95 29–38), under the 100 ms budget**; the `History` replay still takes ≈ 100 ms but on its own thread, and the swap into the pane costs 0.004 ms; the UI's longest message 4 ms (p95 12–14), the longest gap without a frame 20 ms (p95 33–35: the first frame after the key), cadence 15.9 ms. The scrollback of a pane is complete ≈ 100 ms after the switch instead of before it appears.

F1 had only covered a workspace that is already open. Opening one that is closed from the sidebar (9 workspaces, F3's build) took `resolve` 58 ms on the UI thread (key → content 69 ms, no frame for 65 ms), and opening a new folder from the picker took **261 ms** (key → content, p95 395; the UI thread was blocked 319 ms): `reload` ran `git` for each of the 9 folders (**S11**: `reload_entries` 165 ms p50, 352 ms max, growing with the folders). Fixes (F4, "opening"): `Stream::is_fresh` (a `Stamp` of the configs a resolve read) lets a closed workspace use the stream resolved off the UI thread, with its agent session made as `session_of` does; `folders::entry` keeps the repo's project per folder while `.git` and `.git/config` keep their mtimes and reads the branch from `.git/HEAD` (`git::branch_of`) instead of `git branch`. After: closed workspace from the sidebar key → content **22 ms p50** (was 69), picker open **52 ms** (was 261), `reload` 0.2 ms (was 165). What is left of the picker's 52 ms is the new folder's first `resolve` (37 ms) and its first `project_of` (≈ 35 ms), once per new folder.

S10 (a ≈ 30 ms frame when `^␣` is pressed) was seen in three runs and **does not reproduce**. What the trace showed when it did: the cost sat in ratatui's diff-and-flush after the build (`term_draw` 21–46 ms, `draw_build` ≈ 1 ms), the leader's frame writes only ≈ 2 KiB and changes few cells, and it was not caused by idling. Since then 7 runs of 30 switches (≈ 700 frames each) had 0 frames over 16 ms in six runs and 4 in one (16–28 ms, during the slide), with no code change that touches it. Nothing is fixed because nothing was found: it was most likely the machine or the harness (a pty nobody reads for a moment blocks a write on macOS). If `^␣` feels late in Ghostty, report it and run with `JW_TRACE`: `sync_begin`, `term_draw`, `sync_end` and (with `JW_TRACE_CELLS=1`) `diff_cells` split a frame into its parts. The slide's frames change ≈ 12,600 cells each (the whole pane moves by cells) and cost ≈ 1.3 ms.

After F2's threads (`Msg::Snapshot carries a `Parser` built by the reader thread; two runs): the UI thread's longest message during a switch 6–7 ms (was 86), the longest gap without a frame 20–21 ms (was 108–120), cadence 16.0 ms, 0 to 1 gaps over 20 ms. The replay itself did not get cheaper: 118–120 ms now on its own thread (it was 86–108 on the UI thread, and now competes with it for the cores), so **key → content is 146 ms p50, worse than before F2's 119–124 ms, and the 100 ms budget (REQ-188) is not met**. The slide now plays on time, over empty panes, and the content lands at ≈ 146 ms. What is left to cut is the replay of the 2 MiB ring (S3's payload, F3): the daemon sends the whole ring and the TUI replays all of it to rebuild 5,000 rows of scrollback, though only the screen is visible.

Opening a *new* workspace (3 samples): `resolve` 31 ms, but `handle_term` is 130 ms: about 100 ms more before or around `open_entry` that no span covers yet (the picker's confirm). Not measured further; F1 starts there.

Verdicts (before F3): **F1** (S1, S9) and **F2** (S2, S4) are the work; **F3** (S3, S8) and **F4** (S5) have no number behind them and are dropped unless more panes change that. Snapshot cost is per pane, so a workspace with three panes should cost ≈ 3 × 86 ms until F2.

#### Interfaces

| Item | Change | Home |
|---|---|---|
| `JW_TRACE=<file>` | F0: appends `µs,phase,value` lines (key, Detach sent, Tree rx, each Snapshot rx / parsed, `draw_build`, `term_draw`, `sync_begin`/`sync_end`, `diff_cells` under `JW_TRACE_CELLS`). Off by default, zero cost off | `mod.rs`, `draw.rs` |
| `scripts/bench-switch.py` | F0: drives N switches with `scripts/drive.py`, prints p50/p95/max per phase | `scripts/` |
| `Msg::Snapshot(PaneId, Parser)` | F2: the reader thread builds the `Parser`; the UI thread only inserts it | `mod.rs:327` |
| `App::background` jobs | F1: persist recent/folders/stream setup off-thread | `mod.rs` (existing) |
| `PROTOCOL` | 10 (addendum 18: `DaemonMsg::Opened`); 9 (F3): `ClientMsg::SplitSnapshots`, `DaemonMsg::Screen` and `History` (bytes travel raw) | `proto.rs:18` |
| `ClientMsg::SplitSnapshots` | F3: from then on this connection gets a pane it watches as `Screen` (the exact screen, small) then `History` (what a `Snapshot` carried); a client that does not ask still gets one `Snapshot`, and a resync is always a `Snapshot` | `proto.rs`, `daemon/src/lib.rs` (`split_snapshot`), `daemon/src/outbox.rs` |
| `Msg::Snapshot{history}` / `Msg::History` | F3: the TUI draws from the `Screen`, replays `History` on its own thread, then swaps it in with the output and resizes that came since (`Pending`) | `tui/mod.rs` (`on_history`) |

#### Acceptance

- REQ-184 THE TUI SHALL, with `JW_TRACE` set, write the per-phase timings of every switch; WHEN unset it SHALL add no work to a frame.
- REQ-185 WHEN a workspace is switched, the UI thread SHALL do no file, process or blocking socket work before the next draw (S1, S6).
- REQ-186 WHEN a `Snapshot` arrives, the TUI SHALL build its `Parser` off the UI thread and the UI thread SHALL spend ≤ 1 ms to install it.
- REQ-187 WHILE a switch animates, no frame SHALL take longer than 16 ms to build, diff and write at 240×60.
- REQ-188 WHEN the switch is requested, the TUI SHALL reach the new panes' first content within the key→content budget of the table above, p95 over 50 switches.
- REQ-189 (closed 2026-10-11, not built: see Corrections) IF a pane's `Snapshot` has not arrived when the slide ends, THEN the TUI SHALL keep drawing the pane's frame (title, border) and fill it when the snapshot lands, without a full-screen flicker.
- REQ-190 THE switch SHALL keep each pane's scrollback, modes and kitty flags exactly as before.
- REQ-192 WHEN a client that sent `SplitSnapshots` starts to watch a pane, the daemon SHALL send its `Screen` before its `History`, and the `History` bytes SHALL equal what a `Snapshot` of the same moment carries.
- REQ-193 WHILE a pane's `History` is replayed, the TUI SHALL draw and update the pane from its `Screen`, and WHEN the replay ends SHALL swap it in with the output and resizes that came since, so the pane shows no change but its scrollback.
- REQ-191 WHERE `animations` is false, a switch SHALL still obey REQ-185 and REQ-186.

Open (human decisions, still unanswered):

- OPEN-22 Is 100 ms key→content the right switch budget? (The slide itself is 240 ms, REQ-136.)
- OPEN-23 REQ-189 shows a pane's frame while it fills, instead of blank. That is a visible change, and AGENTS.md asks for an HTML prototype first. Prototype it, or accept it as a fix, not a UI change? (Answered 2026-10-11: neither; it is not needed after F3.)
- OPEN-24 (answered 2026-10-11) F0 first, numbers before F1–F4: see Measurements.
- OPEN-25 Brain workspace: this spec's open questions should be reported there; I could not reach it from here.

### Addendum 17 (A1, A2): a pane's program

A pane says what runs in it. The daemon already resolves the foreground process of every pane on each `List` (`PaneInfo.fg`, `daemon/src/lib.rs:549-554`), and the TUI already appends it to the title (`tui/mod.rs:1657-1663`). What is missing is knowing which programs are agents: `jw prompt` accepts only role `agent` (`lib.rs:515-520`), so a `claude` run by hand in a `shell` pane is out of reach of other workspaces. The program is `fg`; no new field, no new message. Prototype approved 2026-10-11 (variant B, programs by detection only).

| Piece | Shape |
|---|---|
| Program | `PaneInfo.fg`, unchanged. It is the foreground process group's command name (`process_name`, `lib.rs:1373`) |
| `is_agent_program(&str)` | One list in `jw-core`, the lowest crate all of daemon, tui and jw share: `claude`, `codex`, `gemini`, `aider`, `opencode`. Fixed in code |
| Agent pane | Role `agent`, or a live pane whose program is in that list |
| `foreground(&Pane)` | The `List` handler's `process_group_leader` + `process_name` pair, factored out so `Prompt` uses the same one |
| `Prompt` | `lib.rs:515`: role `agent` first; else the lowest pane id whose `foreground` is an agent program; else the error `{stream} has no agent pane open` as today |
| `jw ls` | `--json` keeps `panes` (role strings) and adds `programs: [{role, program}]`, one per pane with a program; the table shows the program beside the role |
| `jw read --pane` | `cli.rs:504-510`: role, then program; the missing-pane error lists both |
| Title | The program replaces the dim runs suffix and is drawn in `p().magenta`, bold, when it is an agent (variant B); other programs keep today's dim suffix |
| Marks | `App::marks()` (`tui/mod.rs:963`) treats an agent pane like role `agent` for the `✻` and `?` guesses |

- REQ-194 WHEN `jw prompt <workspace>` finds no pane with role `agent`, THE daemon SHALL send the text to the live pane with the lowest id whose foreground program is a known agent, and reply `Prompted` with that pane.
- REQ-195 WHEN a pane with role `agent` exists, THE daemon SHALL choose it before any pane chosen by program.
- REQ-196 IF no pane has role `agent` or an agent program, THEN `jw prompt` SHALL fail with `has no agent pane open`, as today.
- REQ-197 THE daemon SHALL read a pane's foreground program when `List` or `Prompt` asks, and SHALL add no thread, timer or `ps` to any other path; leaving `claude` shows on the next `List`.
- REQ-198 WHEN `Prompt` has a role `agent` pane, THE daemon SHALL NOT run `ps`.
- REQ-199 THE `jw ls --json` field `panes` SHALL stay a list of role strings; `programs` SHALL be added beside it.
- REQ-200 WHEN `jw read --pane <name>` matches no role, THE CLI SHALL match it against the panes' programs before failing.
- REQ-201 WHERE a pane's program is a known agent, THE TUI SHALL draw it on the title in `p().magenta` and bold, with no new colour literal.
- REQ-202 WHERE a pane's program is a known agent, THE sidebar SHALL give its workspace the same `✻` and `?` marks as role `agent`.
- REQ-203 THE change SHALL add no `ClientMsg` and no `PaneInfo` field, and `PROTOCOL` SHALL stay at 9.

### Addendum 18 (P1–P3): `jw pane open|close|ls`

An agent in a pane can split a workspace and run a program, a shell or a viewer in it, close a pane, and list them. The daemon already had `Split` and `Kill` for the TUI; what was missing was a CLI and the id of the pane a `Split` made, which the daemon announced only through the `Tree` broadcast (`daemon/src/lib.rs`, `add`). No UI is added: the new pane is an ordinary leaf of the tree, so there is no prototype (OPEN-27).

| Piece | Shape |
|---|---|
| `jw pane open <ws> --run <what> [--split right\|down] [--of <pane>]` | `cli.rs` `pane_open`: `find()` as `jw read`; Attach for the `Tree`; `Detach`; `Split`; waits for `Opened`; prints the id |
| `<what>` | `shell` → role `shell`, no command; `view:diff`; `view:md:<file>` (made absolute against the CLI's directory); anything else → role `run`, `sh -c` (`run_target`) |
| Anchor | `--of`, else the leaf with role `agent`, else the last leaf (`anchor`) |
| `jw pane close <ws> <pane> [--force]` | Attach for the `Tree`, check the pane is in it, `Kill`, wait for the `Tree` without it |
| `jw pane ls <ws> [--json]` | the `Tree`'s leaves joined with `List`: id, role, name, program (`PaneInfo.fg`), exit status |
| `DaemonMsg::Opened { pane }` | `proto.rs`: the reply to a `Split`, to the requester only. `split` and `add` return the id (`daemon/src/lib.rs`) |
| `PROTOCOL` | 10 |
| Skill | `crates/jw/skill/SKILL.md` and the test in `skill.rs`; `pane` is in `session::RESERVED` and `help::COMMANDS` |

- REQ-204 WHEN `jw pane open` succeeds, THE CLI SHALL print the new pane's id and nothing else on stdout, and exit 0.
- REQ-205 WHEN the daemon handles a `Split`, THE daemon SHALL send `DaemonMsg::Opened{pane}` to the requesting client only, and nothing to the workspace's other watchers but the `Tree`.
- REQ-206 THE `PROTOCOL` SHALL be 10, and THE CLI SHALL refuse a daemon that answers another number.
- REQ-207 WHEN `--run` is `view:diff` or `view:md:<file>`, THE daemon SHALL make a leaf with that role and no process.
- REQ-208 WHEN `--run` is `shell`, THE pane SHALL run `$SHELL` in the workspace's folder; WHEN it is another string, THE pane SHALL run it through `sh -c` with role `run`.
- REQ-209 WHEN the TUI shows the workspace, THE new or closed pane SHALL appear without any action of the TUI, through the `Tree` the daemon sends every watcher.
- REQ-210 WHILE no `--split` is given, THE CLI SHALL split `right`.
- REQ-211 IF `<ws>` matches no workspace, or several, THEN THE CLI SHALL fail with the message of `jw read`.
- REQ-212 IF no daemon runs, or the daemon does not run the workspace, THEN THE CLI SHALL fail with `<ws> is not open` and SHALL NOT start a daemon.
- REQ-213 IF `--of` names a pane outside the workspace, THEN THE CLI SHALL fail and send no `Split`.
- REQ-214 IF `jw pane close` names the last pane of the workspace, THEN THE CLI SHALL refuse unless `--force` is given.
- REQ-215 IF `jw pane close` names a pane outside the workspace, THEN THE CLI SHALL fail and send no `Kill`.
- REQ-216 THE `jw pane ls` output SHALL give, per leaf, its id, role, name, foreground program and exit status; `--json` SHALL give the same as JSON.
- REQ-217 THE `jw` skill SHALL name `jw pane open`, `jw pane close` and `jw pane ls`, and say that `--run` runs what it is given.
- REQ-218 IF the daemon answers `Error` to the `Split` or the `Kill`, THEN THE CLI SHALL fail with its message on stderr and exit 1.
- REQ-219 IF `jw pane close` names the pane it runs in (`$JW_PANE_ID`), THEN THE CLI SHALL refuse unless `--force` is given.
- REQ-221 IF `--run view:md:<file>` names a file that does not exist, THEN THE CLI SHALL fail and send no `Split`.
- REQ-220 WHEN `--of` is absent, THE CLI SHALL split beside the pane with role `agent`, else beside the last leaf of the tree.

Open (human decisions, still unanswered):

- OPEN-27 No HTML prototype, since no screen or key is added. Built on that default.
- OPEN-28 The default anchor is the agent's pane, else the last leaf, because the daemon does not know which pane has the focus.
- OPEN-29 `jw pane ls` is its own command; `jw ls` does not list panes.
- OPEN-30 An arbitrary program has role `run`, not `shell`.


### Addendum 20 (K1, K2): `jw prompt --pane`

`jw prompt` reaches the pane with role `agent`, else the lowest-id live pane running a known agent (REQ-194), so a second agent in a worktree (`jw pane open --run claude`) is out of reach. `--pane <id>` picks the target; the daemon validates it (RAT-68). No screen or key is added, so no HTML prototype (as OPEN-27).

| Piece | Shape |
|---|---|
| Message | `ClientMsg::Prompt { stream, text, pane: Option<PaneId> }`, `#[serde(default)]`; `PROTOCOL` 10 → 11 |
| Daemon | `agent_pane` takes the `Option<PaneId>`; with `Some` it validates (in the workspace, live, role `agent` or an agent program) instead of searching; the typing is `prompt()` as today |
| CLI | `jw prompt <workspace> [--pane <id>] [--tag] [--] <text>`; flags are read only before the first word of the text |

- REQ-247 THE CLI SHALL accept `jw prompt <workspace> [--pane <id>] [--tag] [--] <text>`, reading flags only before the first word of the text.
- REQ-248 WHEN `--pane` is absent, THE daemon SHALL choose the target as REQ-194 and REQ-196 say.
- REQ-249 WHEN `--pane <id>` names a live agent pane of the workspace, THE daemon SHALL type the text into that pane and reply `Prompted` with its id.
- REQ-250 IF `--pane` names a pane that is not in the workspace, THEN THE daemon SHALL reply `Error` and type nothing.
- REQ-251 IF `--pane` names a pane that has exited or runs no known agent, THEN THE daemon SHALL reply `Error` saying which (the texts are in `named_agent`, `daemon/src/lib.rs`), and type nothing.
- REQ-252 IF `--pane` has no number or the text is empty, THEN THE CLI SHALL fail with the usage before connecting.
- REQ-253 IF `--pane` equals `$JW_PANE_ID`, THEN THE CLI SHALL fail and send no `Prompt`.
- REQ-254 THE protocol SHALL be 11, and `Prompt.pane` SHALL default to `None` when absent.
- REQ-255 WHERE `--tag` is given and `$JW_NAME` is set, THE CLI SHALL prefix the text with `[jw: <JW_NAME>, pane <JW_PANE_ID>] `, or with `[jw: <JW_NAME>] ` when `$JW_PANE_ID` is not set (OPEN-36).
- REQ-256 THE jw skill SHALL tell an agent never to answer another agent's permission question with `jw prompt`, with or without `--pane`.

### Addendum 21 (E1–E5): `^␣ X` opens its modal at once; the PR line and the work row fill in later

`X` waited for `gh` (a network call, no timeout) before it showed anything. The modal now opens on the local git checks and only its `PR` line waits for `gh`. The PR decides done versus rm, never whether work is lost (REQ-12 reads `dirty` and `unpushed`), so it can arrive later without weakening the refusal (RAT-73, RAT-74). The prototype (approved 2026-10-11) is the screen. No message changes: `PROTOCOL` stays 11.

Measured on this checkout before the change (one call each, 3 runs; wall clock including about 30 ms of the timer's own start):

| Step | Time |
|---|---|
| `git status --porcelain` | 35–42 ms |
| `git log HEAD --not --remotes` | 35–53 ms |
| `git rev-parse --verify` (`branch_exists`) | 32–36 ms |
| `gh pr list --head <branch>` | 539–786 ms |

After (a debug build under `scripts/drive.py`, a temp repo with a local `origin` and one worktree with an untracked file): `rm_project` 37 ms, `rm_local` 50–53 ms, `^␣ X` to modal (`rm_ask` to `rm_modal`) 89–92 ms. With `JW_GH` a script that sleeps, the modal is up at the same 89 ms, its PR row and the status bar spin, text typed meanwhile is kept, and the row reads `unknown (gh failed or timed out)` at 3.0 s (`rm_gh` 3009 ms). A release build, 3 runs: `rm_project` 35–47 ms, `rm_local` 46–54 ms, ask to modal 92–94 ms, so no faster than debug: the time is the `git` processes, not jw. A large repo is not measured (RISK-80). E3 then runs the three git calls of `rm_plan` side by side: release, 3 runs, `rm_local` 21–24 ms and ask to modal 65–70 ms, with the same modal. A large repo (a synthetic one: 30,000 tracked files, a worktree with 20,000 untracked files under `node_modules/` and one more untracked file), release, 3 runs: `rm_project` 44–51 ms, `rm_local` 76–92 ms (`git status` alone 60 ms warm, 270 ms cold), ask to modal 122–140 ms: over the 100 ms of the Contract. E4 then asks the worktree's git while `Project::open` runs. The same large repo, release, six runs of each variant taken alternately on a machine with a load average of 9–19 on 8 cores (so noisy): ask to modal 88–185 ms overlapped (median 127) against 125–200 ms in series (median 148). It is faster, and still above 100 ms there; a quiet machine and a real repo are not measured.

| Piece | Shape |
|---|---|
| Jobs | One thread per `X`: `actions::worktree_state` (git on the worktree) starts on its own thread while `Project::open` runs; `App::ask_remove` itself opens the modal on the UI thread from the entry (no job, no git); the thread sends `Job::RmProject{id, project}` (which starts `ask_pr`), runs `branch_exists` while git still reads, and sends `Job::RmWork{id, plan}` (`rm_plan_from(entry, state, branch_exists)`) fills the plan; `Job::RmPr{id, line}` from `App::ask_pr` fills the PR line. `RmReady`, `RmOpen` and `DoneReady` are gone. |
| Modal | `Modal::Rm` has `project: Option<Project>` (`None` until `RmProject`) and `pending: bool` (git has not answered: `plan` is empty, the `Work` row spins, only `esc` acts) and `pr: PrLine` = `Checking`, `Unknown`, `NoPr`, `Info(String)`, `Merged{number, done}`; `Outcome::ToDone` turns it into `Modal::Done` |
| gh | `Client.timeout` (`connectors/github.rs`, `output_within`); `tui/prcache.rs`: `client()`, 30 s memory of answers per `(repo, branch)`, 3 s timeout, shared with `done_plan` |
| Spinner | `tick_motion` spins while `busy` or the PR line is `Checking`; the status bar's `⟳` and the PR line use `anim::spinner` |
| Trace | `rm_ask`, `rm_project`, `rm_local`, `rm_modal`, `rm_gh`, `rm_done_plan`, `rm_pr_dropped`, `rm_work`, `rm_work_dropped` under `JW_TRACE` (`rm_modal` is the open, `rm_work` the moment `↵` and `y` start to act) |

- REQ-257 WHEN `X` is pressed on a worktree, THE TUI SHALL open `Modal::Rm` from `rm_plan` without waiting for `gh`.
- REQ-258 WHILE gh has not answered, THE modal's PR line SHALL show an animated "checking…" and the status bar SHALL show the same spinner.
- REQ-259 WHEN gh answers, THE TUI SHALL change only the PR line: not the typed text, the required name, the key hints or any other line.
- REQ-260 IF the modal was closed, or is another worktree's, when gh answers, THEN THE TUI SHALL drop the answer.
- REQ-261 THE modal SHALL require the worktree name whenever `RmPlan::loses_work()` is true, before and after gh answers (REQ-12 unchanged).
- REQ-262 WHEN the PR is merged and done's checks pass, THE PR line SHALL offer `d` (`⇥` when the name must be typed) and the key SHALL open `Modal::Done` only while nothing is typed; THE TUI SHALL never switch modal by itself.
- REQ-263 IF the PR is merged but done's checks fail, THEN THE PR line SHALL read `#n merged` with no key and the modal SHALL stay the rm modal.
- REQ-264 IF gh fails or does not answer in 3 s, THEN THE PR line SHALL read `unknown (gh failed or timed out)` and the modal SHALL stay the rm modal.
- REQ-265 WHEN the same `(repo, branch)` is asked again within 30 s, THE TUI SHALL answer from memory with no gh call; a failure SHALL NOT be remembered.
- REQ-266 THE UI thread SHALL run no git or gh call for this flow, and gh's answer SHALL NOT reload the registry nor clear the busy indicator of another job.
- REQ-267 THE version SHALL be 0.0.20 and `PROTOCOL` SHALL stay 11.
- REQ-268 WHEN `X` is pressed on a worktree, THE TUI SHALL open `Modal::Rm` in the same key event, from the entry alone, with a spinning `Work` row, no plan and no project, before any git or project lookup; WHILE that row waits, only `esc` SHALL act (`↵`, `y`, `n`, `d`, `⇥`, letters and Backspace do nothing, and nothing is typed); WHEN git answers, THE TUI SHALL fill the plan of the same modal, with the typed name empty, and drop the answer if the modal is closed or another's; IF git or the project lookup fails, THEN THE TUI SHALL close the modal and say why; no confirmation SHALL act without a project. The prototype (version 3, approved 2026-10-12) is the screen, except that `Deletes:` is left out of the waiting modal, since what is deleted (`on_disk`, whether the branch exists) is not known yet.
- REQ-269 WHILE the remove modal would lose work, THE modal's footer SHALL read `↵ remove once the name is typed` and THE status bar SHALL say that the name must be typed first; WHILE its checks run, THE status bar SHALL say it is checking; otherwise THE bar SHALL keep `y yes · esc no`. The rule itself (REQ-12, REQ-261) does not change.

### Addendum 22 (RM1–RM3): delete a session (`jw session rm`, `^D` in the session picker)

| Item | What |
|---|---|
| Removes | the session's workspaces in the daemon (`Close`, per workspace), its registry entries, `sessions/<name>/`, the `renamed.json` pairs that lead to it or leave it, and `last` if it names it |
| Keeps | every checkout and branch on disk: the worktrees are only forgotten. `actions::rm_plan_from` is read-only here, to say which of them hold dirty or unpushed work (`crates/core/src/session.rs`, `rm_plan`) |
| Refuses | the session you are in. `main` is a session like any other: nothing keeps it, and a later `jw` with no session creates it again (REQ-64) |
| Asks | when a worktree loses work (`RmPlan::loses_work`) or a pane has an agent or is busy: the CLI wants `--yes`, the TUI the typed name |
| Daemon | one serves every session; the client sends `Close` for the target's workspaces and signals nothing. `PROTOCOL` stays 11 |
| Prototype | version 3, approved 2026-10-12 (version 2 refused `main`; version 3, where only the current session is refused, was updated with the user's correction and not re-approved) |

- REQ-270 THE `jw session rm <name>` command SHALL remove the session's registry entries, its `sessions/<name>/` directory, the `renamed.json` pairs that lead to it or leave it, and `last` when it names it.
- REQ-271 IF the name is the session the caller is in, THEN `session::rm` and the command SHALL refuse, exit non-zero and say why; `main` SHALL be removable like any other session.
- REQ-272 IF the name is not an existing session, or is not a valid name, THEN the command SHALL exit non-zero and delete nothing.
- REQ-273 THE removal SHALL NOT run `git worktree remove`, delete a branch or touch a path outside the state dir.
- REQ-274 WHEN a worktree of the session has dirty or unpushed work, or a pane has an agent or is busy, THE command SHALL list them and exit non-zero without deleting unless `--yes` is given; a worktree git cannot read SHALL count as at risk.
- REQ-275 WHEN `--yes` is given or nothing is at risk, THE command SHALL send `Close` for each workspace of the session and then remove its files.
- REQ-276 WHEN `^D` is pressed on a session in the picker, THE TUI SHALL open a confirm modal naming the session and what is at risk.
- REQ-277 WHILE something is at risk, THE modal SHALL accept `↵` only when the typed text equals the session's name; otherwise `y` and `n` SHALL do.
- REQ-278 IF the selected session is the current one, THEN `^D` SHALL close the picker, say the reason in the status bar and open no modal.
- REQ-279 THE removal SHALL send `Close` only for workspaces of the target session and SHALL NOT signal any daemon.
- REQ-280 THE picker's footer SHALL list `^D delete`.
- REQ-281 IF no daemon runs, THEN the removal SHALL go on with the files alone.
- REQ-282 THE `session` word SHALL be reserved: no session can be named `session`.
- REQ-283 THE version SHALL be 0.0.21 and `PROTOCOL` SHALL stay 11.
- REQ-284 WHILE the plan is read, THE TUI SHALL run no git on the UI thread; the modal opens when `session::rm_plan` answers, and the picker reopens after a removal.

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
- RAT-12 **The daemon answers queries, not the TUI** (S14). It owns the PTYs, agents run with no client attached, and programs wait briefly for an answer (nvim about 100 ms); a round trip through a client is late or missing.
- RAT-13 **The TUI's palette is the source, not the outer terminal.** Panes are drawn on `p().bg`, so that is the colour a program sits on. Custom themes (S8) change fg/bg, so the message carries the colours, not only `dark`.
- RAT-14 **The theme persists in `session.json`** because restored agents (REQ-74) ask before any TUI reconnects.
- RAT-15 **Keyboard flags in-band, no `PROTOCOL` bump.** DECCKM already reaches the TUI this way: its own parser reads the raw output and the Snapshot replays the mode. An old TUI on a new daemon ignores the flags and sends legacy bytes, as before. A new TUI on an old daemon only sees flags a program pushed without asking first, and CSI u is then what that program wants. Neither side misreads the other.
- RAT-16 **The daemon answers `CSI ? u`.** nvim and crossterm programs (codex) query before they push and stay legacy without an answer. The TUI may be detached, and two TUIs would answer twice. It rides S14's query queue, which already answers `CSI c`.
- RAT-17 **Flag 1 only, and a short encode table.** Shift+Enter needs only disambiguate. Full kitty encoding (release events, every key as an escape, associated text) is a large surface for no visible gain; programs that push more flags (nvim pushes 3) still parse legacy bytes.
- RAT-18 **Push after `ratatui::init()`.** The kitty spec keeps separate stacks per screen, so the push lands on the alternate screen's: a panic that skips the pop leaves the user's shell without the flag once the alternate screen is left.
- RAT-19 **How herdr does it** (v0.8.2, read from source). The same three pieces, with bigger parts. The outer terminal gets 1+2+4 (31 when a pane asks for report-all, switched with pop+push because old iTerm2 clears the stack on set; `src/terminal_modes.rs`). Panes run on libghostty-vt, which tracks the flags, answers every query through a `write_pty` callback and encodes keys with Ghostty's encoder (`src/pane/terminal.rs`). Legacy Shift+Enter is `\r` there too (`legacy_shift_enter_is_just_cr`, `src/input/encode.rs`), and `KittyKeyboardTracker::replay_ansi` (`src/pane/kitty_keyboard.rs`) is the replay shape REQ-97 copies. Rejected: swapping vt100 for libghostty-vt, which would vendor a Zig/C library and rewrite both parsers for one key. Rejected: always sending `ESC \r`, which claude takes as a new line but nvim and codex read as Alt+Enter, and bash gets a stray ESC.
- RAT-20 **Built-ins are embedded JSON, not runtime files** (S16). They must work with no config dir and no install step; `include_str!` keeps one binary while making them the same kind of file a user writes, and worked examples of the format.
- RAT-21 **A user's file wins over a built-in.** It is the simplest way to tweak one (`jw theme export one-dark > ~/.config/jw/themes/one-dark.json`); reserving the names would need an error path and a rename for nothing.
- RAT-23 **The user builds the layout; jw ships no presets.** The user rejected a choice of three fixed shapes: the layout is theirs to decide, panes, places and contents. The builder starts from one shell, which is also the default, so jw never assumes an editor or an agent.
- RAT-24 **`^␣ p` maps roles back to runs instead of keeping `run` in the live tree.** `PaneLeaf` holds `{id, role, name}`; the opening tree already says which run gave each role. Adding `run` to `PaneLeaf` would bump `PROTOCOL` for one action.
- RAT-25 **`[layout]` is rewritten as text, not with `toml_edit`.** jw writes that table itself, and only five keys of it change. A new dependency (and a second TOML model) for one write is not worth it.
- RAT-26 **Selection in the TUI, not the daemon.** It is per viewer (two clients can select different things), changes nothing on the wire, and needs no `PROTOCOL` bump. The TUI already holds each pane's vt100 screen and scrollback.
- RAT-27 **Rejected: turn host mouse capture off while the focused pane has no mouse mode.** It would give the host terminal's selection back, but jw needs the mouse for sidebar clicks, pane focus, border resize and the wheel, and capture is one switch for the whole screen.
- RAT-28 **Rejected: a keyboard select mode (`^␣ v`, chip `SEL`) and Shift + arrows from the shell's cursor.** A mode is tmux's model and terminals today do not need one; Shift + arrows would take those keys from programs. Editing the prompt's own text belongs to the shell's line editor (zsh), which jw cannot see. The mouse does the selecting (RISK-6).
- RAT-29 **A frame deadline only while animating, not a fixed 60 Hz loop.** The loop is event-driven (`event_loop`) and coalesces into 16 ms; a 60 Hz tick would burn CPU and battery all day next to a daemon running real agent work. `EDGE_EVERY` and the which-key wait already use `recv_timeout` the same way.
- RAT-30 **Positions come from `Instant`, not a frame counter.** When a frame is late the animation catches up instead of running slow, and REQ-148's interrupt can start from the displayed value.
- RAT-31 **Welcome state in `settings.json`.** It already holds the user's choices and is re-read on change (REQ-69); a second file for one boolean is not worth it.
- RAT-32 **The which-key delay drops to 150 ms.** A new user needs the keys on screen; an expert who types the chord never sees the popup either way, and `which_delay_ms` still sets it.
- RAT-33 **Rejected: easing in the daemon or on the wire.** Animation is per viewer and cosmetic; putting it in `PaneInfo` would bump `PROTOCOL` for nothing (RAT-26's reasoning).
- RAT-34 **W1: poll the mtime on the `Tick`.** The writers are the CLI (`cli.rs:186,261`) and other TUIs; the daemon writes none of these files and `jw worktree` works with no daemon. A push needs the daemon to watch files it never opens, or every writer to call it first. The TUI already polls `settings.json` this way on the same tick (REQ-69), so this is a second `Watch`, not a new mechanism.
- RAT-35 **W1, rejected: a daemon message.** `PROTOCOL` 9, a new `DaemonMsg`, a daemon-side watcher or a CLI that connects to notify, and the sessions problem (one daemon serves every session). All of it for the same latency.
- RAT-36 **W1, rejected: reload on focus.** The case that hurts is an agent in a pane of this same TUI running `jw worktree`; the terminal never changes focus. Focus events are not requested today (`Msg::Term(_) => {}`) and need `EnableFocusChange`.
- RAT-37 **W1, rejected: a notify/fsevent crate.** A dependency and a thread for two files, when `Tick` already wakes every 500 ms and two `stat`s per 2 s cost nothing measurable.
- RAT-38 **W1: the live rebuild is read-only.** `reload` runs `fill_roots` and saves (`tui/mod.rs:864`); on every external change that read-modify-write would race the writer that triggered it (no lock) and could drop its entry. Startup and the TUI's own jobs keep it.
- RAT-39 **Context by file.** `prompt()` waits for 1200 ms of quiet and caps at 20 s (`daemon/src/lib.rs:1426`); a thread plus a diff pasted in one go is slow and spends the agent's context before it chooses what to read. A short prompt and a file lets claude read the diff from git, always current.
- RAT-40 **`pull/<n>/head`, not `gh pr checkout`.** gh checkout changes the current worktree and its tracking; a fetched named branch plus the existing worktree path keeps the registry, slot and setup rules, and works for forks.
- RAT-41 **`review_base` on `Entry`.** `Stream.base` is only ever git's `origin/HEAD` (`stream.rs:64,78`) and `Config` has no `base` key. One field gives the diff its base (REQ-169) and keeps a closed, then reopened, review in its shape (REQ-168), because `resolve` takes only an `Entry`. The cost is a registry field that older readers ignore.
- RAT-42 **A fixed layout.** `[layout]` is for writing code (editor and agent); a review wants the diff and the agent, half each so a wide diff is readable. A `[review]` table can come later.
- RAT-43 **`origin/<base>` is fetched explicitly.** `diff::load` fails on a missing ref (`diff.rs:80-90`); only an empty base falls back to `HEAD`.
- RAT-44 **No new message.** Everything rides on `Open{tree}`, `view:diff` leaves (daemon-tested in `crates/jw/tests/daemon.rs`) and `Prompt`. The one core change that makes it work is `command()` returning no command for `view:*`.
- RAT-45 **The TUI sends the prompt, not the CLI.** The picker lives in the TUI; one pending slot on `App` costs less than a new path.
- RAT-46 **Capital `R`.** Free in `ACTIONS`; `r` is rename and `hjklHJKL` are fixed.
- RAT-47 **The picker is a `Finder` kind.** It has query, selection, preview and clicks. Its items come from a job, which no kind did before, so `Finder` gains a loading state.
- RAT-63 **General is committed work only.** Until now one scroll held both, which is the complaint. This way General is all of Commits together and Uncommitted is the rest, so no file appears in a view for two reasons. The cost: `^␣ d` no longer shows dirty files first; they are one key away (`2`). Decided with the prototype.
- RAT-64 **A sidebar, not tabs.** The user's call (2026-10-11): views, commits and files are one list in one place. It spends width, so under 100 columns it becomes an overlay with a breadcrumb instead of tabs.
- RAT-65 **Scope is client state.** Putting it in the role (`view:diff:commits`) touches saved layouts, the four hard-coded `view:diff` sites (`stream.rs:46`, `tui/mod.rs:1588`, `stream.rs:552`, `jw/tests/daemon.rs:491,506`) and the tests, and the daemon has nothing to do with it. Reopening on General (REQ-245) is the price.
- RAT-66 **Lazy, per view, with a generation.** Only General loads on open, so opening costs what it did minus the untracked walk. A result is applied only if no newer request exists for its slot (REQ-236), so fast `(` `)` never apply out of order. The caps (200 commits, 2 MiB) are pages, not measurements: see OPEN-32. Measured on a repo with a 4,000-file, 240,000-line change: the capped diff, the file list and 201 commits each took 30–100 ms of git.
- RAT-67 **Viewed by path.** `viewed` held indices and `Job::Diff` rebuilt the view (`tui/mod.rs:3239`), so a reload lost scroll and viewed. Paths survive a list that changes between reloads.
- RAT-22 **`curl`, not a crate,** for `install <url>`: rare, and jw already shells out to `git` and `gh`; `ureq` would add TLS and about 40 crates for one command.
- RAT-48 **Measure before fixing.** S1–S6 are from reading the code; the cost of each is a guess. F0 exists so F1–F4 each cite a number. Parts F3/F4 are conditional for that reason.
- RAT-49 **Parse in the reader thread** keeps `PROTOCOL` at 8 and needs no daemon change; the fix moves work, it does not remove it, so F0 must show that the parse really is what blocks a frame.
- RAT-50 **`Parser` is `Send`** (plain data), so building it in the existing daemon-reader thread (`mod.rs:327`) is sound. Output chunks that arrive for a pane still being built must queue behind its Snapshot (order matters).
- RAT-51 Not warming every workspace in the TUI (subscribing to all): it would multiply `Output` parsing on the UI thread by the number of workspaces. Rejected unless F0 says Attach is the only cost.
- RAT-52 **A1: the program is `fg`, not a new field.** The daemon already resolves it per pane on `List`, and a classified copy in `PaneInfo` would be a second place to drift and a `PROTOCOL` bump for nothing. Each crate asks `is_agent_program(fg)`.
- RAT-53 **A1: detection only.** Detection covers the case that hurts, `claude` started by hand, which nobody would declare. A declaration (`jw pane program`) would also need a rule for when it expires, or `jw prompt` would type into a shell after claude exits. Added later if detection fails in practice.
- RAT-54 **A1, rejected: `jw prompt --pane`.** With one agent per workspace the role-then-program order is enough. A second agent pane is the case that would need it (RISK-63).
- RAT-55 **A1: no timer.** `fg` is read on `List`, which the TUI already sends every 2 s, and on `Prompt`. Leaving claude is seen at the next `List`; the daemon's only loop stays the 30 s scrollback save.
- RAT-56 **A1: the list is fixed in code.** A project-config list is more surface than five names; adding one is a one-line change.
- RAT-57 **A1, variant B of the prototype.** The existing dim suffix already says what runs; the only thing to add is which programs are agents, so the name turns magenta (the colour of `✻`) and nothing else moves. Rejected: a glyph before the name (variant C) adds width to every agent title.
- RAT-58 **Addendum 18: the id comes in a reply.** A `Split` answered nothing; the id was in the `Tree` broadcast. A client could diff the tree before and after, but that races with the TUI or another agent splitting at the same moment. One message and a `PROTOCOL` bump (REQ-206) is the price the rules already set.
- RAT-59 **Addendum 18: `Opened`, not `Spawned`.** `Spawned` says "you are subscribed to this pane"; reusing it would have the TUI think it watches a pane it does not.
- RAT-60 **Addendum 18: `Split` stays as it is.** The TUI's path is tested; the CLI only supplies the `NewPane`. Rejected: a new message for the CLI.
- RAT-61 **Addendum 18: verbs under `pane`.** Like `worktree`; it leaves room for `pane send`. Rejected: panes in `jw ls`, which stays one row per workspace.
- RAT-62 **Addendum 18: no new boundary for `--run`.** `jw prompt` already lets any process of the same user type into an agent, which is arbitrary execution; the socket (directory 0700, file 0600, `daemon/src/lib.rs`) is the whole fence. A token would be its own feature. The skill says so (REQ-217).

- RAT-68 **Addendum 20: the daemon validates `--pane`, not the CLI.** REQ-215 checks in the CLI and the daemon trusts the id (`Kill`), so a raw client crosses workspaces. The daemon already has `panes_of(stream)` in the same call, and a pane can die between `pane ls` and the prompt: the only check that holds is the one next to the write. The cost is a changed `ClientMsg`, which needs `PROTOCOL` anyway: an old daemon would ignore `pane` and type into another one.
- RAT-69 **Addendum 20: `--pane` only before the text.** The text is the remaining words (`cli.rs` `prompt`), and `jw prompt ws fix the --pane flag` must stay text. Reading the option only right after the workspace keeps every current use except text that starts with `--pane`, `--tag` or `--`, which `--` escapes. Rejected: `--pane` anywhere.
- RAT-70 **Addendum 20: role `agent` or an agent program.** The same test as REQ-194, so a pane opened with `--run claude` works whatever its role. A viewer has no program and is refused.
- RAT-71 **Addendum 20: `--tag` is opt-in and CLI-only.** Always prefixing changes the text of every prompt today and clutters what a person types from their pane. Opt-in needs no protocol. The prefix is text, not authentication.
- RAT-72 **Addendum 20: refuse your own pane.** `prompt()` waits for quiet and the sender's pane is busy running the command: it would wait 20 s and paste into itself. Same stance as REQ-219.

- RAT-73 **Addendum 21: modal first, PR as a line.** Local facts decide whether work is lost; the PR only chooses the confirmation (done or rm). Waiting 0.5–0.8 s (and unbounded offline) for the less important fact was the whole delay.
- RAT-74 **Addendum 21: the early modal is always the rm modal.** It is the stricter one, so a late MERGED can only add a way out that the user takes with a key, never loosen the prompt under their fingers. `d` is a letter of the name when the name must be typed, so there the key is `⇥`.
- RAT-75 **Addendum 21: the timeout in core, the memory in the TUI.** A timeout belongs to the call (`Client.timeout`, off by default so the PR picker is unchanged); a TTL belongs to the session, and core keeps no state.
- RAT-77 **Addendum 21: show the modal before git answers, and hold the keys.** `git status` sets the time on a large repo, so it cannot be made faster from jw. Opening the modal when the project is open shows it sooner, but the user cannot act sooner: the gain is that the screen answers `X`, not that a removal starts earlier. The keys wait instead of being queued: a letter typed into a prompt that is about to ask for a name, or an `↵` landing on a plan that turns out to lose work, is the case REQ-12 exists to prevent.
- RAT-78 **Addendum 22: forget the worktrees, never delete them.** Removing a session is bookkeeping; `X` is the way to delete a checkout and it asks about each one. A session delete that ran `actions::rm` per worktree would be a bulk `X` behind one confirmation. The cost is checkouts that jw no longer lists; the CLI and the modal print their paths.
- RAT-79 **No new message.** The client already lists workspaces and closes them by id (`Close`), so `PROTOCOL` stays 11. A `SessionRm` in the daemon would put file deletion where the daemon does not know sessions (addendum 3) and would be refused by an older daemon for no gain.
- RAT-80 **`^D`, not a letter.** In the picker a letter types into the filter. `^D` sits beside `^R` rename and is not bound there.
- RAT-81 **`main` is removable.** It was assumed to be a default session. It is not: `session::rename` already renames it (`session.rs` tests), and `last` falls back to it only because `jw` creates it when none exists.
- RAT-76 **Addendum 21: not chosen.** Fetching the PR when the sidebar selection moves (a `gh` per cursor step); a daemon-side cache (a message and `PROTOCOL` for no gain); parallel `status` and `log` (kept as the fallback of RISK-80).

## Risks

- RISK-1 **Emulation fidelity** (Claude Code, nvim: alt screen, mouse, OSC 52, bracketed paste) decides whether the TUI works. Checkpoint after Q2: a week of daily use; failing it reopens RAT-10.
- RISK-2 **Go rejects unknown keys.** Resolved in P0: `rustOnly` skips `[layout]` children and `[tui]`.
- RISK-3 **"Agent ready" without `herdr agent wait`:** the daemon waits for 1.2 s of quiet (at most 20 s). An agent stopped at a dialog is quiet too, so the prompt lands in the dialog.
- RISK-4 **Daemon crash = all terminals die.** REQ-15 limits the damage; in-flight state is lost.
- RISK-5 **Concurrent writes to `registry.json`** between Go and Rust, with no lock. Last writer wins until the cutover.
- RISK-6 **Scope creep toward tmux** (copy mode, scrollback search). Scope: wheel scroll, mouse selection that copies, with a banner (addendum 12). No search, no buffers.
- RISK-7 **`✻` / `?` are a heuristic** from output activity, BEL and the OSC title. Worst case they show `●` only.
- RISK-8 **Animations vs. PTY output** can raise CPU. Animate only during transitions.
- RISK-9 **Bytes go as JSON number arrays** and a slow client's queue has no bound. Switch to a binary codec and backpressure if dev logs make it show.
- RISK-10 **macOS leaks sockets into children** forked by other threads; tests wait for that child to exit.
- RISK-11 **Setup runs in the shell pane** of a new worktree, so a failed setup does not stop it from opening.
- RISK-12 **dev types into the shell pane** unless `[layout]` has a `dev:<svc>` leaf. With no dev key in v2, a project's dev servers should be `dev:<svc>` leaves.
- RISK-13 **syntect's bundled grammars have no TypeScript or TOML** (ts uses JavaScript; TOML is plain).
- RISK-14 **Protocol change (RAT-9):** an old daemon and a new client disagree. The version check on attach says "restart the daemon" instead of misbehaving.
- RISK-15 **Rename under running processes** leaves shells with a stale `$PWD`. REQ-111 keeps them running anyway (the user's choice): their cwd follows the folder, but a program that kept the old path as text (claude's project dir, an editor's buffers) can break, and the modal says so.
- RISK-16 **`↵` in the diff sends `<Esc>:e path<CR>` to nvim**, assuming normal mode after Esc. Fine for v2; `nvim --server` later if it bites.
- RISK-17 **Mapping worktrees to their project** with `Repo::open` runs one `git` per worktree on every reload. Cache it by path; it changes only when worktrees are created or removed.
- RISK-18 **The `?` mark depends on the agent ringing the bell.** Claude Code does so only when its notifications use the terminal bell. Without that, `?` never shows (the same worst case as RISK-7).
- RISK-19 **`gh` polling** costs a network call per worktree a minute. Only open worktrees are polled, and only while jw runs.
- RISK-20 Resolved: `claude --settings` hooks merge with the user's and project's hooks (checked with claude 2.1.296). A claude started by hand in a shell pane has no jw hooks and falls back to REQ-51's guess.
- RISK-21 **A replayed ring at a new width** wraps differently; fine for scrollback.
- RISK-22 **A folder open in two sessions** is two workspaces sharing nothing. A worktree belongs to the session that made it.
- RISK-23 **`workspaces.json` drifts from Go's registry** after the seed; `jw import` if it is ever needed.
- RISK-24 **`jw new` changes meaning** (worktree → session). `jw new --task` errors with a pointer to `jw worktree`.
- RISK-25 **Claude Code and Codex may read the background only at startup.** If they don't subscribe to mode 2031, a switch reaches them only after a restart. To check by hand.
- RISK-26 **Replies always end with ST:** vt100 doesn't pass `bel_terminated` to its callback. Every parser we know of accepts either.
- RISK-27 **Lock order:** the reader drops `state` before taking `io` to reply, as `set_theme` does per pane. Holding both could deadlock against `Input`.
- RISK-28 **Several TUIs in terminals of different appearance:** the last to connect or switch wins (OPEN-8). DECRQM is answered for 2031 only.
- RISK-29 **A program that dies without popping its keyboard flags** (killed by a signal) leaves them set, and the next program in that shell gets CSI u for modified Enter, Tab and Esc. A real terminal behaves the same. If it bites: clear the stacks when the shell prints its prompt (OSC 133).
- RISK-30 **The Snapshot replays only the active screen's stack.** A client that attaches during a full-screen program relies on the ring for the main screen's stack, which matters only after 2 MiB of output dropped those pushes.
- RISK-31 **With flag 1 on the outer terminal**, crossterm reports Ctrl+letters as `Char+CONTROL` instead of the legacy aliases (Ctrl+H is no longer Backspace). `encode` sends the same bytes either way, and no TUI binding matches an alias (checked in S15).
- RISK-32 **The built-ins' hex values are transcribed by hand** from each project's published palette. A wrong value is cosmetic and a JSON edit; the test only checks that they parse.
- RISK-33 **`install` from a URL runs `curl` on what the user typed.** The body is only parsed and written under `themes/`, never run, and the name must match `[a-z0-9-]+`, so it can't leave the directory.
- RISK-34 **Shadowing hides a built-in.** `jw theme ls` says `file, replaces built-in`, and `install` says so too.
- RISK-35 **A renamed worktree's claude resumes in the new folder** after a daemon restart, while its transcript sits under the old path's project in `~/.claude/projects/`. `claude --resume <id>` may not find it there; the pane then shows claude's error and a new `claude` starts clean.
- RISK-36 **Setup before nvim or the agent:** a failing step's error scrolls away when the program takes the screen. The program still starts, and quitting it shows the error.
- RISK-37 **`save_layout` drops comments inside `[layout]`** and any `[layout.*]` subtable (jw writes none). Comments elsewhere stay.
- RISK-38 **The default layout changes** for projects with no `[layout]`: they open with one shell instead of nvim | agent over a shell. jitsubai and jw have their own layout; others get the setup step on their next first worktree only if they have no config at all.
- RISK-39 **OSC 52 depends on the outer terminal.** iTerm2 asks to allow clipboard access; Terminal.app does not support it. The copy may do nothing, with no error. The banner (REQ-135) says "Copied to clipboard" (it only reports what jw sent) and refuses more than 75 KB (`select::MAX_COPY`), since some terminals drop an OSC 52 over about 100 KB once encoded.
- RISK-40 **A selection holds lines, not text.** Rows are counted from the oldest line of the history, so output that scrolls the pane leaves the selection on its text (REQ-127, REQ-134). A full history drops its oldest line and shifts every index, so REQ-131 clears it then; a program that redraws in place (a progress bar) changes the text under it, and the copy takes what is on screen.
- RISK-41 **Shift + drag may never reach jw.** Terminals of the xterm family (iTerm2, Terminal.app) keep Shift + mouse for their own selection while an app has mouse reporting on, which is how it worked before this addendum. Then REQ-125 never fires and the host selects, as today. Checked by hand 2026-10-10: in Ghostty, Shift + drag over an nvim pane does not select and nothing reaches jw, so REQ-125's Shift path does not fire there. iTerm2 and Terminal.app are not checked. In a pane whose program uses the mouse, the way to copy is to quit its mouse mode or use the program's own selection.
- RISK-42 **Fades need truecolor.** Interpolating between theme colours on a 256-colour terminal bands or picks the wrong shade. REQ-138 applies changes at once there, so the cost is no motion, not wrong colours.
- RISK-43 **A late frame.** Drawing the panes' content shifted by a few cells (REQ-136) re-renders every pane each frame. REQ-148 covers a frame that overruns `FRAME`: the animation jumps to its end state instead of lagging behind input. The cost is measured by hand in Ghostty.
- RISK-44 **Synchronized output can freeze the screen.** A terminal holds the frame until `CSI ? 2026 l` (Ghostty gives up after a second). REQ-139 closes it on every exit path; terminals that do not know the mode ignore it.
- RISK-45 **The shorter which-key delay shows the popup to people who type fast.** A chord typed within 150 ms still never shows it; one typed slower flashes it for an instant, softened by the fade. `which_delay_ms` restores the old 600 ms.
- RISK-46 **`reload` blocks the UI today**: file reads on the main thread, plus a `git` per entry without `root` (`actions.rs:51`). REQ-156 moves the live path off it; the legacy calls (`on_job`, `close`, `submit_modal`) stay inline.
- RISK-47 **Rows replaced under an open overlay.** A modal or `Finder` may hold a row index rather than an entry; verify in `modal.rs` and `finder.rs`. REQ-154 defers instead of proving it.
- RISK-48 **Stamp drift.** A write between our read and our re-stamp is missed until the next write. Stamp before reading, not after.
- RISK-49 **mtime granularity.** Two writes inside one filesystem timestamp tick look like one; a read after the second still sees both. Accepted.
- RISK-50 **Lost updates between writers stay** (no lock, `registry.rs:161`; `session::rename` and `actions::rename` read-modify-write). Out of scope: W1 makes them visible, not likelier.
- RISK-51 **A `view:diff` leaf in `Open` had never been sent by the TUI.** The daemon's `open` started a shell for it (`start` skipped the PTY for views but `open` called `spawn`): fixed, REQ-181, tested in `crates/jw/tests/daemon.rs`. `Stream::panes` gives the leaf no command (`command()`), and `open_specs` keeps the setup out of it.
- RISK-52 **The prompt can land in claude's trust-folder dialog** on a fresh worktree: `prompt()` never inspects the screen and writes text and Enter. The same exposure as `jw worktree --task`; what claude does with it is unverified. Accepted for now.
- RISK-53 **`gh pr list` takes seconds** on a big repo or a slow network. REQ-161 keeps the keys live; a cached list is a later refinement.
- RISK-54 **Context size.** A PR with hundreds of comments is capped at 200 KB and the cut is noted in the file; claude can run `gh pr view` itself.
- RISK-55 **Fork PRs** fetch fine but cannot be pushed to; fine for a review.
- RISK-56 **`gh api repos/{owner}/{repo}/…`** resolves the repo from the cwd; checked against a fake `gh` only until the manual test.
- RISK-60 **`ps -o comm=` may not say the agent's name.** On this machine it prints `claude` (checked 2026-10-11), but a wrapper, `node` or a version-named binary would not match. Fallback if seen: match `ps -o args=`.
- RISK-61 **A hand-run agent has no hooks.** No `--settings`, so no `jw hook`; its sidebar marks come from the `busy` and `bell` guess in `marks()`, wrong while it waits quietly.
- RISK-62 **The pane can change between the lookup and the write.** `prompt()` writes to the PTY; if claude exits in between, the text lands in the shell. The window is the time between two calls; `Prompt` reads `foreground` right before writing.
- RISK-63 **Two agent panes in one workspace.** Role `agent` wins, then the lowest pane id; there is no way to choose the other (RAT-54).
- RISK-64 **`ps` once per candidate pane on a `Prompt` with no role `agent` pane.** A workspace with many shell panes spawns that many. Accepted: it is rare and off the UI thread (`Prompt` runs on its own thread).
- RISK-65 **Many panes.** An agent can open as many as it likes; the daemon has no cap. Left for later.
- RISK-66 **An agent closes its own pane.** The CLI refuses it by `$JW_PANE_ID` (REQ-219), which `--force` overrides.
- RISK-67 **A workspace the TUI does not show.** The TUI applies a `Tree` only for its current workspace (`tui/mod.rs:1383-1388`); another's new pane should appear when it attaches it on a switch. Not tested.
- RISK-68 **A relative `view:md:<file>`** resolves in the TUI against the workspace it shows, so the CLI makes it absolute against its own directory, and refuses a file that is not there (REQ-221): found by a hand test, where a missing file left a viewer on `starting…`.
- RISK-69 **A split into a small pane.** `Split` gets 80×24 and the TUI resizes after; a pane too small for its sibling is the daemon's case, as for the TUI's own splits.
- RISK-70 **`origin/<base>` can be stale.** General and Commits use the local ref, as the diff did; a stale base shows an old range until `git fetch`.
- RISK-71 **`git::output` has no timeout and does not stream** (`connectors/git.rs:31`). A hung git ties one worker thread and its view spins; the UI stays live. REQ-238 needs a bounded reader in core and a `wait` after the kill so no zombie stays.
- RISK-72 **`↵` from Commits opens the working-tree file** (REQ-240), which may no longer have the lines the commit showed. Accepted.
- RISK-73 **Uncommitted goes stale while the pane is open;** there is no watcher. See OPEN-31.
- RISK-74 **Renames are per range.** `-M` runs per call, so General can show `R` for a file that Commits shows as `D` and `A`.
- RISK-75 **`App::on_job` runs `reload()` first for every job** (`tui/mod.rs`): a registry read, and a write when roots are filled, on the UI thread. Fine for the rare jobs it was written for, not for one per `)`; a diff pane's answer returns before it. Any other frequent job needs the same.
- Closed by addendum 3: RISK-5 (own registry), RISK-7 (hooks), RISK-9 (frames), RISK-16 (`nvim --server`), RISK-17 (`root` in the registry).
- RISK-57 Moving parse to another thread can reorder `Snapshot` and `Output`. Keep one reader thread, parse inline there, preserve order.
- RISK-58 Keeping the old frame during loading (REQ-189) changes what the user sees; see OPEN-23. Closed with REQ-189: nothing is built.
- RISK-59 F0 numbers on this Mac may not predict slower terminals or SSH; the budget table is a target, not a guarantee.

- RISK-76 **`PROTOCOL` 11 and the version collide with `feat/mobile-app`.** Whoever merges second goes past the other: `PROTOCOL`, the Cargo version, and these ids.
- RISK-77 **The daemon does not know whether the target agent is waiting on a permission.** With `--pane` an agent could answer another's question; only the skill rule (REQ-256) stops it, not the code. A `waiting` check in the daemon is a separate change.
- RISK-78 **Pane ids change when the daemon restarts.** Within one daemon ids come from `next_pane` and are never reused (`new_id`), so `--pane 3` cannot reach a pane opened later. `restore` gives every restored pane a new id, so an id kept across a restart (in a script, in an agent's notes) can name another pane or none; run `jw pane ls` again.
- RISK-79 **`Error` no longer tells "no agent" from "bad pane" except by its text.** Clients only show it (`cli.rs` `send_prompt`, `tui/mod.rs`), so nothing depends on it.

- RISK-80 **The modal opens within 100 ms on most runs of a large repo; acting on it does not.** After E5 (release, 6 runs, load average 8–9): modal 60–134 ms (median about 85), work row filled 82–188 ms (median about 135), against a single 127 ms before. These numbers are from the E5 build that opened after `Project::open`; the modal now opens in the key event itself (the follow-up commit), so only the time to act, bound by `git status`, is left. Release: 59–70 ms on a tiny repo, about 127 ms (median, 88–185) on 30,000 tracked and 20,000 untracked files, where `git status` sets the time. `rm_local` and `rm_modal` in `JW_TRACE` say. Next step: read what `Project::open` needs from `.git` like `head_of` does. RISK-80's earlier text (before E5) said 59–70 ms on a tiny repo and about 127 ms on the large one.
- RISK-81 **`rm` still does not re-check dirty and unpushed at confirm time (`actions::rm`).** The window between the modal and `↵` is as before; this addendum neither widens nor closes it.
- RISK-82 **A remembered answer can be 30 s old.** The PR may have been merged in the browser meanwhile. It only changes the line and the offer of `d`, and `done` re-checks the merge itself.
- RISK-83 **Forgotten worktrees are untracked.** Nothing lists a checkout once its entry is gone. Mitigation: the CLI and the modal print the paths kept; `git worktree list` still shows them.
- RISK-84 **No lock on the registry** (RISK-50): a writer that loaded `workspaces.json` before `session::rm` can write the entries back. The file is written with a temp file and a rename, so it is never half written.
- RISK-85 **Agent and busy state is a snapshot.** A pane can start work between the confirmation and `Close`.
- RISK-86 **Deleting the session other panes still point at.** A pane started in a removed session keeps `$JW_SESSION`; `session::current` then names a session that has no directory. Its renames are dropped (REQ-270), so it reads as that name.

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
| S0 ✓ `d1b0d80` | Addendum 3 in this spec; prototype v7 (sessions) before the UI parts | — | docs |
| S1 ✓ `dadd1d5` | `workspaces.json` with `session` and `root` | 70 | `core/registry.rs`, `tui/mod.rs` |
| S2 ✓ `77b288e` | `.md` reuse, `ClientMsg::Role` | 71 | `proto`, `daemon`, `tui/mod.rs` |
| S3 ✓ `26ab93c` | Binary Output frames, bounded queue, `PROTOCOL 5` | 76 | `proto`, `daemon`, `client.rs` |
| S4 ✓ `3583a15` | Scrollback ring: `Snapshot`, saved, replayed | 72 | `daemon`, `tui/mod.rs` |
| S5 ✓ `e51c3be` | Claude session ids, hooks, `jw hook`, `ClientMsg::Agent` | 73, 74 | `stream.rs`, `daemon`, `cli.rs`, `tui/mod.rs` |
| S6 ✓ `00adf1b` | nvim `--listen` / `--server --remote` | 75 | `stream.rs`, `tui/mod.rs` |
| S7 ✓ `4be4d25` | Sessions: storage, daemon, `jw new/[name]/sessions`, migration, title, `^␣ a` | 61–65 | `daemon`, `proto`, `folders.rs`, `cli.rs`, `tui/**` |
| S8 ✓ `bbf27ca` | `settings.rs`, JSON themes, reload, keymap by action | 68, 69 | `settings.rs`, `theme.rs`, `tui/**` |
| S9 ✓ `c054635` | Settings screen `^␣ ,` | 67, 68 | `tui/settings_view.rs`, `tui/draw.rs` |
| S10 ✓ `e902627` | `jw ls`, `jw read`, `jw worktree`; the jw skill rewritten | 66 | `cli.rs`, `main.rs`, skill |
| S11 ✓ `27c3c2a` | `jw help`, `--help`; the jw skill moves into the repo, `jw skill [install]` | 77, 78 | `main.rs`, `help.rs`, `skill.rs`, `skill/SKILL.md`, `session.rs` |
| S12 ✓ `7495afb` | The daemon saves everything on SIGTERM, SIGINT and SIGHUP | 79 | `daemon` |
| S13 ✓ `4f7e4a4` | `jw server status`, `jw server stop` | 80 | `server.rs`, `help.rs`, `session.rs` |
| S14 ✓ `b98d8a9` | The daemon answers colour, scheme, cursor and attribute queries; mode 2031; `ClientMsg::Theme`, `PROTOCOL 7` | 81–89 | `proto`, `daemon`, `theme.rs`, `tui/mod.rs` |
| S15 ✓ `8d2f2f0` | Addendum 5: Shift+Enter, the kitty keyboard protocol | 90–100 | `proto/kitty.rs`, `tui/keys.rs`, `tui/mod.rs`, `daemon` |
| S16 ✓ `b184b46` | Addendum 6: themes as JSON, ten built-ins in `crates/tui/themes/`, `schema.json`, user files replace built-ins, `jw theme ls/install/export/use` | 101–109 | `theme.rs`, `crates/tui/themes/`, `crates/jw/src/theme.rs`, `session.rs`, README |
| W0 ✓ `9e944c6` | Addendum 7 in this spec; the prototype | — | docs |
| W1 ✓ `d6c2728` | `^␣ w` asks the name: `actions::new_plan`, `Modal::New` | 110 | `actions.rs`, `tui/modal.rs`, `tui/mod.rs` |
| W2 ✓ `1a5d5ad` | Rename keeps the panes: `ClientMsg::Moved`, `PROTOCOL 8` | 111 | `proto`, `daemon`, `tui/mod.rs`, `tui/modal.rs` |
| W3 ✓ `f575bca` | `^␣` + arrows walk the sidebar; the `MOVE` chip | 112 | `tui/mod.rs`, `tui/draw.rs` |
| D1 ✓ `21b4e27` | Addendum 8: `X` accepts a HEAD already in the default branch | 113 | `actions.rs` |
| R1 ✓ `00dfd2f` | Addendum 9: a restored pane's modes start off | 114 | `daemon/src/lib.rs` |
| L0 ✓ `27c36d5` | Addendum 10 in this spec; the prototype | — | docs |
| L1 ✓ `cad289f` | One shell by default; setup runs before the program when there is no shell pane | 118, 119 | `layout.rs`, `stream.rs` |
| L2 ✓ `98e0473` | `init::render_project`, `config::save_layout`, `Node` ↔ TOML | 117, 120 | `init.rs`, `config.rs`, `layout.rs` |
| L3 ✓ `2f1a0b9` | `Modal::Setup`: the builder, agent and setup step of `^␣ w` | 115–117, 122 | `tui/modal.rs`, `tui/setup.rs`, `tui/mod.rs` |
| L4 ✓ `20dc5da` | `^␣ p` saves the layout; `layout changed` in the header | 120, 121 | `tui/mod.rs`, `tui/draw.rs`, `settings.rs` |
| D2 ✓ `5a4f02a` | Addendum 11: `d` on the project root | 123 | `diff.rs`, `tui/mod.rs` |
| X0 ✓ `9adb9e7` | Addendum 12 in this spec; the prototype | — | docs |
| X1 ✓ `5dd7118` | Selection state in the TUI: drag, Shift + drag, word and line, edge scroll, OSC 52 copy, clear rules | 124–128, 131–134 | `tui/mod.rs`, `tui/draw.rs`, new `tui/select.rs` |
| X2 ✓ `bb27c1a` | The copied banner on the pane's top edge; the keyboard select mode is withdrawn | 135 (129, 130 withdrawn) | `tui/mod.rs`, `tui/draw.rs` |
| M0 ✓ `78f31c5` | Addendum 13 in this spec; the prototype | — | docs |
| M1 ✓ `2d2e8c8` | Motion core: `tui/anim.rs` (tween, ease-out, colour lerp), the frame deadline in `event_loop`, synchronized output, `animations` setting and its `^␣ ,` row | 137–139, 147, 148 | `tui/anim.rs`, `tui/mod.rs`, `settings.rs`, `tui/settings_view.rs` |
| M2 ✓ | Workspace switch: sliding selection, header fade, panes slide; the spinning `✻` | 136, 143 | `tui/anim.rs`, `tui/draw.rs`, `tui/mod.rs` |
| M3 ✓ | Status bar: the leader's hint lists the useful keys, hints cross-fade between modes, the chip's colour eases, messages enter and fade | 140, 142 | `tui/draw.rs` |
| M4 ✓ | Popups fade in, picker row slides, which-key at 150 ms and `which_sticky` (a row in `^␣ ,`) | 141, 144 | `tui/draw.rs`, `tui/finder.rs`, `tui/modal.rs`, `settings.rs` |
| M5 ✓ | Focus border ease; eased wheel scroll | 145 | `tui/draw.rs`, `tui/mod.rs` |
| M6 ✓ | First-run welcome, `welcomed` and `JW_WELCOME=0` (set by `scripts/drive.py`) | 146, 149 | `tui/draw.rs`, `settings.rs`, `scripts/drive.py` |
| W1 | The sidebar follows the registry: `RegistryWatch` on the `Tick`, read-only `load_rows` in the background, rows held while an overlay is open | 150–159 | `tui/registry_watch.rs`, `tui/mod.rs` |
| R0 ✓ `c5152e8` | Addendum 15 in this spec; the prototype (approved 2026-10-11) | — | docs |
| R1 ✓ `7d7678d` | Core: `open_prs`, `detail`, `Entry.review_base`, `Stream::resolve` and `command()` for reviews, `review.rs` (fetch, worktree, context file) | 165–170, 172, 174, 175, 178 | `connectors/github.rs`, `core/registry.rs`, `stream.rs`, `review.rs` |
| R2 ✓ `936b3cd` | TUI: `Kind::Reviews` with a loading state, `R`, jobs, the pending `Prompt`, `m` and `o`; `Open` starts no process for a viewer leaf; the daemon test | 160–164, 171, 173, 176, 177, 179–181 | `tui/finder.rs`, `tui/mod.rs`, `tui/modal.rs`, `settings.rs`, `daemon/src/lib.rs`, `crates/jw/tests/daemon.rs` |
| R3 ✓ | README; version 0.0.13; a hand test with `scripts/drive.py` against a local origin and a fake `gh`; REQ-175 and REQ-183, which that test found missing | 175, 183 | `README.md`, `core/src/actions.rs`, `core/src/connectors/git.rs`, `tui/mod.rs` |
| A1 ✓ `db0d8ce` | Agent programs: `is_agent_program` in `jw-core`, `foreground` in the daemon, `Prompt` by program, `jw ls --json programs`, `jw read --pane` by program, help and the `jw` skill | 194–200, 203 | `crates/core`, `crates/daemon/src/lib.rs`, `crates/jw/src/{cli,help}.rs` |
| A2 ✓ `962d445` | The program on the pane title and the agent marks for such panes | 201, 202 | `tui/draw.rs`, `tui/mod.rs` |
| P1–P3 ✓ `66a27a6`, `a9c7148` | `jw pane open\|close\|ls`: `DaemonMsg::Opened` and `PROTOCOL` 10, the CLI, the skill, README and this addendum | 204–221 | `proto/src/proto.rs`, `daemon/src/lib.rs`, `crates/jw/src/{cli,help,skill}.rs`, `crates/jw/skill/SKILL.md` |
| V0 | Addendum 19 in this spec; the prototype (approved 2026-10-11) | — | docs |
| V1 ✓ | Core: `Scope`, `load(dir, &Scope)`, `Loaded::TooLarge` with `DIFF_CAP`, `commits`; tests in a temp repo | 223–226, 238, 244 | `core/src/diff.rs` |
| V2 ✓ | TUI: `DiffView` with per-view state keyed by path, the sidebar sections, `1 2 3`, `Tab`, `( )`, the narrow overlay and breadcrumb, the title; the jobs with a number per slot, the commit cache and the loading, failed and too-large states (V3's work came with it, the view could not be shown without it) | 222–233, 235–243, 245, 246 | `tui/diffview.rs`, `tui/draw.rs`, `tui/mod.rs`, `core/src/diff.rs` |
| V3 ✓ | A diff pane's answer no longer reloads the registry on the UI thread (RISK-75); an `App`-level test of the job round trip; a hand test with `scripts/drive.py` on a synthetic repo (4,000 new files, 231 commits, 300 untracked) that found three faults in the too-large view, fixed. A review workspace was not run end to end, only `Stream::resolve`'s test for `review_base`; REQ-234 waits for OPEN-31 | 237, 238, 241 | `tui/mod.rs`, `tui/diffview.rs` |
| V4 ✓ | README; version 0.0.18 (past main's 0.0.17); a hand test with `scripts/drive.py` in a repo with commits and a dirty tree, and in a big one | — | `README.md`, `Cargo.toml` |
| K1 ✓ `8b6b568` | Core of Addendum 20 (its commit message says Q1): `Prompt.pane`, `PROTOCOL` 11, `agent_pane` validating a named pane, the integration tests, the version (past main's) | 248–251, 254 | `proto.rs`, `daemon/lib.rs`, `tui/mod.rs`, `tests/daemon.rs` |
| K2 ✓ `cfa2f62` | The CLI of Addendum 20 (its commit message says Q2): `jw prompt [--pane] [--tag] [--]`, the guards, `help.rs`, the skill | 247, 252, 253, 255, 256 | `cli.rs`, `help.rs`, `SKILL.md` |
| E1 ✓ `b1b6f7b` | Measure first: the `rm_*` trace spans and marks, no behaviour change; the numbers in Addendum 21 | — | `tui/mod.rs` |
| E2 ✓ | The two-phase `X`: `RmReady` then `RmPr`, `PrLine`, `⇥`/`d` to done, `prcache.rs`, `Client.timeout`, the spinner; tests; version 0.0.20 (past main's 0.0.19); the prototype (approved 2026-10-11) | 257–267 | `tui/mod.rs`, `tui/modal.rs`, `tui/prcache.rs`, `tui/draw.rs`, `core/src/connectors/github.rs` |
| E3 ✓ | `rm_plan` runs `status`, `log` and the branch check on scoped threads (RISK-80): `rm_local` 50 → 22 ms, modal 92 → 68 ms in release | 257 | `core/src/actions.rs` |
| E4 ✓ | `worktree_state` / `rm_plan_from`: the worktree's git runs while `Project::open` does; median 148 → 127 ms on the large repo | 257 | `core/src/actions.rs`, `tui/mod.rs` |
| E5 ✓ | `Modal::Rm.pending`, `Job::RmOpen` / `Job::RmWork`, the `Work` row, keys held until git answers; `rm_plan_from` takes `branch_exists`; tests; the prototype (version 3, approved 2026-10-12) | 268 | `tui/mod.rs`, `tui/modal.rs`, `core/src/actions.rs` |
| E6 ✓ | The footer and the status bar say when the name must be typed (`Mode::ConfirmName`, `ConfirmWait`); fixes the bar saying `y yes` when `y` does nothing | 269 | `tui/draw.rs`, `tui/modal.rs` |
| Cutover ✓ | Go deleted (`main.go`, `internal/`, `go.mod`), CI is Rust only, the README describes the Rust jw; `testdata/*.go.*` stay as fixtures of files in the wild | 16 | |
| F0 ✓ | `JW_TRACE`, `scripts/bench-switch.py`, the Measurements above | 184 | `tui/trace.rs`, `tui/mod.rs`, `scripts/` |
| F1 ✓ | S1, S9: `Stream::resolve` (the `git` spawn) off the UI thread, the frame deadline counted from the frame's start; the unexplained ≈ 100 ms of a first open | 185, 191 | `tui/mod.rs`, `core` (callers) |
| F2 ✓ threads | S2: parse in the reader thread (`Msg::Snapshot` carries a `Parser`). S4 (keep pane frames while content loads, REQ-189) is closed, not needed after F3 | 186, 190 | `tui/mod.rs`, `tui/draw.rs` |
| F3 ✓ | **now justified (key → content 146 ms, 118 ms of it the replay):** send the screen first and the history after, or a capped tail of the ring; S8 (save off the open path) only if a number shows it | 188, 190 | `daemon/src/lib.rs` |
| F4 ✓ opening | S1 (closed workspaces), S11: stream cache guarded by `Stamp`, `folders::entry` cached per `.git` stamp, branch from `.git/HEAD` | 185 | `core/src/stream.rs`, `core/src/folders.rs`, `core/src/connectors/git.rs`, `tui/mod.rs` |
| F5 | S5: skip unchanged rows in `screen_to`, one flush per frame, only if F0 shows draw over budget | 187 | `tui/draw.rs`, `tui/anim.rs` |
| RM1 ✓ `43a7b2e` | `session::rm` and `session::rm_plan`; `session` reserved; version 0.0.21 | 270–273, 279, 281–283 | `core/session.rs` |
| RM2 ✓ `5d82a9b` | `jw session rm <name> [--yes]`, with `Close` for the session's workspaces | 270–275 | `jw/main.rs`, `jw/cli.rs` |
| RM3 ✓ | `^D` in the session picker and its confirm modal (`Modal::RmSession`, `Job::SessionRm`: the plan is read off the UI thread, then the modal opens; after a delete the picker reopens) | 276–278, 280 | `tui/finder.rs`, `tui/modal.rs`, `tui/draw.rs`, `tui/mod.rs` |

Reuse: `focus_towards` (`tui/mod.rs`) becomes `layout::neighbour`; `actions::{new_stream, rm_plan, rm, sync, done_plan}`, `src/diff.rs`, `src/view/md.rs` and `Modal::Rm` (type the name) stay as they are.

## Decided (were open questions)

- OPEN-1 → `Ctrl-Space`, configurable.
- OPEN-2 → `docs/specs/rust-tui.md`.
- OPEN-3 → keep `jw prompt` and `jw new --task` as socket clients.
- OPEN-4 → unix only; `shell_windows.go` is not ported.
- OPEN-6 → TUI, not a GUI (RAT-10).
- OPEN-7 → DSR and DA (REQ-89) go in S14: same reply path, and without them crossterm programs wait for a timeout.
- OPEN-8 → last-wins between several TUIs.
- OPEN-9 → following the outer terminal's own mode 2031 is later; S14 keeps `AppleInterfaceStyle` and the settings.
- OPEN-10 → plain Esc goes as `CSI 27 u` to a pane with flag 1, as the kitty spec says (it also removes nvim's Esc delay).
- OPEN-11 → the built-in themes are not written to disk on first run; `jw theme export` gives each one (S16).
- OPEN-12 → per-theme syntax colours (a `syntax` block for syntect) are later; highlighting still picks syntect's theme by `dark` (`view/highlight.rs:27`).
- OPEN-13 → copy on release, drag past the edge scrolls, a banner at the pane's top right (prototype 2, approved 2026-10-10); no keyboard mode.
- OPEN-14 (open) Should an external change show a status note ("sidebar updated")? Default: silent. A visible element would need the HTML prototype first.
- OPEN-15 (open) Poll the registry every 2 s (the `List` cadence) or every 0.5 s tick? Default 2 s.
- OPEN-16 → a `[review]` table in the project TOML (layout, agent command, a review skill) is later.
- OPEN-17 to OPEN-21 (addendum 15) → the user did not answer; built with the proposed defaults: the picker lists all open PRs, `m` toggles "requested of me"; the project's setup runs; an existing review asks before the prompt is sent again; the review rules ride in the visible prompt, not `--append-system-prompt`; the trust-folder exposure is accepted (RISK-52). Say so to change any.
- OPEN-26 → a pane's program is detected only, never declared (RAT-53); no `jw prompt --pane` (RAT-54); title variant B (RAT-57). Approved 2026-10-11.
- OPEN-27 to OPEN-30 (addendum 18) → built with the proposed defaults; say so to change any.
- OPEN-31 (open) A key to refresh Uncommitted while the pane is open (not in the prototype, so it needs the prototype first). Default: none; a view reloads when the pane is restored.
- OPEN-32 (open) The caps, 200 commits and 2 MiB of diff, are guesses. Default as written; adjust after a hand test in a big repo.
- OPEN-33 → General is committed work only (RAT-63); OPEN-34 → the pane reopens on General (REQ-245); OPEN-35 → Uncommitted is one list with a mark per file, not split into staged and unstaged. The prototype shows all three and the user approved it (2026-10-11).
- OPEN-5 → `~/.config/jw/settings.json` (JSON, outside Go's `*.toml` glob) and the `^␣ ,` screen (addendum 3).
- OPEN-36, OPEN-37 (addendum 20) → built with the proposed defaults: the sender prefix is opt-in (`--tag`, RAT-71) and `--pane=3` is not supported, like `pane open --of 3`; say so to change.
- OPEN-38, OPEN-39 (addendum 21) → built with the proposed defaults: a merged PR offers `d`/`⇥` on the PR line and never switches modal by itself; the answers live 30 s and gh gets 3 s.
- OPEN-40 (addendum 22) → defaulted: a session removal only forgets worktrees; `--worktrees` to also run `actions::rm` on clean ones is not built.
- OPEN-41 (addendum 22) → defaulted: another client attached to the target session is not asked; its workspaces close.

## Corrections

- REQ-129 and REQ-130 specified a keyboard select mode (`^␣ v`, `SEL`). The user dropped it: terminals today select with the mouse and the shell edits the prompt, so jw keeps the mouse and adds the copied banner (REQ-135).
- Addendum 15's hand test found two things the first build lacked: `X` listed the PR's own commit as unpushed work (it lives only in `refs/jw/pr-<n>`, REQ-175 now says so and `rm_plan` honours it), and the header told a review its layout had changed (REQ-183).
- Addendum 15's first draft sent the PR head to `+review/pr-<n>` and said `Open` already handled `view:` leaves. A fetch cannot update a branch that is checked out in a worktree, so the head goes to `refs/jw/pr-<n>`; and the daemon's `open` started a shell for a viewer leaf until REQ-181.
- REQ-39's done checks, ported from Go, assumed a worktree stays on the branch jw made for it. Agents switch branches, so a worktree whose work merged through another PR looked unfinished (addendum 8).

- Addendum 5's first draft answered only `CSI ? u`. nvim sends `CSI ? u` then `CSI c` and pushes its flags only once both answers arrive (nvim 0.12.5), which the end-to-end test caught. S14 answers `CSI c`; REQ-100 keeps the two in order.
- The first diagnosis of Shift+Enter said it needed a `PaneInfo` field and a `PROTOCOL` bump. It didn't: the TUI's parser already sees the raw output (RAT-15).

- Paths in this spec say `src/…` from when jw was one crate. Since `6be5ac4` the code is a workspace: `src/{layout,core,connectors,actions,folders,session,stream,init,diff}` are in `crates/core/src/`, `src/proto` and `src/client.rs` in `crates/proto/src/`, `src/daemon` in `crates/daemon/src/`, `src/{tui,view,theme,settings}` in `crates/tui/src/`, `src/{main,cli}.rs` and `tests/` in `crates/jw/`.

- The first proposal kept the CLI underneath the TUI; the user chose TUI only.
- The first proposal assumed the TUI would be a client of herdr; the user asked for it to replace herdr.
- v1 put tui-term in the stack; P5 draws vt100 screens itself.
- RISK-2 assumed `[layout]` was a new table. It already existed with `editor`.
- v1's "spaces → streams" with a free space first, a sticky NAV mode (REQ-17), `F` focus-all (REQ-25) and a Telescope finder with `> @ / :` prefixes (REQ-26/27) were replaced by v2's workspaces, one-shot leader and three plain pickers.
- `33c46f5` kept project headings and a "free" section in the sidebar; the prototype has neither. REQ-50 replaces REQ-33.
- v1 said "TUI only, no CLI; the jw skill becomes obsolete". An agent overseeing a session needs the CLI, so it stays as socket clients.
- RISK-17 said one `git` ran per worktree per reload; `root_of` already cached it per project in memory. S1 makes it persistent.
- The addendum-3 plan keyed the daemon by session. Ids unique across sessions made that unnecessary: `session.json` and the daemon stay as they were.
- An addendum-3 draft read `.jw/` as a per-project config folder (like `.vscode/`), then as a pinned "home" brain workspace. The user meant a scope for which projects show: named sessions. The brain is a usage pattern, not a jw concept.
- The 2026-10-09 prototype's `.worktrees/<name>` on `jw/<name>` was wrong for this repo: worktrees live at `<repo>-wt/<name>` on the `branch` template (`feat/{name}`).
- The Contract said the theme follows "the terminal or the OS". It followed the OS only (`theme::system_dark`), and programs in panes were never told it: vt100 dropped their OSC 11 and mode 2031 and nobody answered (S14).
- "There is no navigation mode" (Contract) held until addendum 7: the user asked to move between workspaces and projects with the arrows, so an arrow after the leader starts a short move that any other key ends.
- REQ-40 restarted a renamed worktree's panes so nothing ran in a moving folder. Losing the tab on every rename cost more than the stale paths it avoided; REQ-111 keeps the panes.
- REQ-57 put no limit on `d`, but the TUI refused it on a project root (`current_worktree`). Addendum 11 lets it run there.
- Interfaces → Protocol said every client opens with `Hello` and refuses another `PROTOCOL`. Only the TUI did: `jw ls`, `sessions`, `read`, `prompt` and `worktree` misread an old daemon (everything showed closed) until the review pass of addendum 10's PR.
- The daemon's lock order was "workspaces → panes → a pane's state", and saves ran unserialized from every client thread, the timer and the signal thread, so two could clobber `session.json`. A `saving` lock now comes first: saving → workspaces → panes → state. `split`, `dock`, `kill`, `place` and `rekey` also told watchers before saving, against "save before telling"; they save first now.
- The Contract and Interfaces gave a project with no `[layout]` the default tree (nvim | agent over a shell). Addendum 10 makes it one shell, and the user builds the rest.
- The worktree-names addendum was drafted as addendum 4 with REQ-81–83, RISK-25 and `PROTOCOL 7` on a branch cut before S14–S16, which took those ids. It merged as addendum 7, REQ-110–112, RISK-35 and `PROTOCOL 8`.
- REQ-72's restore reset only the screen (`?1049l`), and later REQ-87 and REQ-98 the theme and kitty state. The old process's mouse, paste and cursor modes survived into the new one until REQ-114.
- REQ-131 first said output that moves the rows under a selection clears it. Rows are counted from the oldest history line, so scrolling output leaves the selection on its text; only a full history, a screen switch or a shrunk history clears it.
- REQ-59 left selection to the host terminal's Shift + drag and RISK-6 put plain mouse selection in scope, but with mouse capture on and a shell that never asks for the mouse, a plain drag did nothing and no REQ covered it (addendum 12).
- REQ-20 named `[tui] animations = false`, a table in the project TOML. Settings live in `settings.json` (RAT-31), so the key is `animations` there (REQ-138).
- REQ-31 and the Contract said 600 ms before the which-key popup. A new user needs the keys sooner (RAT-32), so the default is 150 ms; `which_delay_ms` still sets it.
- REQ-138 first said that without truecolor every change applies at once. The selection band, the spinner and the welcome steps need no colour blending, so only the fades and the panes' slide wait for truecolor.
- REQ-146 first said the welcome starts "with no workspace open" and does not pass on the key that closes it. At startup nothing is open yet even for a returning user, so it starts from an empty sidebar; and step 1 itself tells the user to press the leader, so that key is handled as usual.
- The brief for W1 said the sidebar stayed stale until the TUI was reopened. True for writes by the CLI and by other TUIs; the TUI's own jobs already reload (`on_job`).
- The same brief said the loop wakes only when something animates. `Msg::Tick` wakes it every 500 ms regardless (`tui/mod.rs:353`); only the animation timers in `next_wake` are conditional. The registry watch rides that tick (REQ-151, REQ-152).
- REQ-21 and REQ-57 spoke of "every changed file" in one scroll: the working tree against the base. With addendum 19 that is General plus Uncommitted, in separate views. REQ-169 still holds for General; the uncommitted part moved to Uncommitted.
- REQ-123 said `d` with no `origin/<default>` diffs against `HEAD`. That is now the Uncommitted view; General has nothing to compare then (REQ-226).
- REQ-234 holds in the model (scroll and viewed live in `Pos`, by path, apart from the files), but nothing reloads a view while the pane is open, so nothing exercises it until OPEN-31 is answered.
- Until addendum 19, a diff reload rebuilt the view and lost scroll and viewed files (`tui/mod.rs:3239`); REQ-234 fixes that.

- F2's threads were expected to bring key → content to about 30 ms. They did not: moving the replay off the UI thread removes the stalls (gap 120 → 20 ms) but the content cannot appear before the replay ends, and the replay took longer on a shared core. Only F3 shortens it.
- REQ-189 (a pane's frame kept while its content loads) was planned because a switch cleared every pane and the slide ran over nothing. After F1–F3 the tree reaches the TUI in ≈ 4 ms (the frame shows with `starting…`) and the content in ≈ 6 ms when the workspace is open, ≈ 10 ms when it is closed and ≈ 46 ms for a folder opened for the first time, so the empty window is shorter than three frames and the slide starts over content. Closed on 2026-10-11 without the prototype; reopen it only if a blank flash is seen when opening a workspace.
- The speed addendum was drafted as addendum 14 with REQ-150–159, RAT-34–37, RISK-46–48 and OPEN-14–17 on a branch cut before W1 and R1–R3, which took those ids. It merged as addendum 16, REQ-184–193, RAT-48–51, RISK-57–59 and OPEN-22–25, with version 0.0.14.
- F3 was drafted as capping the ring; the split (`Screen` then `History`) keeps the scrollback whole (REQ-190) and needed `PROTOCOL` 9, which the addendum had said would stay 8.
- Addendum 16 was agreed with S1–S8 as guesses. F0 confirmed S1, S2 and found S9 (the frame cadence is ≈ 20 ms, not 16); it showed S3, S5, S6 and S7 cost almost nothing at one pane. Its OPEN ids were first 1–4, which this spec already used: renumbered 22–25.
- Addendum 13 said that on a switch the panes "only enter; nothing leaves". True, but it also means the slide animates over blank panes until the Snapshots arrive (S4, addendum 16).
- The brief for A1 assumed the daemon did not know a pane's child. It does: `fg` and `cwd` on every `List` (`lib.rs:549-554`), and the TUI already shows `fg` in the title. The first plan added `PaneInfo.program` and a `PROTOCOL` bump; with detection only, `fg` is the program and the protocol stays at 9 (RAT-52).
- Addendum 18 was briefed as exposing `Split` and as using `PaneInfo.program`. The field is `PaneInfo.fg` (older than PR #42; commit `452a300` is only a version bump, the agent logic is `db0d8ce`), and `Split` returned nothing, so the id needed a reply and `PROTOCOL` 10: REQ-203's "stays at 9" holds for A1 only.
- The plan for addendum 18 said a pane cannot know its own id (RISK-2 of the plan). It can: the daemon sets `JW_PANE_ID` (`daemon/src/lib.rs`), which `jw hook` already reads; REQ-219 uses it.
- `find()` is documented as taking `project/name` but its filter compares the whole label, the name or the id (`cli.rs`); a path is not accepted.
- REQ-206 and the Protocol rows of addendum 18 said `PROTOCOL` 10. Addendum 20 adds a field to `Prompt` and makes it 11 (REQ-254); `Interfaces → Protocol` now lists `Prompt{stream,text,pane?}`.
- RISK-78 was first written saying a restarted daemon restores its panes with their ids. `restore` allocates new ones (`daemon/src/lib.rs`, `new_id`); the old id only finds the scrollback file.
- The brief for Addendum 21 guessed that `Project::open` searches sibling worktrees. It does not (`actions.rs:28-35`); the sibling loop in `ask_remove` only runs after a failed open. The measured cost is gh.
- The status bar's `⟳ checking <name>` was thought to be animated. It was a static glyph; the animated spinner belonged to the agent marks only (`tick_motion`).
- Before Addendum 21, a merged PR whose done checks failed (a dirty tree, a HEAD outside the PR) made `X` fail with an error and no modal, so the worktree could not be removed with `X` at all. It now opens the rm modal with `#n merged` (REQ-263).
- REQ-257 said the modal opens "from `rm_plan`", and E2 to E4 opened it once git had answered. E5 opens it when the project is open and fills the plan later (REQ-268); the first row of RISK-80's next steps (an early modal) is built, the second is still open.
- The brief for Addendum 22 said to refuse `main` as a default session. jw has no default session; `main` renames like any other (`session.rs`, `a_renamed_session_keeps_its_workspaces`). Only the current session is refused (REQ-271).

## Tests

- core: parse config/registry from Go fixtures; registry.json round-trip read back by Go.
- layout: tree → rects for several sizes; `insert`/`remove`/`swap`/`neighbour` as tables.
- daemon (tempdir, `tests/`): `cat` pane, detach, kill the client, reattach, same screen; `Split`/`Kill`/`Swap`/`Name` produce `Tree` and update `session.json`; `printf '\e]2;hi\a'` arrives as `Title`; a version mismatch is an error; S14: OSC 10/11, `?996n`, `6n`, `c` answered from the theme, `?997` only to panes with `?2031h`, the theme in `session.json`, replayed queries unanswered. S16: every built-in parses under its own name; schema and `Colors` agree; a user file shadows a built-in once; export → install round-trips; `jw theme` against a temp `XDG_CONFIG_HOME` (`crates/jw/tests/theme.rs`).
- actions: fakes behind traits; `X` takes the done or rm path by PR state; rename against a temp repo; `free.json` → `folders.json` migration.
- finder: fuzzy ranking tables; folder browser over a temp dir.
- TUI: ratatui `TestBackend` for the which-key popup, sidebar and modals.
- keyboard protocol: `kitty.rs` units (push, pop past empty, set modes, query, depth, alternate stack, replay round trip); `keys.rs` units (Shift+Enter is `\r` with flags 0 and `CSI 13;2u` with flag 1); the daemon answers `CSI ? u` before `CSI c`, and a late Snapshot ends with the stack. End to end, with a script that plays Ghostty (answers the probe, sends keys as Ghostty does with flag 1) against an isolated jw: Shift+Enter is a new line in claude 2.1.296 and codex 0.162.1, fires `imap <S-CR>` in nvim 0.12.5; a program that never pushes still gets `\r`; the TUI pops on exit, and pushes nothing when the terminal doesn't answer.
- selection (X1, X2): unit tests over a vt100 screen for the extracted text (trailing spaces, soft-wrapped rows, wide characters, history rows) and for word and line bounds; the OSC 52 bytes; the highlight and the banner text; `mouse_encodings` still passes. Manual: zsh drag, double and triple click, drag past the edge; the banner appears top right and goes after about 2 s; nvim plain drag reaches nvim, Shift + drag selects.
- motion (M1–M6): units for the tween (starts at 0, reaches 1 at its duration, monotonic, an interrupt starts from the displayed value) and for the colour lerp; `next_wake` blocks when idle, wakes a frame after the last draw while a transition runs, for the drag edge and the which-key, takes the soonest, and wakes when a message starts to leave; `TestBackend` for the spinner glyph by elapsed time, the welcome (shows; any key closes it, writes `welcomed` and is still handled), the slide's last frame equal to the static draw, and `animations = false` giving the final frame at once; `tick_motion` on an `App` without a daemon (`App::bare`): the band starts on the current row without motion, a switch glides it and keeps the loop awake until `SWITCH`, `animations = false` jumps it, no workspace means no band, the spinner steps only for visible rows; the bytes of a draw start with `CSI ? 2026 h` and end with `CSI ? 2026 l`, also after a failed draw. Manual in Ghostty with `scripts/drive.py`: switching workspaces with the arrows, the which-key at 150 ms, no tearing in a fast `cat`, idle CPU near zero.
- review (R1–R3): core, with a fake `gh` as in `github.rs`: `open_prs` and `detail` (JSON in, structs out, non-zero exit, no gh); `review_stream` in a temp repo whose origin serves `refs/pull/7/head`: worktree at the head sha, branch `review/pr-7`, `origin/<base>` present, a second call reuses it, a failed fetch leaves nothing; `Stream::resolve` with and without `review_base` (base, tree, `command("view:diff")` is `None`); registry round-trip with and without the field; the context file's content and cap. TUI: `Finder` Reviews filter, order and loading state; the jobs and the pending prompt on an `App` without a daemon (`App::bare`). `crates/jw/tests/daemon.rs`: `Open` with the review tree, then `Prompt` reaches the agent pane; `PROTOCOL == 8`. Manual: `scripts/jw-test` in a repo with an open PR and a fork PR: `^␣ R`, filter, `↵`, the diff against the PR's base, claude's first message, close and reopen the workspace, `X`.
- diff views (V1–V3): core, in a temp repo: General lists only committed changes; Uncommitted lists staged, unstaged and untracked, and works before the first commit; `commits` order, the 200 cap and its note, a merge and a root commit in `Commit(sha)`; `DIFF_CAP` gives `TooLarge` with the file list, for a committed diff and for untracked files, which are not read; an empty base. TUI, on an `App` without a daemon: a view switch keeps scroll and viewed (each view, and each commit); a reload keeps them by path; a result with an old `gen` is dropped; `1 2 3`, `Tab`, `( )` and the sidebar cursor; narrow shows the breadcrumb and the overlay; no key uses Alt. Manual: `scripts/jw-test` in a repo with commits and a dirty tree, in a big repo (the UI never stalls while a view loads), and in a review workspace.
- manual (after Q2 and Q5): claude and nvim in panes; `w`, `t`, `x`, `HJKL`, `n`, `d`, `↵` in the diff, `q`, then `jw` again shows the same layout.
- switch speed (F0–F4): `JW_TRACE` lines per phase; `scripts/bench-switch.py` p50/p95/max against an isolated `scripts/jw-test` dir with 3 worktrees and a 2 MiB ring (kill only that dir's pid); a `Msg::Snapshot` parser equals the inline one and Output after it applies in order; `tick_motion` and a 240×60 draw on `App::bare`/`TestBackend` under the budget.
- agent programs (A1, A2): `is_agent_program` and `is_agent_pane` tables (`core/src/program.rs`; a path or a `-` prefix is not part of the name). Daemon, in `crates/jw/tests/daemon.rs`: a `shell` pane running a link to `cat` named `claude` is reached by `Prompt` (a script shows as its interpreter, and macOS kills a copy of `cat`); the no-agent error is `prompt_without_an_agent_pane_is_an_error`. `App` units: `marks()` gives `✻` for a busy shell pane running `claude` and nothing for `nvim`; `pane_title` names the agent program and flags it, and leaves other programs unflagged. Not covered by a test: role `agent` winning over a program pane, the pane that stops being an agent, the magenta title colour (`TestBackend`), `jw ls --json programs` and `jw read --pane <program>`. Manual with `scripts/jw-test`: run `claude` by hand in a shell pane, `jw prompt` it from another workspace, exit claude and see the next `jw prompt` refuse.
- pane commands (addendum 18): `crates/jw/tests/daemon.rs`: `a_split_tells_only_its_requester_the_new_pane` (REQ-205, 207) and `jw_pane_opens_lists_and_closes_panes` (the binary against a daemon and a state dir of its own: not open, open a viewer and a command, the other client's `Tree` (REQ-209), `ls` as a table and JSON, `--of` and `close` outside the workspace, the last pane refused). `cli.rs` units: `run_target` and `anchor` tables. `skill.rs` names the three commands. Not covered by a test: `--force` on the last pane, REQ-219, `--split down` placement, `view:md:` against a running TUI. Manual with `scripts/jw-test`: with a TUI open, `jw pane open <ws> --run view:diff` from a second terminal.
- prompt to a named pane (addendum 20): `crates/jw/tests/daemon.rs`: `prompt_with_a_pane_reaches_that_agent` (REQ-249), `prompt_with_a_pane_refuses_what_is_not_a_live_agent_of_the_workspace` (REQ-250, REQ-251); `proto.rs`: `a_prompt_without_a_pane_names_none` (REQ-254); `cli.rs` units: `prompt_options_end_at_the_first_word_of_the_text` (REQ-247, REQ-252), `a_tag_names_who_sent_the_text` (REQ-255), `skill.rs`: `the_skill_says_how_to_name_a_pane_and_not_to_answer_permissions` (REQ-256), `the_pane_you_run_in_is_not_a_target` (REQ-253). REQ-248 holds through the REQ-194 and REQ-196 tests above, which send no `pane`.
- live registry (W1): `RegistryWatch` units over a tempdir (unchanged, touched, a file that appears or goes); an `App` without a daemon: rows from `Msg::Rows` replace the sidebar, with a modal open they land in `pending_rows` and the next tick after it closes applies them, `active` keeps its id; a truncated JSON leaves the rows alone; `next_wake` is still `None` when idle. Manual: with a TUI open, `jw worktree <name>` from a second terminal shows the row within about 2 s; repeat with `^␣ r` open and the row appears after `esc`.
- addendum 21: a gh that hangs times out (`github.rs`); a late answer changes only the PR line, is dropped for another modal, offers `d`/`⇥` only while nothing is typed, the row fills in place (render test), the spinner runs while checking (`tui/mod.rs` `motion_tests`); the memory keeps an answer for the TTL and no failure (`tui/prcache.rs`).
