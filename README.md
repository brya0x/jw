# jw

[![CI](https://github.com/brya0x/jw/actions/workflows/ci.yml/badge.svg)](https://github.com/brya0x/jw/actions/workflows/ci.yml)

**A terminal for working with coding agents in parallel.** jw is its own multiplexer. Each piece
of work gets its own git worktree, with an agent (Claude Code or Codex), nvim, a shell, dev
servers and a block of ports. Everything stays in one TUI that keeps running when you close it.

```
jw                       # open the last session
jw new testing           # a new session, starting in this folder with one shell
jw sessions              # every session, and what runs in it
jw help                  # every command
jw server status|stop    # is the daemon running; stop it
jw theme                 # every theme; jw theme install|export|use
```

Inside, press `Ctrl-Space` and then one key. Wait a moment after `Ctrl-Space` and every key
shows.

---

## Why

Coding agents made it cheap to work on several things at once, but the checkout didn't keep up:

- One working tree means one branch at a time. Switching branches restarts dev servers and
  confuses whichever agent is mid-task.
- Two copies of the same app fight over the same port (5173, 8081, 3000…).
- `git worktree add` gives you a second tree without your gitignored `.env.local` or
  `node_modules`, so it doesn't boot.
- Agents, editors and dev servers end up in scattered terminal windows.

jw treats a workstream as one unit: **1 worktree = 1 branch = 1 workspace = 1 port block =
1 PR**.

---

## Requirements

| Tool | Why |
|---|---|
| macOS or Linux | jw runs on PTYs and a unix socket |
| `git` ≥ 2.40 | worktrees |
| `claude` (Claude Code) or `codex` | the agent pane |
| `nvim` | the editor pane; files you open from jw go to it |
| `gh` | PR state in the sidebar, and for removing merged worktrees |

## Install

```sh
git clone https://github.com/brya0x/jw && cd jw && cargo install --path crates/jw --locked
```

---

## The model

- **Session.** A named list of workspaces: `main`, `salesassist`, `testing`. The sidebar shows
  the session on screen. `jw new <name>` starts one, `^␣ a` switches between them. Every pane
  has `$JW_SESSION`.
- **Workspace.** A folder (any folder, git or not) or a git worktree of a project. It holds
  panes laid out in splits.
- **Daemon.** It owns every terminal, so closing the TUI (`^␣ q`) or the window keeps
  everything running. `jw` attaches again. A restarted daemon brings the workspaces back, with
  their scrollback, and each agent resumes its own conversation.
  `jw server status` says whether it runs; `jw server stop` stops it, closing every pane.

## Screen

```
┌ main        ^␣ a sessions ┬ jitsubai/auth-flow  feat/auth-flow  from main  PR #42 open  ~/ws/jitsubai-wt/auth-flow ┐
│ 1 ● jitsubai              │╭ editor  nvim app/login/actions.ts ─╮╭ agent  claude ──────────╮                        │
│ 2   ↳● auth-flow ✻ ⚡     ││ …                                  ││ ✻ Reading …             │                        │
│ 3   ↳● billing   ⚑        ││                                    ││ > _                     │                        │
│ 4 ● notes                 │├ shell  zsh ───────────────────────┴┴─────────────────────────┤                        │
└ TERM  ^␣ then a key · ␣ switch · o open · w worktree · t pane · a sessions · ? all keys ─────────────────────────────┘
```

- **Sidebar marks:** `●` open, `○` closed, `✻` the agent is working, `?` it is waiting for
  you, `⚡` a dev server runs, `⚑` the PR is merged.
- **Header:** follows the focused pane, like a shell prompt. It shows the repo and branch of
  the pane's current directory.
- **Mouse:** click, scroll and drag the line between panes to resize. Shift+drag selects text.

## Keys (after `Ctrl-Space`)

| Go | | Worktree | | Panes | |
|---|---|---|---|---|---|
| `␣` | switch workspace | `w` | new worktree | `hjkl` | focus |
| `tab` | previous workspace | `r` | rename | `HJKL` | move |
| `1-9` | workspace by number | `s` | sync with base | `t` | new shell |
| `o` | open a folder | `d` | changes (diff) | `x` | close |
| `/` | open a file | `X` | remove | `n` | name |
| `a` | sessions | | | `f` | full screen |
| `,` | settings | | | | |
| `q` | detach | | | | |

- **Renaming:** `^␣ r` on a worktree renames its branch and its folder. On a plain folder it
  only changes the name jw shows. `ctrl-r` in `^␣ a` renames a session, and `^␣ n` names a
  pane.
- **`^␣ X`:** after the PR is merged it asks once. When work would be lost (changes not
  committed, commits not pushed), you type the worktree's name to confirm.
- **`^␣ d`:** opens the diff against the base on the right. It shows one column when the
  terminal is narrow and two side by side when it is wide. `↵` opens a file in nvim, or in
  the Markdown reader for a `.md`.

All of these except `1-9 hjkl HJKL ? q` can be rebound in `^␣ ,`.

## Settings and themes

`^␣ ,` opens the settings screen, which writes `~/.config/jw/settings.json`. Every key is
optional:

```json
{
  "leader": "C-Space",
  "theme": "system",
  "dark": "one-dark",
  "light": "one-light",
  "which_delay_ms": 600,
  "keys": { "sessions": "g" }
}
```

Every theme is a JSON file. Ten are built in, a dark and a light one of each:
`one-dark`/`one-light`, `catppuccin-mocha`/`catppuccin-latte`, `tokyo-night`/`tokyo-night-day`,
`gruvbox-dark`/`gruvbox-light` and `solarized-dark`/`solarized-light`. Their files are in
[`crates/tui/themes/`](crates/tui/themes/). Your own go in `~/.config/jw/themes/<name>.json`;
a file named like a built-in replaces it. jw applies changes to these files within 2 s, and
programs in panes (nvim, codex, claude) are told the new background.

A theme names its dark or light mode and its colours as `#rrggbb`. The schema,
[`themes/schema.json`](crates/tui/themes/schema.json), describes each colour, and naming it
in `$schema` gives your editor validation and completion:

```json
{
  "$schema": "https://raw.githubusercontent.com/brya0x/jw/main/crates/tui/themes/schema.json",
  "name": "my-night",
  "dark": true,
  "colors": {
    "bg": "#1a1b26", "panel": "#16161e", "line": "#292e42", "fg": "#c0caf5", "dim": "#565f89",
    "sel": "#283457", "blue": "#7aa2f7", "green": "#9ece6a", "yellow": "#e0af68",
    "red": "#f7768e", "magenta": "#bb9af7", "cyan": "#7dcfff"
  }
}
```

The diff colours (`add_bg`, `del_bg`, `add_word`, `del_word`) may be left out. When missing,
they are mixed from green and red.

From the terminal, `jw theme` does the same without opening the settings:

```sh
jw theme                                   # every theme: dark or light, built-in or file, in use
jw theme export gruvbox-dark > mine.json   # a theme to start from
jw theme install mine.json                 # checks it and copies it to ~/.config/jw/themes/
jw theme install https://example.com/t.json --name t   # or from a URL (curl), or - for stdin
jw theme use mine                          # the dark or light theme, by its "dark"
```

In the settings' themes page, `c` copies a theme to your themes folder and `e` opens a theme
file in nvim.

---

## Projects

A project's config is looked up in this order:

1. `<repo>/.jw.toml`: commit it if your team wants the same setup.
2. `~/.config/jw/<project>.toml`: personal, matched by remote URL.
3. Defaults: worktrees in `<repo>-wt/`, branch `feat/{name}`, no setup, no ports, and the
   layout nvim + claude + shell.

```toml
match  = "github.com/acme/myapp"          # which repo this applies to
root   = "~/code/myapp-wt"                # where worktrees go
branch = "feat/{name}"                    # branch name template
setup  = ["pnpm install --frozen-lockfile"]
sync   = "rebase"                         # or "merge"

[agent]
default = "claude"
claude  = { start = "claude", resume = "claude --continue" }
codex   = { start = "codex",  resume = "codex resume --last" }

[layout]                                  # the panes a worktree opens with
editor = "nvim"
split  = "down"
ratio  = 0.68
a = { split = "right", a = { run = "editor" }, b = { run = "agent" } }
b = { split = "right", a = { run = "shell" }, b = { run = "dev:web" } }

[ports]                                   # offset inside the worktree's block
web = 0
api = 2

[dev]                                     # what a dev:<service> pane runs
web = ["pnpm -F web dev --port {port.web} --strictPort"]

[[env]]                                   # copied from the main checkout, then rewritten
file = "apps/web/.env.local"
set  = { API_URL = "http://localhost:{port.api}" }
```

- **Placeholders:** `{name}`, `{base}`, `{slot}`, `{port.<service>}`.
- **Ports:** every worktree gets a slot, and every slot owns 100 ports:
  `port = 20000 + slot × 100 + offset`. Panes get `JW_SLOT`, `JW_PORT_BASE` and
  `JW_PORT_<SERVICE>`.
- **Agents:** jw starts claude with its own conversation id and hooks that tell jw whether it is
  working, waiting or idle. Your own claude settings and hooks still apply.

---

## For coding agents

An agent in any pane can see and drive the other workspaces of its session:

```
jw ls [--json]                                   # workspaces, state, marks, branch, PR
jw read <workspace> [--pane role] [--lines N]    # a pane's last lines, scrollback included
jw worktree <name> [--in <project>] [--task "…"] # a new worktree, with a task for its agent
jw prompt <workspace> "<task>"                   # type a task into an open workspace's agent
```

`jw skill install` puts the Claude Code skill that teaches an agent these commands in
`~/.claude/skills/jw/`. Run it again after updating jw.

A workspace in one session can oversee the others. That is how a "brain" agent keeps track
of the rest.

## State

| File | What |
|---|---|
| `~/.local/state/jw/workspaces.json` | every worktree, with its session |
| `~/.local/state/jw/sessions/<name>/` | a session's folders and recent list |
| `~/.local/state/jw/session.json`, `scrollback/` | the daemon's pane trees and output |
| `~/.config/jw/settings.json`, `themes/` | settings and themes |

## Development

```sh
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

CI runs exactly that on every pull request, on Ubuntu and macOS. The code is a workspace of
five crates (`crates/core`, `proto`, `daemon`, `tui`, `jw`). [AGENTS.md](AGENTS.md) has the
map and the rules, and each crate has its own. The design and its history are in
[docs/specs/rust-tui.md](docs/specs/rust-tui.md).

## License

MIT
