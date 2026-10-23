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
- Flags. `parse_args` serves 13 (`--resume`/`-r`, `--session-id`, `--name`/`-n`, `--model`, `--effort`,
  `--permission-mode`, `--add-dir`, `--settings`, `--agent`, `--continue`/`-c`, `--chrome`, `--no-chrome`, `--prompt-suggestions`) and refuses every other flag, and the binary then runs the
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

## 2. The path

### 2.1 The script: `scripts/claude-path.py`

Runs jwn's conversation against the real `claude`, stage by stage, in a private daemon
(`/tmp/jwn-path`: its own socket and state) on a throwaway git repo in `/tmp`, with the person's own
`~/.claude` and the cheapest model. It checks structure (an ask of this kind, a tool item of this
name, the status back to `idle`, a file written or not), never wording. It is not part of `just ci`:
it costs tokens (about 0.003 USD a turn on haiku) and needs the person's login.
`scripts/claude-path.py --list` names the stages; `scripts/claude-path.py turn tool` runs those.
It stops only the daemon it started, by its recorded pid.

| Stage | What it proves against the real engine | REQ |
|---|---|---|
| `turn` | a message streams, the turn ends with its result, the model and session id | 378, 380, 382 |
| `tool` | `Bash` asks permission (a command that writes, not `echo`, which the engine allows by itself); an allow runs it and writes the file; a deny is `denied` and writes nothing | 384–387 |
| `modes` | the engine confirms `acceptEdits`, `plan`, `manual`; `acceptEdits` writes with no ask | 389, 390 |
| `plan` | a plan arrives as an ask with its text; refusing it ends the turn | 436 |
| `question` | `AskUserQuestion` is an ask of options; an answer reaches Claude | 435 |
| `interrupt` | a long turn stops with a note and the engine answers again | 383 |
| `local` | `/rename`, `/color`, `/effort`, `/mode`, `/cost`, `/context`, `/compact` (divider), `/clear` | 418–421 |
| `tasks` | a background shell is a running task; `conversation_stop_task` ends it | 428, 432 |
| `shell` | a `!` line runs in the folder and its output is in the item | 394 |
| `resume` | after the daemon restarts, `--resume` brings the old items back and Claude remembers | 391, 540 |
| `edit` | an `Edit` asks with a diff and the tool item keeps it | 379, 384, 556 |
| `agent` | a `Task` subagent is a card | 429, 430 |
| `queue` | a message sent during a turn waits in the queue and is answered after it | 382 |
| `commands` | a skill of the folder's `.claude/skills` is listed as the project's and its description has no mark (the engine's marks of origin, N172a) | 792, 793 |
| `survey` | what the 17 commands that jwn sends as typed answer (not a pass or fail) | 420 |

### 2.2 Result of the first run (2026-10-19, claude 2.1.303, haiku)

All 13 checked stages pass (about 45 checks). Every failure of the first run was the script's own: the
engine allows `echo` without asking, `/cost` is an `out` item and not text, a `!` line has to be waited
for until it stops running, a refused tool is `denied`, and a restarted daemon keeps its workspace. None
was a bug of jwn. Column A of the inventory is therefore tried against the real engine for these
features; what is not yet in the script is on the manual list below.

`survey` (column B): the real engine answers each of the 17 commands in under 3 seconds, as text,
with no model call except `/context`, `/recap`, `/usage` and `/skill-doctor`:
- `/advisor`, `/autocompact`, `/config`, `/context`, `/goal`, `/output-style`, `/recap`,
  `/reload-plugins`, `/reload-skills`, `/usage`, `/skill-doctor`, `/list-agents` work and print their
  state (a list, a usage line, a markdown table). Setting forms exist (`/advisor <fable|opus|sonnet|off>`,
  `/config key=value`, `/autocompact`).
- `/agents` answers «the wizard has been removed», `/mcp` «use `/mcp` in the terminal for details».
- **`/fast` answers «Fast mode is not available in the Agent SDK» and `/focus` «isn't available here
  yet»**: the headless engine refuses them, so jwn cannot offer them while it runs the engine this way.
  This changes what column C can hold (the user chose it whole on 2026-10-19): `/fast` and `/focus`
  wait for the engine.
- `/output-style` lists the styles, so choosing one is a command to send, not a screen to build.

### 2.3 What still needs eyes (the manual list)

Each step is typed in a pane of the terminal client run on its own state dir (`JWN_STATE_DIR` and
`JWN_SOCKET` set, as the script does) against a throwaway repo, and looked at in the terminal client and, when connected, on the phone.

| Look at | Do | Expect |
|---|---|---|
| Folding (REQ-558) | ask for five reads and greps in a row | one row «Buscó … ejecutó …», a click opens it |
| Working line (REQ-563, 579, 580) | send a long turn | the line with time and tokens, a blank row above, a spinner on the workspace's row |
| Diff of an edit (REQ-556) | allow an `Edit` | the diff in the tool row, `^O` opens it |
| Images (REQ-407, 447) | paste a screenshot, send a photo from the phone | `[Imagen#1]`, Claude describes it |
| `@` mentions (REQ-408) | type `@` and letters | the files of the folder, one is written in |
| Suggestion (REQ-572, 573) | finish a turn, empty the field | the grey suggestion, `tab` takes it, `esc` puts it aside |
| Advisor (REQ-570) | have Claude consult the advisor | `● Advising using …` and its answer |
| Held session (REQ-575) | open the same session in Claude's own screen | the field says who holds it and writes nothing |
| Older items (REQ-567–569) | open a conversation of more than 300 items and scroll up | `▲ hay anteriores`, then 100 more |
| Markdown (REQ-465–468) | ask for a table and a list | blocks, not raw text |
| Modes on the footer (REQ-402, 578) | `⇧tab`, close, open a new conversation | the footer shows the last mode |
| Phone sheets (REQ-441, 444) | a permission and a plan on the phone | the sheet, the answer, the mark on Home |
