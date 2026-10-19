# The Claude test path

What it is for: the user's definition of done (2026-10-19, point 1) is a path that tries every
feature of Claude Code in jwn, so that jwn can replace Claude's own screen for daily work. This
file is that path. Part 1 says what Claude Code has and where jwn stands; Part 2 (not written
yet) is the runnable path, a script against the real engine and a manual list for what needs
eyes.

## 1. The inventory

Taken on 2026-10-19 from the real engine, not from the spec: `claude` 2.1.303 run headless as
jwn runs it (`-p --input-format stream-json --output-format stream-json --verbose
--include-partial-messages --permission-prompt-tool stdio --model haiku`) in a throwaway git
repo in `/tmp` with the person's own `~/.claude` (so the transcripts land under
`~/.claude/projects/-private-tmp-…`), one `initialize` request and one turn. Its `system/init`
line and its answer to `initialize` list what follows; `claude --help` lists the flags.

| Claude Code has | Count |
|---|---|
| Slash commands built in to the engine (not skills) | 48 (of 115 listed; 67 are skills and plugin skills) |
| Tools in the headless engine | 31 (the init list; it has no `Grep`, `Glob` or `TodoWrite`, see `claude-engine.md`) |
| Agents | 9 (`claude`, `Explore`, `general-purpose`, `Plan`, `scout`, `statusline-setup`, plugins') |
| Models | 13 (`default`, `opus`, `fable`, `sonnet`, `haiku`, and full ids) |
| Output styles | 5 (`default`, `Proactive`, `Concise`, `Explanatory`, `Learning`) |
| MCP servers / plugins | 4 / 8 (in this person's config) |
| Flags of `claude --help` | 67 |

### Where jwn stands, in three columns

**A. Built, and tried only against the fake engine** (`crates/daemon/tests/fake-claude.py`; every
automated test of the conversation uses it; no test or script runs the real binary). The path has to
run these against the real one: starting, resuming and continuing; sending, queue and interrupt;
the four modes and `/mode`, `/plan`, `⇧tab`; permission asks and their answers; questions
(`AskUserQuestion`, several shapes); plans (`ExitPlanMode`) and their three answers; the checklist;
tool rows, diffs, folding and the running-tool list; subagents (`Task`), background tasks, `Monitor`
and stopping a task; `/model`, `/effort`, `/rename`, `/color`, `/compact`, `/clear`, `/cost`,
`/exit`; `!` lines; `@` mentions and pasted images; the advisor row; the suggestion; thinking
summary; the turn's result line; a held session; older items; Markdown blocks; resume after a daemon
restart; errors and a dead engine. By hand, once, on 2026-10-16: a turn, `Write` asking, an
interrupt, `/effort` (`claude-engine.md`, «A whole conversation against the real engine»).

**B. Built, never run against the real engine** (the engine lists them; jwn sends what is typed):
41 built-in commands go to the engine as typed and nobody has looked at what comes back, among
them `/advisor`, `/agents`, `/autocompact`, `/config`, `/context`, `/fast`, `/focus`, `/goal`,
`/heapdump`, `/import`, `/mcp`, `/output-style`, `/recap`, `/reload-plugins`, `/reload-skills`,
`/usage`, `/usage-credits`, `/extra-usage`, `/skill-doctor`, `/list-agents`, `/team-onboarding`, and
the 67 skills and plugin skills (`/commit`, `/code-review`, `/simplify`, `/loop`, `/schedule`, …). The
doc says a few answered locally with no model call; the rest are unknown. Also here: `/init` and
`/security-review` (sent as prompts), the tools `EnterPlanMode`, `EnterWorktree`, `ExitWorktree`,
`CronCreate/Delete/List`, `ScheduleWakeup`, `Workflow`, `SendMessage`, `ListAgents`, `TaskStop`,
`RemoteTrigger`, `Skill`, `WebFetch`, `WebSearch`, `NotebookEdit`, `LSP` («not exercised», drawn as
a loop row, RISK-50).

**C. Not in jwn at all** (so a person who needs one still types `command claude`):
- Flags. `parse_args` serves 10 (`--resume`/`-r`, `--session-id`, `--name`/`-n`, `--model`, `--effort`,
  `--permission-mode`, `--add-dir`, `--settings`, `--agent`, `--continue`/`-c`) and refuses every other flag, and the binary then runs the
  original: `--dangerously-skip-permissions`, `--permission-mode bypassPermissions`,
  `--allowedTools` and `--disallowedTools`, `--tools`, `--mcp-config` and `--strict-mcp-config`,
  `--append-system-prompt` and `--system-prompt`, `--agents`, `--worktree`, `--fork-session`,
  `--from-pr`, `--bare`, `--fallback-model`, `--max-budget-usd`, `--plugin-dir`, `--setting-sources`,
  `--debug`, `--file`, `--brief`, `--tmux`, `--teleport`, `--remote-control`.
- The mode `bypassPermissions` (a conversation offers `manual`, `acceptEdits`, `plan`, `auto`).
- Commands that need Claude's own screen and are refused with «use `command claude`»: 12 of them
  (`login`, `logout`, `vim`, `theme`, `terminal-setup`, `ide`, `chrome`, `doctor`, `insights`,
  `upgrade`, `keybindings`, `release-notes`) plus the 56 that Claude lists as its own screens and the engine does not serve
  headless (`/add-dir`, `/rewind`, `/branch`, `/btw`, `/diff`, `/export`, `/hooks`, `/copy`,
  `/cd`, `/fork`, `/plugin`, `/sandbox`, `/statusline`, `/voice`, …).
- Output styles (`/output-style`), fast mode (`/fast`), `/focus`, remote control, background
  sessions (`--bg`, `claude attach`), the interactive screens of `/plugin`, `/hooks`, `/rewind`.

### What the inventory says about the next step

Column A is a long list that a script can run in a few turns of the cheapest model (about 0.06 USD a
turn, `claude-engine.md`). Column B is a list of unknowns: running them is how they become A or C. Column C
is a list of decisions, and only the person can say which of them belong in «done».
