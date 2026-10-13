# State sync: one model, owned by the daemon

Status: design note, not agreed. Nothing here is built, and no spec was changed.
Evidence: `docs/qa/QA-REPORT.md` (bug ids B-nn below) and the code read on 2026-10-13.

## The problem

The phone and the TUI show the same workspaces, but what they say about each one is worked out separately:

| Where | What it computes | Citation |
|---|---|---|
| TUI | the marks `✻ ? ⚡ ⚑` from `PaneInfo` (hooks first, then `busy` and `bell` as a guess for a Claude without hooks) and the PR map | `crates/tui/src/tui/mod.rs:1162-1184` |
| `jw ls` | a state word and marks from the same panes | `crates/jw/src/cli.rs:818` (`state_of`) |
| Phone | marks again, in Kotlin: `Working`, `Waiting`, `Dev`; the merged-PR flag is left out ("the bridge cannot know") | `android/proto/src/main/kotlin/dev/jw/proto/Workspaces.kt:58,136` |

The rows themselves are already shared: `core::sidebar::rows` feeds both the TUI and the bridge (`crates/core/src/sidebar.rs:26`).
What is not shared is the *activity* of each row, and the TUI also keeps state the daemon never sees: it polls the panes every 2 s
(`tui/mod.rs:1287-1289`), asks GitHub itself (`poll_prs`, `tui/mod.rs:1131`), caches PRs (`tui/prcache.rs`) and reads the registry and
folders straight from disk (`reload`, `tui/mod.rs:1070`).

What the QA saw because of it:

- A Claude started by hand works and the phone's Home row shows nothing, while `jw ls` says `working ✻` (B-22). This is what the owner saw with `ci-status`.
- The merged-PR mark never reaches the phone (B-20); a fresh agent at its idle prompt reads "waiting" (B-16); "finished" has no signal (B-21); the phone and the PC name a workspace differently (B-17).
- A reconnect re-asks several watches separately and the app comes back on the Home list or on the first tab (B-08, B-26).
- Two Claudes in one folder both show the newest conversation (B-30), because the daemon picks a transcript by folder when it has no id.
- Home and Avisos offer y / n for something the person has not seen; the default preset sends Enter and accepted a plan "and used auto mode" (B-19).

## The idea

The daemon owns the state *and* the view model. The TUI and the phone are clients: they receive the same document and draw it in their own way
(terminal cells, Compose). What travels is meaning (this workspace is working, that agent waits for a plan), not layout.

Terminal bytes and screen rows stay on their own frames (`crates/proto/src/proto.rs:725-780`: frame kind 1, a JSON head and raw bytes); they are not in the document.

Why not send a widget tree: the TUI draws cells and the phone draws bubbles and blocks, so a tree would need a second renderer, and it would still
not contain what only Claude's terminal draws (`/status`, `/config`: B-03).

## The model (proposal)

One document per session, with a revision number.

```
SessionState { rev, workspaces: [Workspace] }
Workspace    { id, kind: root|worktree|folder, name, project, branch, path, exists,
               pr: { state, ci }?, activity: [working|waiting|dev|merged] }
Pane         { id, role, program, exited: status?, agent: Agent, conversation: SessionId? }
Agent        { state: idle|working|waiting|exited, source: hook|transcript|screen, since,
               waiting_for: { kind: permission|question|multi|plan|screen, tool, summary }? }
```

- `activity` is derived by the daemon from the panes' agents, so no client counts marks again.
- `waiting_for` is built from the pending tool call, the last `tool_use` with no `tool_result`, which the chat already tracks and summarises
  (`crates/daemon/src/chat.rs:645-698`; the plan text at `:689`). With no transcript call (the trust dialog, a picker) its kind is `screen` and it has no summary.
