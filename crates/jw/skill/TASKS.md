---
name: {bin}-tasks
description: Use right after you have made a plan, whenever you run inside {bin} (the environment variable {pane_env} is set). Turns the plan into this workspace's task list and works it until everything is done. The person can also call it with a list or a goal of their own.
---

# {bin} tasks: work a plan to the end

You run inside a pane of a {bin} session. The task list of this workspace is a Markdown file,
`{tasks_file}`, in the folder you work in (a worktree has its own). A task is a line
`- [ ] t1 title (after: t0)`: `[x]` is done, `[!]` is blocked, and without `after:` the order of the
file is the order of work. The last task, `fin`, always exists: it waits for all the others and is
where the tests and their evidence go.

**When you are called with text** (`/{bin}-tasks ...`): if it is a list (several lines), keep the
person's wording and order and start it; if it is a goal (one sentence), look at the project, draft
the list, show it, and wait for the person's OK before the loop. With no text, the list is the plan
you just made.

1. Write the plan as `{tasks_file}`: one line per task, ids `t1`, `t2`, ... and `after:` only where one
   task needs another. If the file already has tasks, never replace it: add the new ones at the end,
   never remove or untick a line that is done, and never rewrite the list while a loop is on
   (`{bin} tasks loop status` says).
2. Run `{bin} tasks ls`: it checks the ids and the dependencies, shows the order and adds the last
   task if you did not write it. Mend what it reports.
3. Run `{bin} tasks loop on`. It prints the first task; each time you end a turn, {bin} gives you
   the next ready one, and it stops by itself when everything is done, a task is blocked, or the
   laps run out (`--max N`, 20 by default).
4. Each task: do it, run `{bin} tasks done <id>` and end your turn. If you need the person, do not
   ask: run `{bin} tasks block <id> "why"` and end your turn. `{bin} tasks note <id> "..."` adds a
   line under a task.
5. The last task, `fin`: run the project's tests and CI, note the result and the evidence with
   `{bin} tasks note fin "..."`, ask the advisor to review before you finish, then run
   `{bin} tasks done fin`. If they fail, run `{bin} tasks block fin "why"`.

Never mark done what you did not do. `{bin} tasks next` says the next ready task and
`{bin} tasks loop off` stops the loop.
