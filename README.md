# jw

[![CI](https://github.com/brya0x/jw/actions/workflows/ci.yml/badge.svg)](https://github.com/brya0x/jw/actions/workflows/ci.yml)

**Parallel workstreams for coding agents.** One command gives each piece of work its own git
worktree, its own [herdr](#herdr) tab with an agent (Claude Code or Codex), an nvim diff view,
a dev-server pane, and its own block of ports — so three streams can run side by side without
stepping on each other.

```
jw init                    # once per repo: draft a config from what it finds
jw new mobile-login        # worktree + branch + ports + setup
jw open mobile-login       # herdr tab: nvim diff | agent | dev servers
jw ls                      # what's open, what's pending, which PRs merged
jw close mobile-login      # free the tab, keep the work
jw done mobile-login       # PR merged? confirm → remove worktree + branch
```

> **Status: early.** Everything here works except stack-aware `jw done` — see
> [Roadmap](#roadmap).

---

## Why

Coding agents made it cheap to work on several things at once. The checkout didn't keep up:

- One working tree means one branch at a time. Switching branches restarts dev servers and
  confuses whichever agent is mid-task.
- Two copies of the same app fight over the same port (5173, 8081, 3000…).
- `git worktree add` gives you a second tree, but it's born without your gitignored
  `.env.local`, without `node_modules`, without built workspace packages — it doesn't boot.
- Agents, editors and dev servers end up scattered across random terminal windows, and you lose
  track of which one belongs to which branch.

`jw` treats a workstream as one unit:

```
1 stream = 1 worktree = 1 branch (or stack) = 1 herdr tab = 1 port block = 1 PR
```

---

## Requirements

| Tool | Why |
|---|---|
| `git` ≥ 2.40 | worktrees |
| [`herdr`](#herdr) | terminal workspace manager for AI coding agents — hosts the tabs and panes |
| `claude` (Claude Code) **or** `codex` | the agent in each tab |
| `nvim` + [diffview.nvim](https://github.com/sindrets/diffview.nvim) | the review pane |
| `gh` | PR status for `jw ls` / `jw done` |
| `gh-stack` *(optional)* | stacked PRs — `gh extension install github/gh-stack` |

Recommended: [gitsigns.nvim](https://github.com/lewis6991/gitsigns.nvim) for inline hunks
while the agent edits.

## Install

```sh
go install github.com/brya0x/jw@latest
```

`jw version` says which build you have — the commit and its date, read from what Go embeds in
the binary, so there's nothing to bump by hand:

```
$ jw version
jw v0.0.0-20260929155208-6f6d4986f92d (commit 6f6d498, 2026-09-29)
```

Or from source:

```sh
git clone https://github.com/brya0x/jw && cd jw && go build -o ~/.local/bin/jw .
```

---

## The tab

`jw open <name>` creates (or focuses) one herdr tab per worktree:

```
┌──────────────────────────┬──────────────────────────┐
│ nvim                     │ claude --continue        │
│ :DiffviewOpen base...HEAD│   or                     │
│                          │ codex resume             │
│ everything this branch   │                          │
│ changed, live            │ cwd = the worktree       │
├──────────────────────────┴──────────────────────────┤
│ dev shell — ports loaded, `jw dev <service>` ready  │
└─────────────────────────────────────────────────────┘
```

- **Left — nvim.** Opens `DiffviewOpen origin/<base>...HEAD`: the full diff of the branch
  against where it forked. As the agent writes, you review. `:DiffviewRefresh` to catch up.
- **Right — the agent.** Starts in the worktree, so it only sees this stream's files. On
  re-open it **resumes** instead of starting fresh (`claude --continue` resumes the latest
  conversation in that directory). `jw open --agent codex` uses Codex; `--agent both` puts
  them side by side.
- **Bottom — dev.** A shell with `.jw.env` loaded. `jw dev web` starts that service on this
  worktree's ports.

Closing the tab (`jw close`) kills the panes and dev servers. The worktree, branch and
uncommitted changes stay. `jw open` brings everything back, agent included.

---

## herdr

**herdr** is a terminal multiplexer built for coding agents: workspaces,
tabs, panes, and named agents you can list, prompt and wait on from the command line. `jw`
drives it; it never replaces it.

How `jw` maps onto herdr:

| herdr | jw |
|---|---|
| **workspace** | one per project (`myapp`, `api`) — label comes from config; `{name}` in it makes one per worktree |
| **tab** | one per worktree |
| **panes** | nvim, agent, dev — layout from config |
| **agent name** | the worktree name — or the workspace label, when each worktree has its own |

Because the agent is named after the worktree, you can drive it from anywhere — another pane,
another agent, a script:

```sh
herdr agent list                                  # which streams are working / idle / blocked
herdr agent prompt mobile-login "add the error state to the login form"
herdr agent read mobile-login --source recent-unwrapped --lines 80
```

That's the intended setup for orchestration: one "control" agent in its own workspace that
plans and routes, and one agent per `jw` tab that does the work.

Rules `jw` follows with herdr:

- Parses herdr's JSON responses for IDs; never guesses them.
- Creates panes with `--no-focus` unless you ran `jw open` yourself — it won't steal your focus.
- Only closes tabs and panes it created.

---

## Agents: Claude Code or Codex

Each tab runs one agent, started in the worktree directory. The agent needs no knowledge of
`jw` — it sees a normal repo on a normal branch.

| | Start | Resume on `jw open` |
|---|---|---|
| Claude Code | `claude` | `claude --continue` |
| Codex | `codex` | `codex resume --last` |

Both commands are configurable (`[agent]` in config), so flags like model or permission mode
live in one place.

Why worktree-per-agent instead of several agents in one checkout:

- **No crossed edits.** An agent running tests or a formatter can't touch another stream's
  files.
- **Clean context.** `git diff`, `git status` and search results only show this stream.
- **Safe review.** The nvim pane shows exactly what this agent changed and nothing else.

---

## nvim

The left pane is the review surface. What you get with diffview.nvim:

| Want | Command |
|---|---|
| Whole branch vs base | `:DiffviewOpen origin/main...HEAD` *(what `jw open` runs)* |
| Only uncommitted work | `:DiffviewOpen` |
| One layer of a stack | `:DiffviewOpen <parent-branch>...HEAD` |
| History of this branch | `:DiffviewFileHistory` |
| Refresh after agent edits | `:DiffviewRefresh` |

The base is never assumed to be `main` — `jw` reads it from `origin/HEAD` (so `development`
repos just work).

The nvim command is configurable (`[layout] editor`) if you prefer fugitive, neogit or plain
`git difftool`.

---

## Ports

Every worktree gets a **slot**, and every slot owns a block of 100 ports:

```
port = 20000 + slot × 100 + offset
```

Offsets come from config. With `web = 0`, `api = 2`, `metro = 81`:

| | web | api | metro |
|---|---|---|---|
| main checkout | 5173 | 8787 | 8081 *(stock defaults, untouched)* |
| slot 1 | 20100 | 20102 | 20181 |
| slot 2 | 20200 | 20202 | 20281 |

- Slots are allocated **globally across all projects**, so two repos never collide.
- `jw new` writes `<worktree>/.jw.env` with `JW_SLOT` and `JW_PORT_<SERVICE>`, and adds it to
  `.git/info/exclude` — never committed, shared by every worktree of the repo.
- Env files that point at `localhost:<port>` get rewritten per worktree (see `[[env]]` below),
  so the web app in slot 2 talks to the API in slot 2.

---

## Commands

### `jw init [--repo] [--print] [--force]`

Drafts a config for the repo you're in, from what it can see:

- **setup** from the lockfiles (`pnpm-lock.yaml` → `pnpm install --frozen-lockfile`, and the
  same for bun, yarn, npm, go, bundler, composer, cargo);
- **env files**: every `.env*` git ignores, except templates (`.example`, `.sample`…) and
  anything inside an ignored directory (`node_modules/`, nested worktrees);
- **localhost ports** in those files, written as a commented `set = …` for you to point at a
  service.

What needs your judgement — service names in `[ports]`, commands in `[dev]` — is left as
commented examples. The file is loaded back before `jw init` reports success.

By default it writes your personal `~/.config/jw/<project>.toml`, matched by the origin URL.
`--repo` writes `.jw.toml` in the repo instead, to commit; `--print` only shows the draft. An
existing config is never overwritten without `--force`.

### `jw new <name> [--from <ref>] [--branch <branch>] [--no-setup]`

1. `git fetch`, then `git worktree add <root>/<name> -b <branch> <ref>`
   (`<ref>` defaults to `origin/<default branch>`).
2. Assigns a free slot, writes `.jw.env`.
3. Copies the configured env files from the main checkout and rewrites their ports.
   A file missing in the main checkout is skipped with a warning.
4. Registers it. If any step up to here fails, the worktree and branch are removed.
5. Runs `setup` (install, build workspace packages…) with the `JW_*` variables exported.
   If setup fails, the worktree is **kept** — fix the cause and run `jw setup`.

Opens nothing.

**Working on a branch that already exists** — yours from before, or one a teammate pushed:
`jw new sync-center --branch feat/sync-center` checks it out as is instead of creating one.
A branch that only exists on origin gets a local branch tracking it. Without `--branch`, a name
that collides with an existing branch is an error, so nothing is reused by accident; `--from`
doesn't combine with it; and a rollback never deletes a branch `jw` didn't create.

### `jw setup [name]`

Re-runs the config's `setup` commands in a worktree — the one you're standing in if no name
is given.

### `jw open [name] [--agent claude|codex|both] [--no-focus]`

Creates or focuses the herdr tab — the one you're standing in if no name is given.
Idempotent: a live tab is focused, not rebuilt; a tab closed by hand is recreated.

- The tab goes in the project's herdr workspace (`workspace` in config), created on first use.
  `workspace = "{name}"` (or `"myapp {name}"`) gives every worktree a workspace of its own;
  closing its only tab closes it. The agent then takes the workspace's label as its name, so
  keep it to what herdr accepts: a lowercase letter, then a-z, 0-9, `-`, `_`, 32 at most.
- Every pane gets the `JW_*` variables — herdr doesn't pass env from a pane to its splits, so
  `jw` sets them on each one.
- The first open **starts** the agent; later opens **resume** it.
- With `--agent both`, Codex is named `<name>-codex`.
- `--no-focus` builds the tab without switching to it — for scripts and other agents.

A new worktree is a new folder, so Claude Code asks once whether you trust it. `jw` never
answers that dialog for you: it tells you the agent is waiting and leaves the answer to you.

### `jw ls [-a] [-i]`

Worktrees for the current project (`-a`, or outside any repo: all projects).

```
NAME           ID        BRANCH              SLOT  TAB     PR          STATE
mobile-login   0b1c9e2a  feat/mobile-login   1     open    #412 open
web-billing    7f3ad011  feat/web-billing    2     closed  #409 draft  dirty
api-webhooks   c21e8b40  feat/api-webhooks   3     closed  #401 merged ready for done
```

- **TAB** is checked against herdr: a tab you closed by hand is forgotten, not left as a ghost.
- **PR** comes from one `gh pr list` per project (`-` no PR, `?` gh unavailable).
- **STATE** is `missing` (worktree gone from disk), `dirty` (uncommitted or untracked files) or
  `ready for done` (PR merged, tree clean).

`jw ls -i` is the same list as a picker:

| Key | Does |
|---|---|
| `↑`/`↓`, `j`/`k` | move |
| `enter`, `o` | `jw open` the selected worktree |
| `c` | `jw close` it |
| `d` | `jw done` it — the confirmation is asked on the normal terminal |
| `s` | `jw sync` it |
| `x` | `jw rm` it — same guards and confirmation as the command |
| `a` | toggle this project / all projects |
| `r` | refresh |
| `q`, `esc` | quit |

Under the table: path, port range, PR link and state of the selected worktree. After `close` or
`done` it comes back with the result; after `open` it exits, since focus moved to the tab.

### `jw sync [name] [--merge]`

Brings the stream's branch up to date with the base branch (`origin/<default>`).

- It refuses a dirty tree and a rebase or merge already in progress, and says how to finish or
  abort it. A branch already up to date is left alone.
- It **rebases** by default; `--merge` merges the base in instead, and `sync = "merge"` in the
  config makes that the project's default.
- On conflicts it stops, lists the files and the commands to continue or abort, and exits
  non-zero. The stream's agent can pick it up from there.
- It never pushes. When a rebase rewrote commits that were already on origin, it tells you to
  publish with `git push --force-with-lease`.

### `jw rm [name] [--keep-branch] [--force]`

Removes a stream whatever its PR says — abandoned work, a PR closed unmerged, a worktree you
deleted by hand. It's `jw done` without the merged requirement, so it guards local work instead:

- It lists uncommitted files and commits that exist on no remote, and **refuses** if any of
  that would be lost. `--force` lets it go ahead — and it still asks.
- It deletes the worktree and the **local** branch, never the remote one. `--keep-branch` keeps
  the local branch too; a branch `jw new --branch` adopted is always kept.
- A worktree already gone from disk is pruned from git and dropped from the registry.

Nothing is removed without a yes on a terminal.

### `jw dev [service] [-w name]`

Starts a service from `[dev]` on this worktree's ports. Without a service it lists them, with
their commands already expanded. `-w` picks a worktree other than the one you're in.

Before starting anything it checks the ports the service's commands use. If one is taken it
starts nothing and says who holds it — process, pid, and whether it's already running in this
worktree:

```
jw: port(s) already in use, not starting web:
  20100 (web): pid 68080 — node vite --port 20100
      already running in this worktree: stop it, or use the pane it runs in
```

- **One command:** `jw` execs it — the service owns the terminal, so colours, `ctrl+c` and
  interactive keys (Expo's "press i") work as if you had typed it.
- **Several commands, inside herdr:** each one after the first gets its own split of the dev
  pane; the first runs in the pane you're in.
- **Several commands, outside herdr:** they run side by side with `[1]`, `[2]` prefixes on
  every line. `ctrl+c` stops all of them, including the servers their shells started.

### `jw close [name] [-y]`

Kills the tab — agent and dev servers included. Keeps worktree, branch and slot; `jw open`
brings it back. Closing a stream from inside its own tab works: the registry is updated before
the tab (and the shell running `jw`) goes.

If the dev pane is running something besides its shell, it asks first. Without a terminal to
ask on it refuses; `-y` closes without asking.

### `jw done [name]`

1. **Refuses** if the worktree has uncommitted or untracked files.
2. Finds the PR for the branch with `gh pr list --head <branch>` and **refuses** unless it is
   merged.
3. **Refuses** if the local HEAD has commits that aren't in what GitHub merged. This compares
   against the PR's head commit, not the upstream branch, so it still works after GitHub deletes
   the remote branch on merge.
4. Asks: `delete worktree … and branch …? [y/N]`. Without a terminal it refuses.
5. Closes the tab, `git worktree remove`, deletes the local branch, frees the slot.

The checks are about the branch the worktree is on **now**: if you (or an agent) switched to
another branch inside it, `jw` follows, says so, and also deletes the branch it first created —
only if git sees that one merged. A PR merged before the stream existed is recognised as an old
branch name being reused, not as this stream's PR.

Nothing is deleted without that confirmation. Run it from anywhere but the stream's own tab:
closing that tab would kill the shell `jw` runs in half-way, so `jw done` (and `jw rm`) refuse
there and say so. Stacks: each layer is its own branch, so today
`jw done` checks the one branch the worktree was created with.

Every command accepts the name or a prefix of the id.

---

## Config

Lookup order:

1. `<repo>/.jw.toml` — commit it if your team wants the same setup.
2. `~/.config/jw/<project>.toml` — personal, matched by remote URL.
3. Defaults — worktrees in `<repo>-wt/`, no setup, no ports, layout nvim + claude + shell.

```toml
# ~/.config/jw/myapp.toml
match     = "github.com/acme/myapp"          # which repo this applies to
root      = "~/code/myapp-wt"                # where worktrees go
workspace = "myapp"                          # herdr workspace label; "{name}" = one per worktree
branch    = "feat/{name}"                    # branch name template
setup     = [
  "pnpm install --frozen-lockfile",
  "pnpm turbo run build --filter='./packages/*'",
]

sync      = "rebase"                         # jw sync: "rebase" or "merge"

[agent]
default = "claude"
claude  = { start = "claude", resume = "claude --continue" }
codex   = { start = "codex",  resume = "codex resume --last" }

[layout]
editor = "nvim -c 'DiffviewOpen origin/{base}...HEAD'"

[ports]                                      # offset inside the slot's block
web   = 0
api   = 2
metro = 81

[dev]                                        # jw dev <service>
web    = ["pnpm turbo dev --filter=web^...", "pnpm -F web dev --port {port.web} --strictPort"]
api    = ["PORT={port.api} pnpm -F api dev"]
mobile = ["pnpm -F mobile exec expo start --port {port.metro}"]

[[env]]                                      # copied from the main checkout, then rewritten
file = "apps/web/.env.local"
set  = { API_URL = "http://localhost:{port.api}" }

[[env]]
file = "apps/api/.env.local"                 # copied as-is
```

Placeholders: `{name}`, `{base}`, `{slot}`, `{port.<service>}`.

---

## PRs and stacks

- **Single branch:** `gh pr create --base <default>` from the agent pane.
- **Stack:** with `gh-stack` installed, all layers of a stack live in **one** worktree; you move
  between layers by checkout. Checking every layer in `jw done` is on the roadmap.

The main checkout stays on the default branch forever. It's where you `pull`, and where new
worktrees fork from — nobody works in it.

---

## State

| Path | What |
|---|---|
| `~/.local/state/jw/registry.json` | one entry per worktree |
| `<worktree>/.jw.env` | slot + ports for that worktree |
| `~/.config/jw/*.toml` | personal project configs |

```json
{
  "id": "0b1c9e2a-…",
  "name": "mobile-login",
  "project": "myapp",
  "branch": "feat/mobile-login",
  "path": "~/code/myapp-wt/mobile-login",
  "slot": 1,
  "tab": "w4:t2",
  "pr": 412
}
```

`id` is a uuid and never changes. `name` is the handle you type — unique per project; across
projects use `project/name`.

---

## Tips

- **RAM is the real limit.** Worktrees are cheap; dev servers aren't. Keep at most two tabs
  with dev servers running and `jw close` the rest — they come back with `jw open`.
- **Name streams by intent**, not ticket number: `mobile-login` reads better in `herdr agent
  list` than `ABC-1234`.
- **One stream, one PR.** If a stream grows a second concern, `jw new` a second stream.

---

## For coding agents

`jw` is built to be driven by agents as much as by people.

**Teach your agent to use it.** The guide is compiled into the binary, so it always matches your
`jw`:

```sh
jw agents install                          # Claude Code skill: ~/.claude/skills/jw/SKILL.md
jw agents install --into ~/.codex/AGENTS.md  # Codex, or any repo's AGENTS.md (a marked block)
jw agents                                  # just print it
```

Re-running updates the copy in place. The guide is
[internal/core/guide/guide.md](internal/core/guide/guide.md).

**Inside a stream,** every pane has `JW_NAME`, `JW_SLOT` and `JW_PORT_<SERVICE>` in its
environment, and `jw info` shows the rest: branch, path, each port and whether something
listens on it, tab, PR, state.

**Machine-readable output.** `jw ls --json`, `jw info --json` and `jw new --json` print the same
stream object. `new --json` keeps stdout for the JSON alone and sends progress to stderr.

```json
{
  "name": "web", "branch": "feat/web", "slot": 1, "port_base": 20100,
  "ports": { "web": { "port": 20100, "listening": true } },
  "open": true, "tab": "w4:t1",
  "pr": { "number": 412, "state": "open", "url": "https://github.com/acme/myapp/pull/412" },
  "state": ""
}
```

**Orchestrating.** `jw new <name> --task "…"` creates the stream, opens it without stealing
focus and hands the task to its agent; `jw prompt <name> "…"` does the same for an open one.
Neither waits for the work. Neither ever types into a dialog: if the agent sits at one (folder
trust, an approval), they stop instead.

**Exit codes** are part of the interface:

| Code | Meaning |
|---|---|
| 0 | done |
| 1 | failed — the message says why |
| 2 | wrong command or arguments |
| 3 | **a person has to decide**: a confirmation, a dialog. Don't retry — ask. |

Confirmations (`done`, `rm`, `close` with a dev server up) never accept an answer without a
terminal: an agent can't delete a worktree by accident.

## Architecture

```
main.go                  wires the real connectors into commands.App
internal/
├─ commands/             every jw command, one file each; talks to the world only through App
├─ connectors/           the contract: Multiplexer, PullRequests, Shell
│  ├─ herdr/             Multiplexer, via the herdr CLI
│  ├─ github/            PullRequests, via the gh CLI
│  ├─ system/            Shell, per OS (shell_unix.go / shell_windows.go)
│  └─ git/               worktrees and branches, via the git CLI
└─ core/                 jw's own logic, no outside dependencies
   ├─ config/            per-project TOML, placeholders, ports
   ├─ registry/          the worktree list on disk
   └─ envfile/           rewriting keys in .env files
```

Dependencies point one way: `commands` → `connectors` + `core`. `core` imports nothing from
the project, and `commands` imports no concrete connector — only `main.go` does. Supporting
another tool (tmux instead of herdr, another forge instead of GitHub) is a new package under
`connectors/` that satisfies the interface, plus one line in `main.go`.

## Roadmap

- [x] `jw new` / `jw ls` + registry
- [x] config, ports, env files, `jw setup`
- [x] `jw open` + herdr layout
- [x] `jw close`
- [x] `jw done` + PR checks
- [x] `jw dev`
- [ ] `jw done` across every layer of a stack
- [x] `jw ls -i` interactive (bubbletea)

## License

MIT