- A Claude without jw's hooks gets its state from the transcript and the screen (`esc to interrupt`, `crates/daemon/src/lib.rs:493`), with `source` saying so.
- A pane is tied to its conversation by id (the pane's arguments, or what the hooks report), not by "newest in the folder".
- `exists` is false when a workspace's folder is gone; `exited` says an agent ended, so a client can say it (B-28).

## The protocol (proposal)

- `StateWatch{session, since}` from a client. The daemon answers a snapshot, or the patches since `since` if it still has them.
- `StatePatch{from, to, ops, digest}`: operations against revision `from`, and a hash of the document at `to`. A client whose hash differs asks for a snapshot.
- The daemon coalesces changes (about 300 ms) and keeps the last few minutes of patches for catch-up.
- JSON for all of it; the frames already carry JSON (`proto.rs:725-780`), so the document is a new message, not a new transport.
- Nothing is kept for old clients. `PROTOCOL` is 25 (`proto.rs:18`); a different number is refused on `Hello`, as today.
- The chat blocks, the shell blocks and the screen text keep their own watches (`ChatWatch`, `ShellWatch`, `TextView`: `proto.rs:138-154`). Whether the chat joins the document is open.

## What the UI does with it

- The Home row and Avisos stop offering y / n. A waiting row says what waits (`plan · <title>`, `pregunta · <question>`, `permiso · Write b.txt`,
  `Claude espera en pantalla`) and its button opens the full-screen question the app already has. This follows the owner's decision recorded in the QA report.
- The TUI reads the same document and loses `App::marks`; `jw ls` prints it.

## What it does not fix

- What only Claude's terminal draws: `/status`, `/config`, the `/model` picker footer, the mode line (B-03, B-04). They need a "ver pantalla" toggle and a Shift+Tab key.
- The phone's own bugs: the day/night crash (B-01), `ls` columns (B-05), `less` (B-06), the theme (B-07), markdown (B-11), box characters (B-18).
- PC-side bugs: the branch label that does not follow `git switch` (B-24) and agents that do not resume after a restart (B-29).

## Order of work (proposal, not agreed)

1. The daemon computes `activity` and `waiting_for`; the phone reads them. Closes B-22, B-20, B-16, B-21, B-17 and B-19.
2. The TUI reads the document and drops `marks`, `poll_prs` and `prcache`.
3. Revisions, patches and resync. Closes B-08 and B-26.
4. Conversation ids per pane. Closes B-30.
5. Decide whether the chat's pending question and plan become part of the document.

## Risks

- **The TUI is the large part.** 23 call sites read the registry and folders from core (`tui/mod.rs`, `modal.rs:227,946`, `registry_watch.rs:23`), and the sidebar, status bar and pickers read `marks` or `daemon_panes`. The rule `core, proto ← tui` stays true (`AGENTS.md`).
- **Who writes the registry.** The TUI, the CLI and the daemon all write `workspaces.json` and `session.json`; if the daemon owns the document it must own those writes or watch them, or the document drifts from disk.
- **Freshness.** `waiting_for` from the transcript is as fresh as the 2 s chat poll (`daemon/src/lib.rs:1612`); a prompt can appear on the PC before the row says what it is.
- **Screen-only prompts** (trust dialog, pickers) can say `screen` and nothing more.
- **PR and CI move into the daemon**, which then needs `gh` and a network; a failure should read "unknown" and never hold a patch.
- **Cost of the document** (patch log, digest per watcher) is unmeasured.

## Open questions

- Patch format: RFC 6902 JSON Patch, or a smaller keyed merge of our own.
- Measure message sizes and latency before committing; the QA's latencies are upper bounds and it measured no sizes.
- Phone first or TUI first.
- Whether the chat blocks join the document or stay on `ChatWatch`.
- A new spec file, or addenda to `docs/specs/mobile.md` and `docs/specs/rust-tui.md`, which hold the requirements it would replace (mobile REQ-18, 20, 28, 55; rust-tui REQ-51, 73).
- `AGENTS.md` asks for an approved HTML prototype before any UI change: the Home row that says what waits needs one.
