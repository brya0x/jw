# Working inside a jw stream

`jw` runs parallel workstreams: each stream is its own git worktree, branch, herdr tab and
block of ports. You may be the agent inside one of those tabs, or an orchestrator creating
them. Either way, use `jw` for the things below instead of doing them by hand.

## If you are inside a stream

Your pane already has the stream's variables:

| Variable | What |
|---|---|
| `JW_NAME` | the stream's name |
| `JW_SLOT` | its slot |
| `JW_PORT_<SERVICE>` | the port of each service, e.g. `JW_PORT_WEB` |
| `JW_PORT_BASE` | the first port of the slot's block |

- **Know your stream:** `jw info` (or `jw info --json`) shows the branch, the path, every port
  and whether something already listens on it, the tab, the PR and its state.
- **Start dev servers with `jw dev <service>`**, never with the app's own `dev` script: `jw`
  puts them on this stream's ports, so they don't collide with other streams. `jw dev` with no
  service lists what exists. If it says a port is taken, the server is probably already running
  in the dev pane — use it, don't start a second one.
- **Use the stream's ports** (`$JW_PORT_*`) in URLs, curl calls and browser checks. The default
  ports (5173, 8081…) belong to the main checkout, not to you.
- **Catch up with the base branch with `jw sync`** (rebase; `--merge` to merge). It refuses a
  dirty tree: commit first. On conflicts it stops and lists the files — resolve them, `git add`,
  and `git rebase --continue` (or `git merge --continue`).
- **Push yourself; `jw` never pushes.** After `jw sync` rebased a branch that was already
  pushed, it tells you to publish with `git push --force-with-lease` — only do that if your
  instructions allow force-pushing.

## If you orchestrate streams

- `jw new <name> --task "<what to do>"` creates the worktree, runs setup, opens its tab without
  stealing focus, and hands the task to the stream's agent. Add `--branch <existing>` to work on
  a branch that already exists, `--json` to get the stream back as JSON on stdout.
- `jw prompt <name> "<task>"` hands a task to a stream that is already open. It returns at once;
  the work shows up in that stream's tab.
- `jw ls --json` lists every stream with ports, tab, PR and state. `ready for done` means the PR
  is merged and the tree is clean.

## Never do these

- **Don't answer a `jw` confirmation.** `done`, `rm` and `close` (with a dev server running) ask
  `[y/N]` because they delete things. Without a terminal they refuse — that is on purpose.
- **Exit code 3 means a person has to decide** (a confirmation, a dialog in an agent's pane).
  Don't retry it and don't look for a workaround: stop and ask the person.
- **Don't finish your own stream from inside it.** `jw done` and `jw rm` close the stream's tab
  first — the one you run in — so they refuse there. Tell the person the stream is ready; they
  run it from elsewhere.
- Don't delete worktrees, branches or registry entries by hand. `jw done` (merged PR) and `jw rm`
  (anything else) check for unpushed work first.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | done |
| 1 | failed — the message says why |
| 2 | wrong command or arguments |
| 3 | needs a person: ask, don't retry |
