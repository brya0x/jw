# Claude Code as an engine (what the conversation reads and writes)

Home of the facts the daemon relies on when it runs `claude` headless (spec: *The conversation*, REQ-371 and following). Everything here was run against **Claude Code 2.1.301** on 2026-10-15 unless it says *docs* (stated by the official pages under `code.claude.com/docs`) or *unverified* (neither). A message shape that is not listed is ignored, never an error (RISK-51).

## The command

```
claude -p --input-format stream-json --output-format stream-json --verbose \
  --include-partial-messages --permission-prompt-tool stdio \
  [--session-id <uuid> | --resume <id>] [--name <n>] [--model <m>] [--effort <e>] [--permission-mode <mode>]
```

- stdin takes one JSON object per line; closing stdin ends the process with status 0 after the last result. *tested*
- A person's message: `{"type":"user","message":{"role":"user","content":"text"}}`. *tested* Images: `content` as blocks with `{"type":"image","source":{"type":"base64","media_type":…,"data":…}}`. *docs, unverified*
- `--permission-mode` takes `acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan` (`claude --help`); the conversation offers the first four of them with `plan` and `manual`. `--effort` takes `low`, `medium`, `high`, `xhigh`, `max`. `-n`/`--name` names the session.
- The person's own `~/.claude` applies: its SessionStart hooks and MCP servers show in the stream (`system/hook_*`, `mcp_servers` with `needs-auth`).

## What comes out (one JSON object per line)

