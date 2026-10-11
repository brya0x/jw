---
status:      agreed (v2 + the screen addendum + addendum 3 "sessions", 2026-10-09; P0–P9 built against v1); addendum 12 (X0) drafted
scope:       [Cargo.toml, src/**, .github/workflows/ci.yml, internal/core/config/config.go (one relaxation), README.md]
depends_on:  [git, gh on PATH]
supersedes:  [herdr backend: internal/backends/terminal, internal/connectors/herdr]
---
# jw in Rust: a TUI that is its own multiplexer (replaces herdr)

## Contract

- One binary `jw`, unix only (macOS + Linux). With no arguments it opens the TUI client; a hidden `jw daemon` runs the server. The other commands are socket clients for sessions and for agents (addendum 3): `jw new <session>`, `jw [session]`, `jw sessions`, `jw ls`, `jw read`, `jw worktree`, `jw prompt`, `jw hook`, plus `jw help` and `jw skill [install]`.
- **Two concepts.** A *workspace* is an open folder: a project (any folder, git or not) or one of its worktrees. A workspace holds *panes*.
- **One-shot leader.** `Ctrl-Space`, then one key, then back to the terminal (tmux-style). A pause of 150 ms after the leader shows every key (600 ms before addendum 13); `?` shows them at once. The one exception is an arrow after the leader: arrows keep moving through the workspaces until another key gives the keyboard back (addendum 7).
- **Every action applies to the current workspace or the focused pane.** The sidebar is a list to read and click, numbered for `1–9`, and walked with `^␣` + arrows.
- **The daemon** owns every terminal (one PTY per pane, a VT parser with screen and scrollback) and each workspace's pane tree (splits, ratios, names). It survives the client and writes `session.json` on every change.
- **The client** draws the sidebar (projects with their worktrees indented), the current workspace's panes, a header (workspace · branch · PR) and a status bar. It owns only focus and the full view.
- **Viewers are panes the client draws:** a GitHub-style diff (unified when narrow, side by side when wide, sticky file headers, viewed, changed words) and a Markdown reader. Code files open in nvim; there is no built-in editor or file tree.
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
| `↑↓` next/previous workspace, `←→` next/previous project; more arrows keep moving | | |
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
| Protocol | `src/proto`: u32 length + serde_json frames; a workspace is named by its registry id in `stream`. The client opens with `Hello{protocol}` and refuses a daemon that answers anything but the same `PROTOCOL` (RISK-14). C→D: `Hello`, `Attach{stream}`, `Detach`, `Input{pane,bytes}`, `Resize{pane,cols,rows}`, `Open{stream,tree}` (a `Tree<NewPane>`), `Split{pane,dir,new}`, `Kill{pane}` (removes the leaf), `Close{stream}`, `Swap{a,b}`, `Name{pane,name?}`, `Theme{dark,fg,bg}` (S14), `List`, `Prompt{stream,text}`, and the older `Spawn` (one pane, placed on the right). D→C: `Hello`, `Tree{stream,tree}` (on attach and after every change), `Snapshot{pane,…}`, `Output{pane,bytes}`, `Title{pane,title}` (OSC 0/2 via `vt100::Callbacks`), `Exited{pane,status}`, `Panes{…}` (with foreground process), `Spawned{pane}`, `Prompted{pane}`, `Error{msg}`. Pane ids are `u64` |
| Tree | `src/layout.rs`: leaves carry `{id, run, name?}`; `run = "shell"\|"agent"\|"editor"\|"dev:<svc>"\|"view:diff"\|"view:md:<path>"\|"<cmd>"`; `view:*` leaves have no PTY. Ops: `insert(beside, dir, leaf)`, `remove(id)` (the sibling takes the space), `swap(a,b)`, `neighbour(id,dx,dy)` (nearest rect that overlaps on the other axis), `rects` |
| Layout config | `[layout]` in the project TOML as in v1 (tree of `split`, `ratio`, `a`/`b`, leaves `run`). It is the starting tree when a workspace opens; without it, one shell (addendum 10). Go ignores the tree and `[tui]` (`rustOnly`, config.go) |
| Session | `session.json` in the state dir (next to the socket for any socket but the default one, so tests never touch it), written by the daemon with temp + rename after every tree change: `{workspaces: [{id, tree}]}`, each leaf with role, name, cmd, cwd and env. Recency and recent folders arrive with Q3/Q4 |
| Folders | `src/folders.rs` replaces `src/free.rs`. `folders.json` in the state dir holds `[{id, dir, opened, created}]`: the project roots and plain folders the user opened. The first load migrates `free.json` (each session becomes a folder; the file is renamed `.migrated`). A folder's `Entry` uses `project` = the git project name, or the folder name for a plain folder |
| Pane env | the JW_* vars of `actions::jw_env` (a plain folder gets JW_ID, JW_NAME, JW_PROJECT) + `JW_PANE_ID` |
| Theme | `src/theme.rs`: One Dark / One Light `Palette`s that every draw module (and the syntect theme) asks for through `theme::p()`. Pick: `JW_THEME=dark\|light` pins one (OPEN-5); otherwise macOS's `AppleInterfaceStyle`, re-read every 3 s so a switch repaints; dark elsewhere. Panes (S14): on every tick the TUI sends `ClientMsg::Theme` (`theme::wire()`) when `{dark, fg, bg}` changed. The daemon keeps it in `session.json` and answers from `Titles` (`daemon/src/lib.rs`): OSC 10/11 `?` → `rgb:RRRR/GGGG/BBBB` + ST, `?996n` → `?997;1\|2n`, `?2031$p` → `?2031;1\|2$y`, `6n` → cursor, `c` → `?1;2c`. A pane that set `?2031h` gets `?997;1\|2n` on a dark↔light switch. `COLORFGBG` is `15;0` / `0;15` at spawn. Querying the outer terminal is left for later (OPEN-9) |
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
- REQ-21 WHILE a diff pane is focused, the TUI SHALL show every changed file in one scroll with `j/k` per file, `]`/`[` per hunk, `v` viewed, `t` unified ⇄ side by side, and changed words highlighted; side by side by default when the pane is wide.
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
| First run | With no workspace open and `welcomed` unset, three steps appear one by one: `^␣` starts every shortcut, then `o` opens a folder, `?` lists the keys. Any key closes it and sets `welcomed` |

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
- REQ-146 WHEN the TUI starts with no workspace open and `welcomed` is false, it SHALL show three steps, one every 650 ms, and WHEN any key is pressed SHALL close them, write `welcomed: true` and SHALL NOT pass that key on. WHERE `JW_WELCOME=0` is set, the TUI SHALL NOT show it, so tests and `scripts/drive.py` (which sets it) keep their first key. The empty-stage line (Screen table) stays for later runs.
- REQ-147 THE motion SHALL add no `ClientMsg` and no `PaneInfo` field, and `PROTOCOL` SHALL stay at 8.
- REQ-148 WHEN a key or a click arrives during an animation, the TUI SHALL handle it before the next frame; WHEN a change interrupts an animation, the new one SHALL start from what is on screen; IF a frame takes longer than `FRAME`, THEN the animation SHALL jump to its end state.

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
- RAT-22 **`curl`, not a crate,** for `install <url>`: rare, and jw already shells out to `git` and `gh`; `ureq` would add TLS and about 40 crates for one command.

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
| M3 | Status bar: hint per mode, chip colour, message enter and fade | 140, 142, 149 | `tui/draw.rs` |
| M4 | Popups fade in, picker row slides, which-key at 150 ms and `which_sticky` | 141, 144 | `tui/draw.rs`, `tui/finder.rs`, `tui/modal.rs`, `settings.rs` |
| M5 | Focus border ease; eased wheel scroll | 145 | `tui/draw.rs`, `tui/mod.rs` |
| M6 | First-run welcome, `welcomed` and `JW_WELCOME=0` (set by `scripts/drive.py`) | 146 | `tui/draw.rs`, `settings.rs`, `scripts/drive.py` |
| Cutover ✓ | Go deleted (`main.go`, `internal/`, `go.mod`), CI is Rust only, the README describes the Rust jw; `testdata/*.go.*` stay as fixtures of files in the wild | 16 | |

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
- OPEN-5 → `~/.config/jw/settings.json` (JSON, outside Go's `*.toml` glob) and the `^␣ ,` screen (addendum 3).

## Corrections

- REQ-129 and REQ-130 specified a keyboard select mode (`^␣ v`, `SEL`). The user dropped it: terminals today select with the mouse and the shell edits the prompt, so jw keeps the mouse and adds the copied banner (REQ-135).
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

## Tests

- core: parse config/registry from Go fixtures; registry.json round-trip read back by Go.
- layout: tree → rects for several sizes; `insert`/`remove`/`swap`/`neighbour` as tables.
- daemon (tempdir, `tests/`): `cat` pane, detach, kill the client, reattach, same screen; `Split`/`Kill`/`Swap`/`Name` produce `Tree` and update `session.json`; `printf '\e]2;hi\a'` arrives as `Title`; a version mismatch is an error; S14: OSC 10/11, `?996n`, `6n`, `c` answered from the theme, `?997` only to panes with `?2031h`, the theme in `session.json`, replayed queries unanswered. S16: every built-in parses under its own name; schema and `Colors` agree; a user file shadows a built-in once; export → install round-trips; `jw theme` against a temp `XDG_CONFIG_HOME` (`crates/jw/tests/theme.rs`).
- actions: fakes behind traits; `X` takes the done or rm path by PR state; rename against a temp repo; `free.json` → `folders.json` migration.
- finder: fuzzy ranking tables; folder browser over a temp dir.
- TUI: ratatui `TestBackend` for the which-key popup, sidebar and modals.
- keyboard protocol: `kitty.rs` units (push, pop past empty, set modes, query, depth, alternate stack, replay round trip); `keys.rs` units (Shift+Enter is `\r` with flags 0 and `CSI 13;2u` with flag 1); the daemon answers `CSI ? u` before `CSI c`, and a late Snapshot ends with the stack. End to end, with a script that plays Ghostty (answers the probe, sends keys as Ghostty does with flag 1) against an isolated jw: Shift+Enter is a new line in claude 2.1.296 and codex 0.162.1, fires `imap <S-CR>` in nvim 0.12.5; a program that never pushes still gets `\r`; the TUI pops on exit, and pushes nothing when the terminal doesn't answer.
- selection (X1, X2): unit tests over a vt100 screen for the extracted text (trailing spaces, soft-wrapped rows, wide characters, history rows) and for word and line bounds; the OSC 52 bytes; the highlight and the banner text; `mouse_encodings` still passes. Manual: zsh drag, double and triple click, drag past the edge; the banner appears top right and goes after about 2 s; nvim plain drag reaches nvim, Shift + drag selects.
- motion (M1–M6): units for the tween (starts at 0, reaches 1 at its duration, monotonic, an interrupt starts from the displayed value) and for the colour lerp; `animating()` is false with no tween, so the loop blocks; `TestBackend` for the spinner glyph by elapsed time, the welcome (shows, any key closes it and writes `welcomed`), the slide's last frame equal to the static draw, and `animations = false` giving the final frame at once; the bytes of a draw start with `CSI ? 2026 h` and end with `CSI ? 2026 l`, also after a failed draw. Manual in Ghostty with `scripts/drive.py`: switching workspaces with the arrows, the which-key at 150 ms, no tearing in a fast `cat`, idle CPU near zero.
- manual (after Q2 and Q5): claude and nvim in panes; `w`, `t`, `x`, `HJKL`, `n`, `d`, `↵` in the diff, `q`, then `jw` again shows the same layout.
