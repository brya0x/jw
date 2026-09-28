# jw

**Parallel workstreams for coding agents.** One command gives each piece of work its own git
worktree, its own [herdr](#herdr) tab with an agent (Claude Code or Codex), an nvim diff view,
a dev-server pane, and its own block of ports — so three streams can run side by side without
stepping on each other.

```
jw new mobile-login        # worktree + branch + ports + setup
jw open mobile-login       # herdr tab: nvim diff | agent | dev servers
jw ls                      # what's open, what's pending, which PRs merged
jw close mobile-login      # free the tab, keep the work
jw done mobile-login       # PR merged? confirm → remove worktree + branch
```

> **Status: design stage.** This README is the spec. Commands described here are not
> implemented yet. See [Roadmap](#roadmap).

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
| **workspace** | one per project (`myapp`, `api`) — label comes from config |
| **tab** | one per worktree |
| **panes** | nvim, agent, dev — layout from config |
| **agent name** | the worktree name |

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

### `jw setup [name]`

Re-runs the config's `setup` commands in a worktree — the one you're standing in if no name
is given.

### `jw open [name] [--agent claude|codex|both] [--no-focus]`

Creates or focuses the herdr tab — the one you're standing in if no name is given.
Idempotent: a live tab is focused, not rebuilt; a tab closed by hand is recreated.

- The tab goes in the project's herdr workspace (`workspace` in config), created on first use.
- Every pane gets the `JW_*` variables — herdr doesn't pass env from a pane to its splits, so
  `jw` sets them on each one.
- The first open **starts** the agent; later opens **resume** it.
- With `--agent both`, Codex is named `<name>-codex`.
- `--no-focus` builds the tab without switching to it — for scripts and other agents.

A new worktree is a new folder, so Claude Code asks once whether you trust it. `jw` never
answers that dialog for you: it tells you the agent is waiting and leaves the answer to you.

### `jw ls [-a]`

Worktrees for the current project (`-a`: all projects).

```
NAME           ID        BRANCH              SLOT  TAB     PR          STATE
mobile-login   0b1c9e2a  feat/mobile-login   1     open    #412 open   2 unpushed
web-billing    7f3ad011  feat/web-billing    2     closed  #409 draft  dirty
api-webhooks   c21e8b40  feat/api-webhooks   3     closed  #401 merged ready for done
```

Reconciles against `herdr tab list` and `git worktree list`, so a tab or tree you killed by
hand doesn't leave a ghost.

### `jw dev <service>`

Runs inside the bottom pane. Starts the service's configured command(s) with this slot's ports
substituted. A service can be several commands (e.g. a package watcher + the app); each gets
its own split.

### `jw close <name>`

Kills the tab — agent and dev servers included. Keeps worktree, branch and slot. Warns first if
a dev server is running.

### `jw done <name>`

1. Checks the PR with `gh pr view <branch> --json state,mergedAt` — every layer, if it's a stack.
2. **Refuses** if anything is unmerged, uncommitted or unpushed.
3. Asks: `delete mobile-login? [y/N]`.
4. Closes the tab, `git worktree remove`, deletes the local branch, frees the slot.

Nothing is deleted without that confirmation.

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
workspace = "myapp"                          # herdr workspace label
branch    = "feat/{name}"                    # branch name template
setup     = [
  "pnpm install --frozen-lockfile",
  "pnpm turbo run build --filter='./packages/*'",
]

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
  between layers by checkout. `jw done` only proceeds when every layer is merged.

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

## Roadmap

- [ ] `jw new` / `jw ls` + registry
- [ ] `jw open` + herdr layout
- [ ] ports + `jw dev`
- [ ] `jw close`
- [ ] `jw done` + PR / stack checks
- [ ] `jw ls` interactive (bubbletea)

## License

MIT