| `type` / `subtype` | What it carries | Notes |
|---|---|---|
| `system/init` | `session_id`, `cwd`, `model`, `permissionMode`, `tools`, `slash_commands` (names; 115 on the author's machine, skills and plugin commands included), `terminal_slash_commands` (`doctor color focus reload-plugins`, which the stream-json engine does run: see the `initialize` row), `skills`, `agents`, `plugins`, `mcp_servers`, `capabilities`, `claude_code_version` | emitted **at the start of every turn**, not once. *tested* |
| `system/status`, `system/thinking_tokens` | `requesting`; `{estimated_tokens, estimated_tokens_delta}` | *tested* |
| `stream_event` | `message_start`, `content_block_start/delta/stop` (`text_delta`), `message_delta`, `message_stop` | only with `--include-partial-messages`; **never nested** (a subagent's text is not streamed) *tested* |
| `assistant` | `message.content[]` of `text` and `tool_use` blocks, `parent_tool_use_id` (null on the main thread), `uuid` | *tested* |
| `user` with `tool_result` | `tool_use_id`, `content`, `is_error?`, and a rich `tool_use_result` | *tested* |
| `user` with a string and `isReplay: true` | an echo: `<command-message>x</command-message><command-name>/x</command-name>` for a command or skill (the expanded body is **not** echoed), `<local-command-stdout>…</local-command-stdout>`, `<task-notification>…<event>tick 1</event></task-notification>` | the last one **starts a turn with no input from the person** *tested* |
| `system/permission_denied` | `tool_name`, `tool_use_id`, `decision_reason` | what the default mode does with a tool that needs permission and no prompt tool *tested* |
| `control_request` / `can_use_tool` | `request_id`, `request{tool_name, input, description, permission_suggestions, tool_use_id}` | with `--permission-prompt-tool stdio`; answer below *tested* |
| `system/compact_boundary` | `compact_metadata{trigger, pre_tokens, post_tokens}` | after `/compact`; the summary comes as a `user` line *tested* |
| `conversation_reset` (top level) | `new_conversation_id`, `trigger: "clear"` | after `/clear`; the session id changes *tested* |
| `system/session_title_changed` | `title` | after `/rename x` *tested* |
| `system/background_tasks_changed`, `task_started`, `task_updated`, `task_notification` | `task_id`, `task_type` (`local_bash`, `local_agent`, monitors), `tool_use_id`, `is_backgrounded`, `patch{status, end_time}`, `summary`; `output_file` **only** in `task_notification` (the path of a Bash result's text, `Output is being written to: …`, is how a running shell's file is known; a Monitor result has none; both are `local_bash`) | background Bash, Monitor, subagents *tested*; captures in `crates/daemon/tests/captures/` (2.1.304) |
| `system/task_progress` | `task_id`, `description`, `usage{total_tokens, tool_uses, duration_ms}`, `last_tool_name` | a running subagent *tested* |
| `rate_limit_event` | `rate_limit_info{status, rateLimitType, unifiedWindows{five_hour, seven_day}}` | *tested* |
| `result` | `subtype` (`success`, `error_during_execution`), `result`, `is_error`, `num_turns`, `terminal_reason` (`completed`, `aborted_streaming`), `total_cost_usd` (cumulative per process), `usage`, `permission_denials`, `duration_ms`, `session_id` | one per turn *tested* |

## What goes in

| Message | Effect | Notes |
|---|---|---|
| `{"type":"control_response","response":{"subtype":"success","request_id":"…","response":{"behavior":"allow","updatedInput":{…}}}}` | allows the tool | *tested* |
| the same with `{"behavior":"deny","message":"…"}` | the tool fails with that message and Claude says so | *tested* |
| `{"type":"control_request","request_id":"…","request":{"subtype":"interrupt"}}` | answers `{still_queued:[]}`, then a user line `[Request interrupted by user]` and a `result` with `aborted_streaming`; the process stays up | *tested* |
| `{"type":"control_request","request_id":"…","request":{"subtype":"initialize"}}` | answers `{commands: [{name, description, argumentHint, builtin?, aliases?}], agents, models: [{value, resolvedModel, displayName, supportedEffortLevels…}], current_permission_mode, account, claude_code_version, …}`: the names **and descriptions** of every command and skill (`builtin: true` for Claude's own, a `:` in the name for a plugin's), and where each is defined is **in the description's text, not in a field**: a plugin's begins with its name in parentheses (`(vercel) …`), a personal skill or `~/.claude/commands` command ends in `(user)`, a project's in `(project)`, and the skills of claude.ai (no `:` in the name, an alias `anthropic-skills:…`) in `(claude.ai sync)`; bundled skills (`dataviz`, `run`) are `builtin`, and the models with the one `default` stands for, which is what the `/` list and the model shown need; the daemon sends it when an engine starts (id `jwn-init`) and again when the person's `/reload-plugins` or `/reload-skills` ends. The list is read **when asked**: a skill file made after the engine started is not in it (122 commands) until one of those lines has run (123); both run headless as a turn of `num_turns: 0` and cost 0 (a synthetic `assistant`, one `system/init` and a `result`), and the request is answered at once even in the middle of a turn, which then goes on | *tested* 2026-10-16; the reload and the mid-turn request, and the marks of origin (124 commands), 2026-10-23 (claude 2.1.309) |
| `{"subtype":"set_permission_mode","mode":"plan"}` | changes the mode of the engine that runs (`manual`, `acceptEdits`, `plan`, `auto`): answers `control_response` success `{mode}` and then `system/status` with `permissionMode`; no restart and no new cache | *tested* 2026-10-16 |
| `{"subtype":"set_model","model":"sonnet"}`, `{"subtype":"set_max_thinking_tokens","max_thinking_tokens":2000}` | answer `control_response` success | accepted *tested*; the conversation changes the model with the line `/model x` (it prints «Set model…»), which also works |
| `stop_task`, `rewind_files`, `get_context_usage`, `mcp_*` | the SDK has methods of those names, "streaming input mode only" | the names are in the binary; the **wire shape is unverified** (RISK-50) |

## Commands sent as a message

A line that starts with `/` is a command (only at the start of a message). Tested in this mode:

- **Local, no model call** (`num_turns` 0, text in `result`): `/context` (a markdown table), `/usage` and `/cost` (both the subscription's use, **not** the session's cost), `/recap`, `/goal`, `/agents`, `/mcp`, `/config`, `/model` (prints, or sets: *docs*), `/effort`, `/rename x`, `/color red`, `/compact`, `/clear`.
- **Prompt commands and skills**: a project command (`.claude/commands/x.md`), a skill (`.claude/skills/x/SKILL.md`), a plugin skill (`plugin:skill`) and `/doctor`, `/init`, `/review` run as a normal turn; they are in `slash_commands`.
- **Answered «`/x` isn't available in this environment»** as a success: `/help /status /resume /rewind /tasks /artifacts /hooks /permissions /memory /skills`. No picker, no hang.
- **Not shell mode**: `!echo hi` goes to the model as text.
- **Slow or unknown**: `/doctor` ran as a turn of many `Bash` calls (all refused by the sandbox of the test) and `/insights` started a model call with no result in 20 s.

## Tools that have a surface

| Tool | Seen | What a client draws |
|---|---|---|
| `ToolSearch` | loads the schema of a deferred tool (`tool_reference` content) | hidden |
| `AskUserQuestion` | arrives as `can_use_tool` with `input.questions[]` = `{question, header, options: [{label, description}], multiSelect}` (up to four questions in one call). The answer is an **allow** whose `updatedInput` is the same input plus `answers`: `{"<question text>": "<label>"}`; several labels of a `multiSelect` are one string joined with `", "`; any other text is taken as the person's own answer («Other»), and the labels of a `multiSelect` with the own words after them are one string too (`"Rust, Kotlin, mi lenguaje"` reaches Claude as `The user answered: "¿Qué lenguajes?"="Rust, Kotlin, mi lenguaje"`; capture `crates/daemon/tests/captures/ask-multi-own-2.1.304.jsonl`, *tested 2026-10-23*, version 2.1.304). An allow with no `answers` ends as «The user did not answer the questions» *tested 2026-10-16* | a question: the options, one question at a time |
| `ExitPlanMode` | arrives as `can_use_tool` with `input = {plan (markdown), planFilePath}`; in `plan` mode the plan is first written with `Write` to `~/.claude/plans/…md` (no permission asked). **Allow** → the tool answers «User has approved your plan» and a `system/status` with `permissionMode: default` follows; a `set_permission_mode acceptEdits` sent at once takes effect in the same turn, so the next `Write` is not asked. **Deny** with a message → the plan is revised (the message goes to Claude as an error result) *tested* | a plan: «aprobar y editar sin preguntar», «aprobar preguntando», «seguir planeando» |
| `Task` (the subagent in this version; `Agent` in others) | `tool_use` `{description, subagent_type, prompt, run_in_background}`; `system/task_started` `{task_id, tool_use_id (the Task's), description, subagent_type, task_type: local_agent, is_backgrounded, prompt}`; `system/task_progress` `{task_id, tool_use_id, description, usage{total_tokens, tool_uses, duration_ms}, last_tool_name}`; the subagent's own tool calls arrive as `assistant` messages with `parent_tool_use_id` = the Task's; **its permission requests arrive as ordinary `can_use_tool` with `agent_id` (= `task_id`) and the nested `tool_use_id`**; `task_updated` `{patch: {status: completed, end_time}}`, `task_notification` `{status, summary, output_file}`; the final `tool_result` carries `tool_use_result` with `totalDurationMs`, `totalTokens`, `totalToolUseCount` *tested* | a card with its progress and totals, its steps when opened |
| `Bash` with `run_in_background` | `background_tasks_changed {tasks: [{task_id, task_type: local_bash, description}]}` (the list of what runs, whole each time), `task_started` (`is_backgrounded: true`), the immediate `tool_result` «Command running in background with ID»; stopping it: see `stop_task` *tested* | a task row |
| `Monitor` | needs permission; each line it prints arrives as a replayed `task-notification` and **opens a turn**; ends with `task_notification` completed | a task row and a note per line *tested (N81)* |
| `EnterPlanMode` | no permission asked in `auto`; `tool_use {}` then `system/status` `permissionMode: plan`, then the tool result «Entered plan mode»; at `ExitPlanMode` allowed the engine says `status` with the mode it came from (`auto` here, `default` from manual) before the result *tested 2026-10-20, started and resumed, `--permission-mode auto`; capture `crates/daemon/tests/captures/plan-from-auto-2.1.304.jsonl`* | the plan's answers (REQ-605) |
| `--chrome` and `/chrome` | `--chrome` at the start gives the server `claude-in-chrome` (`connected`) and about fifteen tools (`mcp__claude-in-chrome__navigate`, `…__computer`, `…__tabs_context_mcp`, …), which answer; without it there is no such server. `/chrome` sent as a message is answered «`/chrome` isn't available in this environment» and adds nothing *tested 2026-10-20, 2.1.304, headless stream-json* | no surface of its own; the tools are Claude's |
| `PushNotification` | `tool_use` `{message, status}`, no permission asked; the result says «Not sent — this terminal is active» (`pushSent: false`, `disabledReason: user_present`) when someone is at the terminal | a note with the message *tested 2026-10-16* |
| `TaskStop`, `ListAgents`, `SendMessage`, `Workflow`, `CronCreate/Delete/List`, `ScheduleWakeup`, `RemoteTrigger` | in the tool list of this version (with the person's config); not exercised | a loop row (RISK-50) |
| `TodoWrite`, `TaskCreate`, `TaskUpdate`, `Artifact` | **not in the tool list of this version's headless engine** (`CLAUDE_CODE_ENABLE_TASKS=1` and `…_TODOS=1` add nothing; a search for them answers «No matching deferred tools found»); `TodoWrite` reads `{todos: [{content, status: pending\|in_progress\|completed, activeForm}]}` in the SDK and is handled in case it comes, untested | the checklist and the pages stay empty in this version |

## Stopping a task

`{"type":"control_request","request_id":"…","request":{"subtype":"stop_task","task_id":"<id of task_started>"}}` answers `control_response` success `{}` and then, in this order, `task_updated {patch: {status: "killed", end_time}}`, `task_notification {status: "stopped"}` and `background_tasks_changed {tasks: []}`; the turn goes on (Claude is told the task ended). *tested 2026-10-16 with a background `Bash`*

## Where a conversation lives

The transcript is `~/.claude/projects/<folder with / and . turned into ->/<session_id>.jsonl`; `--resume <id>` works with stream input (tested) and `--no-session-persistence` leaves none. Resuming a session that was started in Claude's own terminal screen from `-p` was not tested.

## A whole conversation against the real engine (2026-10-16)

Through a private daemon: a turn (text streamed, its cost), `Write` asking permission with `permission_suggestions` (so «for this session» is offered), allowed from another client, the file written; an interrupt in the middle of a long turn («aborted_streaming», the process stays); `/effort low` (the engine answers with text as an `assistant` message, which is the command's output and not Claude's); the mode changed to `plan` while the engine ran; the daemon stopped and started again; `--resume` of the session answered from the first message of the conversation. A turn costs what it costs in Claude's own screen: with the author's plugins and skills a turn with a cold cache is about 0.1 USD even on `haiku`, and a new process after `--resume` pays that cache again once.

## The Stop hook, as a way to keep an agent going (N180, measured 2026-10-25, claude 2.1.309)

Four runs of a real `claude --model haiku` in a private temp directory (`--setting-sources project --settings <file>` with one `Stop` command hook; nothing of the person's daemon or conversations; the processes were killed only by their own pid, and the directory and the sessions it made were deleted afterwards). The hook logged its stdin and, for the first *n* calls, answered `{"decision":"block","reason":"…"}`.

| Question | Measured |
|---|---|
| Does `{"decision":"block","reason":"…"}` on stdout (exit 0) keep the agent going? | Yes. The same turn goes on: a synthetic `user` line `[{"type":"text","text":"Stop hook feedback:\n<reason>"}]` appears in the stream, then a `system/notification`, and the agent does what the reason says (it wrote the file the reason named). The agent sees it labelled as a stop hook's feedback, and says so in its text. *tested* |
| What does the hook get on stdin? | One JSON object: `session_id`, `transcript_path`, `cwd`, `prompt_id`, `permission_mode`, `effort`, `hook_event_name: "Stop"`, **`stop_hook_active`**, `last_assistant_message` (the text of the turn that is ending), `background_tasks`, `session_crons`. *tested* |
| What is `stop_hook_active`? | `false` the first time a turn tries to stop, `true` on every stop that follows a block of the same turn (it did not go back to `false` over six blocks in a row). It is a flag, not a counter, and **claude does not cap the loop**: a hook that blocked six times in a row was obeyed six times (seven `Stop` calls, ten model turns, one `result`). Whatever stops the loop (no ready task, a blocked one, a maximum of laps) is the hook's own business. *tested* |
| A new person's message? | The flag is `false` for the first stop of a turn, so a lap counter has to live outside the process (a file or the daemon), not in `stop_hook_active`. That it resets at the next message was not seen: the two messages of the run were queued together and claude took them as one turn. *unverified* |
| Does it arrive in the headless engine (`-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio`)? | Yes, with `--settings` on the command line: `Stop` fired, the block was honoured, and the stream carried **one** `result` for the whole continued turn (`num_turns` counted the extra model turns), so the reducer sees one turn that is longer, not two. *tested* |
| Does the daemon's engine pass that file today? | No. `launch.rs` writes `claude-hooks.json` and only terminal panes get `--settings` (`panes.rs`); `engine.rs` adds `--settings` only when the person put one in the arguments. `jw hook` also routes by `JW_PANE_ID` (`hook.rs:29`, a silent exit without it), which only `pty.rs:101` puts in a child's environment; the engine's child gets `cfg_env` and no pane id (`engine.rs:321-333`). So for the engine to loop it needs **both** the same `--settings` file and the pane id in its environment (`--settings` is additive with the person's own hooks). *read in code, not run through the daemon* |
| How does the model take the `reason`? | haiku read it as «Stop hook feedback», said so, and followed it because it matched what the person had asked. A bare order from a hook may be refused by a stronger model, so the `reason` should name the person's file and the task («next ready task in `.jw/tasks.md`: t2 endpoints»), not just command. *unverified for sonnet and opus* |
| A permission waiting? | With `--permission-mode default` and the permission request (`control_request`/`can_use_tool`) left unanswered for 30 s, `Stop` did not fire: the turn has not ended. A permission, an `AskUserQuestion` and an `ExitPlanMode` plan are `can_use_tool` requests, so the hook is never reached while one waits. *tested for a permission; the other two follow from the same request, unverified* |
| A question asked in plain text? | `Stop` fires like any other end of turn, and `last_assistant_message` is the only thing the hook could read to guess it. That is a guess, not a signal: the way out is to say in the `reason` what to do instead of asking («if you need the person, run `jw tasks block <id> "why"`»). *by design, unverified* |

Not measured: the interactive terminal (the same `Stop` hook jw already installs for attention; the block was not tried there), `exit 2` with stderr as the other way to block, `--resume` of a session that was kept going by a hook, and the cost of the extra turns (they pay the cache again like any turn).

### Getting the agent to use a skill after its plan (N180, 2026-10-25, claude 2.1.309)

Three runs of `claude -p --model haiku` in a private temp directory, with `JW_PANE_ID` set and one project skill `jw-tasks` (it shows in the `system/init` skills). The prompt asked for a plan of three steps and then to run it. One run each, so these are first readings, not rates.

| Question | Measured |
|---|---|
| Does the agent invoke the skill on its own, by its `description` («use right after you have made a plan, whenever you run inside jw»)? | **No.** It made the plan in text and did the work; no `Skill` call. *tested, one run* |
| With a louder description («MANDATORY — invoke this skill … before doing any step»)? | **No**, the same. *tested, one run* |
| With a `UserPromptSubmit` hook that returns `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"You run inside jw. After you make a plan, use the jw-tasks skill (Skill tool) before doing any step of it."}}`? | **Yes.** The agent called `Skill` right after its plan, wrote `.jw/tasks.md` with the closing `fin` task the skill asked for, and ticked `[x]` as it worked (it also wrote `(after: none)`, a slip the format has to tolerate). So `additionalContext` reaches the model and moves it. *tested, one run* |
| Does the nudge fire for a message written to the engine's stdin (`--input-format stream-json`), the engine's way? | **Yes** (2026-10-25, one haiku run in a private temp dir with `--setting-sources project`): the `UserPromptSubmit` hook ran once for the one message, and the agent called `Skill` for the project skill before doing the work and wrote the list. So the nudge reaches a conversation. *tested, one run* |
| Can the daemon say something to the agent when the person approves a plan in the engine? | No text channel: approval is `{behavior:"allow", updatedInput}` with the input unchanged (`conversation.rs` `answer_plan`, `permission_reply`); only a refusal carries text. *read in code* |
| Is the nudge available in the engine? | Same as the Stop hook: the engine gets no hooks `--settings` and no `JW_PANE_ID` today. *read in code* |

**When the person calls the skill** (`/jw-tasks …` as the message; haiku, `-p`, one run each, with a `package.json` of three dependencies and nothing installed):

| Question | Measured |
|---|---|
| A goal in one sentence, `/jw-tasks actualizar las dependencias del proyecto`? | The skill ran as a normal turn (the path the engine already has for skills typed as a message). The agent read the project, drafted seven tasks with `after:` plus the closing `fin`, wrote `.jw/tasks.md` and, as the skill said, **stopped to ask the person to review it**. *tested, one run* |
| A list pasted after the command (four bullets)? | The agent **kept the wording and the order**, numbered them `t1…`, added `fin`, and — as the skill said — went on to work the list. Where npm was refused it did what it could, ticked three tasks and, for the one it could not do, wrote `(bloqueado: …)` in free text instead of using `[!]`; it also added notes in parentheses to the lines it finished. So the agent edits the file by hand and writes free text: the parser has to tolerate it (and `jw tasks block`/`note` are what give it structure). *tested, one run* |
| Name | `/tasks` is claude's own command (answered «isn't available in this environment» in the engine, see above), so a skill called `tasks` would collide with it; `jw-tasks` does not. *read in this doc* |

Not measured: the same with sonnet or opus; a plan made in plan mode (`ExitPlanMode`) and a `PostToolUse` hook on it answering with `additionalContext`; the interactive terminal; how many runs out of ten take the nudge.

### The task loop in the engine (N180d2, 2026-10-25, claude 2.1.309)

One run of the stage `loop` of `scripts/claude-path.py` (haiku, a private daemon, a throwaway repository, the person's own `~/.claude`; the transcript it made was deleted): a list of two small tasks (`create uno.txt`, `create dos.txt`), the final task written by `tasks_loop_on`, and a conversation started in `acceptEdits` whose permissions the stage answers yes.

| Question | Measured |
|---|---|
| Does the engine run the hook? | Yes, started with `--settings <state>/claude-loop-hooks.json` (only `Stop`) and `<PREFIX>_PANE_ID` in its environment: `jw hook stop` found its pane and the daemon's socket. *tested, one run* |
| Does the loop go on until the list ends? | Yes: the loop ended on its own with «all 3 tasks done in 4 laps», every task `[x]` in the file and both files made. Four laps for three tasks means one task was handed out twice; which one, and whether the agent ticked the tasks with `jw tasks done` or edited the file, was not recorded (the transcript was deleted). *tested, one run* |
| How many `result` lines for the whole loop? | **One**, as measured with `-p` before (a turn that is longer, not four turns); the conversation shows a note for each lap. *tested* |
| Did the agent do the final task? | The final task is `[x]` and has a note under it that begins «No test suite…», so it looked for checks and noted that it found none. The advisor sentence was in the task's text and whether the agent followed it was not checked. *tested, one run* |

Not measured: sonnet and opus following the reason (OPEN-71); an interrupt by the person (RISK-132); a permission asked in the middle of a lap (the stage answers it; the loop paused until then by design, REQ-908).

## Cost and limits

A turn costs what the same turn costs in Claude's own screen, through the person's account; a call of about a dozen tokens of output cost about 0.06 USD, almost all cache creation. `total_cost_usd` in `result` is cumulative for the process.
