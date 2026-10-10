---
name: jw
description: Use when working inside a jw workspace (a pane of the jw TUI; JW_* variables in the environment, JW_SESSION set) or when overseeing, creating or handing work to other workspaces of a jw session. Covers jw ls, jw read, jw worktree, jw prompt, jw sessions, jw help, and the JW_* variables.
---

# Working inside jw

`jw` is a terminal workspace manager. A **session** (`main`, `salesassist`, …) is a list
of **workspaces**. A workspace is either a folder or a git worktree of a project, and it
holds panes: an editor, an agent, shells, and dev servers. You run in one of those panes,
and `$JW_SESSION` says which session you belong to. Every command below acts on that
session.

## Your pane's variables

| Variable | What |
|---|---|
| `JW_SESSION` | the session |
| `JW_NAME`, `JW_PROJECT`, `JW_ID` | your workspace |
| `JW_PANE_ID` | your pane |
| `JW_SLOT`, `JW_PORT_BASE`, `JW_PORT_<SERVICE>` | in a worktree: its ports |

In a worktree, use its ports (`$JW_PORT_*`) in URLs, curl calls and browser checks. The
default ports (5173, 8081…) belong to the main checkout, not to you.

## Seeing the other workspaces

- `jw ls` lists the session's workspaces with their state: `working`, `waiting` (the
  agent asks for something), `idle` or `closed`. It also shows their marks (`✻` agent
  working, `?` waiting, `⚡` dev server), branch and PR. `jw ls --json` gives the same
  thing as JSON.
- `jw read <workspace> [--pane <role>] [--lines N]` prints the last lines of a pane,
  scrollback included. It reads the agent's pane by default. Use it to see what an agent
  is asking, or how a build ended. `<workspace>` is a name from `jw ls`
  (`project/name` for a worktree, or just the folder's name).

## Driving them

- `jw worktree <name> [--in <project>] [--task "<what to do>"]` creates a worktree of the
  project, either the one in the current folder or the `--in` project of this session. It
  opens the worktree with its layout and hands the task to its agent. Add
  `--branch <existing>` to work on a branch that already exists, or `--from <ref>` to
  branch from something other than the project's base.
- `jw prompt <workspace> "<task>"` types a task into the agent of a workspace that is
  open. It waits until the agent is quiet, so it never interrupts it.
- `jw sessions` lists the sessions and how many of their workspaces run.

## Never do these

- **Don't answer an agent's permission question for it** with `jw prompt`. If `jw ls`
  shows `waiting`, read the question with `jw read` and tell the person. They answer it in
  that pane.
- **Don't remove worktrees, branches or registry entries by hand**, and don't edit
  `~/.local/state/jw/*.json`. Removing, syncing and finishing a worktree are done in the
  TUI (`^␣ X`, `^␣ s`), which checks for unpushed work first.
- **Push yourself; jw never pushes.**
- `jw new <name>` starts a *session*, not a worktree. A worktree is `jw worktree`.

## When unsure

`jw help` lists every command, and `jw <command> --help` gives its usage. This skill ships
with jw: `jw skill install` writes the copy that matches the jw on PATH.
