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
| `system/init` | `session_id`, `cwd`, `model`, `permissionMode`, `tools`, `slash_commands` (names; 115 on the author's machine, skills and plugin commands included), `terminal_slash_commands` (`doctor color focus reload-plugins`), `skills`, `agents`, `plugins`, `mcp_servers`, `capabilities`, `claude_code_version` | emitted **at the start of every turn**, not once. *tested* |
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
| `system/background_tasks_changed`, `task_started`, `task_updated`, `task_notification` | `task_id`, `task_type` (`local_bash`, `local_agent`, monitors), `tool_use_id`, `is_backgrounded`, `patch{status, end_time}`, `output_file`, `summary` | background Bash, Monitor, subagents *tested* |
| `system/task_progress` | `task_id`, `description`, `usage{total_tokens, tool_uses, duration_ms}`, `last_tool_name` | a running subagent *tested* |
| `rate_limit_event` | `rate_limit_info{status, rateLimitType, unifiedWindows{five_hour, seven_day}}` | *tested* |
| `result` | `subtype` (`success`, `error_during_execution`), `result`, `is_error`, `num_turns`, `terminal_reason` (`completed`, `aborted_streaming`), `total_cost_usd` (cumulative per process), `usage`, `permission_denials`, `duration_ms`, `session_id` | one per turn *tested* |

## What goes in

| Message | Effect | Notes |
|---|---|---|
| `{"type":"control_response","response":{"subtype":"success","request_id":"…","response":{"behavior":"allow","updatedInput":{…}}}}` | allows the tool | *tested* |
| the same with `{"behavior":"deny","message":"…"}` | the tool fails with that message and Claude says so | *tested* |
| `{"type":"control_request","request_id":"…","request":{"subtype":"interrupt"}}` | answers `{still_queued:[]}`, then a user line `[Request interrupted by user]` and a `result` with `aborted_streaming`; the process stays up | *tested* |
| `{"type":"control_request","request_id":"…","request":{"subtype":"initialize"}}` | answers `{commands: [{name, description, argumentHint, builtin?, aliases?}], agents, models: [{value, resolvedModel, displayName, supportedEffortLevels…}], current_permission_mode, account, claude_code_version, …}`: the names **and descriptions** of every command and skill (`builtin: true` for Claude's own, a `:` in the name for a plugin's), and the models with the one `default` stands for, which is what the `/` list and the model shown need; the daemon sends it when an engine starts (id `jwn-init`) | *tested* 2026-10-16 |
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
| `Agent` (subagent; named `Task` in older versions) | `tool_use` `{description, subagent_type, prompt, run_in_background}`; nested `assistant`/`user` messages with `parent_tool_use_id`; `task_started`, `task_progress`, `task_updated`, `task_notification`; the final `tool_result` carries `totalDurationMs`, `totalTokens`, `totalToolUseCount`, `resolvedModel` | a card with its progress and totals, its steps when opened *tested* |
| `Bash` with `run_in_background` | `tool_result` «running in background with ID»; `task_*` messages; the output in `output_file` | a task row *tested* |
| `Monitor` | needs permission; a command with a shell variable is refused («can't be checked»); each line it prints arrives as a replayed `task-notification` and **opens a turn**; ends with `task_notification` completed | a task row and a note per line *tested* |
| `ToolSearch` | loads the schema of a deferred tool (`tool_reference` content) | hidden |
| `TaskCreate`, `TaskUpdate`, `TaskList` (todos), `ScheduleWakeup`, `CronCreate/List/Delete`, `Workflow`, `Artifact`, `PushNotification`, `AskUserQuestion`, `ExitPlanMode` | in the tool list or in the docs; **not exercised** | a checklist, a loop row, a card with a link, a note, a question, a plan (RISK-50) |

## Where a conversation lives

The transcript is `~/.claude/projects/<folder with / and . turned into ->/<session_id>.jsonl`; `--resume <id>` works with stream input (tested) and `--no-session-persistence` leaves none. Resuming a session that was started in Claude's own terminal screen from `-p` was not tested.

## A whole conversation against the real engine (2026-10-16)

Through a private daemon: a turn (text streamed, its cost), `Write` asking permission with `permission_suggestions` (so «for this session» is offered), allowed from another client, the file written; an interrupt in the middle of a long turn («aborted_streaming», the process stays); `/effort low` (the engine answers with text as an `assistant` message, which is the command's output and not Claude's); the mode changed to `plan` while the engine ran; the daemon stopped and started again; `--resume` of the session answered from the first message of the conversation. A turn costs what it costs in Claude's own screen: with the author's plugins and skills a turn with a cold cache is about 0.1 USD even on `haiku`, and a new process after `--resume` pays that cache again once.

## Cost and limits

A turn costs what the same turn costs in Claude's own screen, through the person's account; a call of about a dozen tokens of output cost about 0.06 USD, almost all cache creation. `total_cost_usd` in `result` is cumulative for the process.
