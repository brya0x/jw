---
name: {bin}
description: Drive the other workspaces and agents of this {bin} session from your own pane: list them, read what a pane shows, send a task to another agent, make a worktree with its panes, open or close a pane. Use it when the task is to coordinate work across worktrees or agents, not to edit the project you are in.
---

# {bin}: the session you are running in

You run inside a pane of a {bin} session: a terminal the daemon owns, with other workspaces
(folders and git worktrees) and other panes beside yours. A pane is always an ordinary shell;
an agent (`claude` or `codex`) is the program running in it. `{pane_env}` in your environment is
your own pane's id, and these commands use it so that you never write to yourself.

Everything goes through the `{bin}` command. A workspace is named by its id, or exactly by its
name, branch or folder, or by the start of its name or branch when only one fits. A pane is named
by its id (or the start of it) or its title.

## Look

- `{bin} ls [--json]` the workspaces: name, kind, branch, how many panes, the most urgent mark
  (`permission`, `question`, `plan`, `ci_failed`, `shell_done`, `screen`, `agent_done`, `merged`).
- `{bin} pane ls <workspace> [--json]` its panes: id, title, the program running in it, and
  `claude` or `codex` when that is an agent.
- `{bin} read <workspace> [--pane P] [--lines N]` what the pane shows now (the agent's pane when
  there is one, else the first), the last 40 lines by default. When that agent is a `claude`
  conversation it prints the last N items instead (what the person said, what Claude answered,
  the tools it used), and a line that starts with `?` when it waits for an answer.
- `{bin} read <workspace> --pane P --blocks` for a shell pane: the last commands, each with its
  output and how it ended, instead of the raw screen.

Read before you write: look at what the other agent is doing or asking before you send it anything.
A workspace whose mark is `permission`, `question` or `plan` has an agent waiting for a person.

## Act

- `{bin} prompt <workspace> [--pane P] [--tag] [--now] [--wait [--timeout SECONDS]] [--] <text>`
  gives the text to the agent that runs in the pane. To a `claude` conversation it is a message,
  as if the person had typed it: it never answers a question the agent has waiting, and when the
  agent is busy the message waits its turn. To any other agent it types the text and presses
  Enter, after the agent's screen has been still for a second, so a task sent to an agent that
  has just started is not lost (`--now` writes at once). `--tag` starts the text with who sent
  it, so the other agent can answer you. Anything that follows `--` is the text, even if it
  starts with a dash. With `--pane` it types into any pane, a plain shell included, where it
  would run as a command.
  `--wait` (a `claude` conversation only) waits for the turn the message starts to end, up to
  10 minutes or `--timeout`, and prints the text of Claude's last answer instead of
  "sent to pane". If the turn stops because Claude asks something, it prints the question with
  its numbered options and exits 3: the person has to answer it.
- `{bin} worktree <name> [--in repo] [--branch B] [--from REF] [--task TEXT]` makes a git worktree
  of the repository you are in (or `--in`: a folder or a workspace), the way the person would, with
  what the repository's `.jw.toml` says (its folder, its branch template, its setup and its panes).
  It prints the workspace's id and path. With `--task` it hands the text to the agent that starts
  there, as `prompt` does.
- `{bin} pane open <workspace> [--split right|down] [--of P] [--] [program words]` opens a pane
  (a shell when no program is given) and prints its id.
- `{bin} pane close <workspace> <pane> [--force]` closes a pane. It refuses your own pane and the
  last pane of a workspace unless `--force`.

## Rules

- Exit codes: 0 done, 1 it failed (the message is on stderr), 2 a use it does not accept, 3 the
  agent waits for an answer (`prompt --wait`; the question is on stdout).
- Do not poll in a loop. Send the task, do your own work, and `read` when you need the result or
  when `ls` shows a mark on that workspace. Use `--wait` only when you cannot go on without the
  answer.
- Do not send a task to a terminal agent that is waiting for an answer from a person
  (`permission`, `question`, `plan`): your text would answer it. A `claude` conversation takes it
  as a message and does not answer anything; still, tell the person what waits.
- A worktree is a git worktree: its branch and files are the person's work. Do not remove
  worktrees; leave that to them.
