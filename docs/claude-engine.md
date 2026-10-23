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

## Resume with an unanswered tool_use

What `claude` leaves in the transcript when its process ends while a question (`AskUserQuestion`), a plan (`ExitPlanMode`) or a permission (`can_use_tool`) waits for an answer, and what `--resume` does with it. *Tested 2026-10-23, Claude Code 2.1.309, `--model haiku`, each case in a private temp directory and its own session, the process ended by its recorded pid (or process group); `--permission-mode manual` for the question and the permission, `plan` for the plan; a first message that asks for the tool, then the pending `control_request`, then the end.*

**The transcript when the process is killed (SIGKILL) at the pending request.** It ends on the `assistant` line with the `tool_use` and nothing after it: claude writes nothing when it dies, and no `tool_result` exists. The `tool_use` of a permission (`Bash`) was already in the file at the moment the `can_use_tool` arrived (checked at 0 s); the question's and the plan's were there 1.5 s later (not checked sooner). The `can_use_tool` carries the same `tool_use_id` as that line, and for a question also `requires_user_interaction: true`.

**What `--resume <session>` does with that `tool_use`.** Nothing until the first user message reaches it. A process started with `--resume` that only gets the `initialize` request (`jwn-init`) and then ends leaves the transcript untouched, with the `tool_use` still unanswered. When a user message comes, claude first writes, **in the transcript only** (not on stdout), a `user` line with the `tool_result` `{"is_error": true, "content": "[Tool call interrupted: the session ended before this call's result was recorded, so its outcome is unknown. Check whether it took effect before relying on it or running it again.]"}` (the line also has `toolDenialKind: "interrupted"`) and an `assistant` line of `model: "<synthetic>"` with the text `No response requested.`; then the message, as a normal turn. It does not complain and does not ask again by itself.

| Case | What the person's message did (resumed with the same `--permission-mode`) |
|---|---|
| question, `elegí la opción B` | claude answered «Anotado: elegiste la opción B. La llamada anterior a la herramienta de pregunta se interrumpió antes de que tuviera resultado, así que tomo tu respuesta directamente del chat»; no new `AskUserQuestion` |
| plan, `Aprobado: seguí con el plan.` | claude went on with the plan's first step (`Write`), **no new `ExitPlanMode`**. The engine was still in `plan` mode, so that `Write` came as a `can_use_tool`; resumed with `--permission-mode acceptEdits` the `Write` ran with no request and the file was made |
| permission, `sí, adelante con el comando` | claude proposed **the same `Bash` again** (same command): a new `tool_use` and a **new `can_use_tool`**, which must be answered again. The first call never ran: the process died before the permission was answered |

**The transcript after the way `jw stop` ends an engine** (close its stdin, wait 3 s, kill the process group: `stop_all` and `finish` in `engine.rs`) is **not** the one above, because claude answers the pending request itself when stdin closes:

- **Question:** stdout `control_cancel_request`, two `user` lines and a `result` `error_during_execution`; the process ends with status 1 in about 1 s. The transcript ends with the `tool_result` «The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to…» and a `user` line `[Request interrupted by user for tool use]`.
- **Plan and permission:** the engine **does not stop**: the `tool_result` is «Tool permission request failed: AbortError: Tool permission stream closed before response received», the model **tries the same tool again** and gets «Tool permission request failed: AbortError: Stream closed», and may write a closing text; left alone it ended with status 0 after 3.9–4.3 s, so with a 3 s grace the group is killed in the middle of that and the transcript ends in one of those places (varies from run to run).

**A permission that waits and a tool that was already allowed and runs leave the same transcript.** With `Bash` `sleep 20 && touch ranfile`: killed (SIGKILL of the `claude` pid) while the permission waited, killed 3.5 s after it was allowed, and stopped as `jw stop` does (stdin closed, 3 s, group killed; `claude` was still alive at 3 s, waiting for the tool), the transcripts have the same lines in type and count, ending on the `assistant` `tool_use:Bash`, nothing after. So a pending permission **cannot be read from the transcript**: it needs a record of the daemon's own. The command of the permission that waited **never ran** (no `ranfile`), but the allowed ones **did finish by themselves** about 20 s later, also after the group kill (the tool's shell is evidently not in the killed group; not investigated), so a tool without a result may have completed.

So the unanswered `tool_use` at the end of the transcript is what a crash or a kill leaves, **not** what a deliberate stop leaves; after a stop the request is only recognised by those English error texts, which are claude's and may change.


## A whole conversation against the real engine (2026-10-16)

Through a private daemon: a turn (text streamed, its cost), `Write` asking permission with `permission_suggestions` (so «for this session» is offered), allowed from another client, the file written; an interrupt in the middle of a long turn («aborted_streaming», the process stays); `/effort low` (the engine answers with text as an `assistant` message, which is the command's output and not Claude's); the mode changed to `plan` while the engine ran; the daemon stopped and started again; `--resume` of the session answered from the first message of the conversation. A turn costs what it costs in Claude's own screen: with the author's plugins and skills a turn with a cold cache is about 0.1 USD even on `haiku`, and a new process after `--resume` pays that cache again once.

## Cost and limits

A turn costs what the same turn costs in Claude's own screen, through the person's account; a call of about a dozen tokens of output cost about 0.06 USD, almost all cache creation. `total_cost_usd` in `result` is cumulative for the process.
