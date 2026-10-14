---
status: built (N0–N6); the UIs are next
scope: [next/contract, next/crates/contract, next/crates/daemon]
depends_on: []
supersedes: []
---
# next: a daemon that serves a tree of JSON nodes, for any client

## Contract
One daemon per session owns every terminal. Clients (TUI, phone, CLI) connect over one stream and exchange JSON objects, one per line (`t`-tagged). The state is a tree of **nodes** opened on demand: `session` (level 0: workspaces with a summary) → `workspace` (level 1: its panes) → `pane` (level 2: its screen). Every node has `id` (uuid), `v` (its document version), `epoch` (changes at every daemon start), `rev` (counts changes), and `attention`.

**The envelope never changes** (`hello`, `open`/`close`, `snapshot`/`patch`/`gone`, `cmd`/`reply`, `error`). Evolution happens in node documents: additive fields never bump `v` and receivers ignore unknown fields; a breaking change adds `v+1` of that one node type, served beside the old. At `hello` the client lists the versions it reads per node type (`accepts`) and the daemon answers the version it will serve (`accepted`); a type without a common version is not served. There is no global protocol number.

A client opens a node and gets a `snapshot`, then `patch`es (RFC 6902 subset `add`/`remove`/`replace`, `from`→`to` rev) until it closes it. A patch never edits `rev`: the client applies the ops to the cached document it holds at `from`, then sets `rev` to `to`; any other state means reopen for a snapshot. Opening with `epoch`+`since` resumes from a cache: the daemon replies one `patch` (empty when nothing changed) when the epoch matches and the revision is still retained, else a `snapshot`. The epoch sent is the one in the cached document, never the connection's. Clients cache nodes by `(epoch, id)`.

**Attention** is a typed item the daemon computes once and a client never re-derives: `question`, `plan`, `permission`, `shell_done` (with `exit_code`), `agent_done`, `screen`, `merged`, `ci_failed`. Each names the node that opens when tapped and the `rev` that raised it. A node's `attention` lists its whole subtree, so it bubbles to the session. Looking is shared: a client that shows a node sends `mark_seen {node, rev}` and the daemon ends the attentions that node raised up to that revision, so every client clears them (the user's decision; the first proposal was a per-client *seen*). Each client also keeps a local record so it clears at once and while offline. An attention lasts as long as its cause (a `shell_done` ends when the next command starts), so the daemon drops it then; whether a person looked at it is the client's *seen*. There is no approve/deny anywhere.

**Shell blocks.** A shell pane can also be read as the commands it ran. When the shell reports its marks (below), the pane carries `blocks`, the id of a `blocks` node (type `blocks`, v1, opened on demand like any node): the pane's commands, newest last, each with its folder, start time, duration, exit status and output. The output is rows of styled spans (text with colour and bold, italic, underline), resolved by the daemon, so a client draws spans and needs no terminal emulator. A command that takes the whole screen (nvim) is a block with `full` and no output, and the node is in mode `full` while it runs: clients then draw the pane's screen node instead. The designs are `docs/prototypes/tui.html` (terminal and phone). The shell's input stays with the shell: a client's editor sends the line with `write` (text, then `\r`), stops a program with `write` of `^C`, and repeating or copying a block is the client's business.

A **pane is a terminal**: its process is always the user's `$SHELL`; a program runs inside it (`running` and `last_program` carry it, and the pane's `title` is its name).

## Interfaces
Home of every shape is `contract/schema/contract.schema.json`; the examples are `contract/fixtures/{client,daemon,invalid}`; the Rust types are `crates/contract/src/lib.rs`. Do not restate them here.

| Direction | `t` | Purpose |
|---|---|---|
| client→daemon | `hello` | `client`, `accepts: {type: [v]}` |
| | `open` / `close` | `node`, optional `epoch`+`since` to resume |
| | `cmd` | `id`, `cmd` (below) |
| daemon→client | `hello` | `epoch`, `root` (session id), `accepted: {type: v}` |
| | `snapshot` | a whole `node` |
| | `patch` | `node`, `epoch`, `from`, `to`, `ops` |
| | `gone` | node removed or unknown |
| | `reply` | `id`, `ok` + `result?` or `error{code,message}` |
| | `error` | a message with no `id` to reply to |

The `blocks` node: `pane`, `mode` (`blocks` or `full`), `blocks`: `Block {id, cmd, cwd, started, running, code?, ms?, truncated?, full?, out}`, `out` = rows of `Span {t, fg?, bg?, b?, i?, u?}`, colours a palette index (0 to 255) or `#rrggbb`. Everything is in `contract/schema/contract.schema.json`; examples in `contract/fixtures/daemon/{snapshot-blocks,snapshot-blocks-full,patch-blocks-append}.json`.

Commands (`cmd`): `rename_session`, `open_folder`, `create_worktree`, `remove_workspace`, `new_pane`, `close_pane`, `write`, `resize`. Error codes: `bad_request`, `not_found`, `unsupported`, `conflict`, `git_failed`, `io`.

Command semantics (N3): `open_folder` takes an absolute path of an existing directory and opens it once (`conflict` if open; `not_found`, `bad_request` for a missing path, a file or a relative path). `create_worktree` makes `<repo parent>/<repo name>-wt/<branch with / → ->` with `git worktree add` (a new branch from `base`, or an existing branch checked out, never both) and opens it as `<repo>: <branch>`; names that are options or not branch names are `bad_request`, git's own message is `git_failed`. `remove_workspace` closes a workspace; with `remove_worktree` it first runs `git worktree remove` without force, so a worktree with changes stays and so does its workspace. A successful open replies `result: {workspace, path}`.

Panes (N4): `new_pane` starts the user's `$SHELL` (else `/bin/sh`) in the workspace folder on a PTY (24×80 until `resize`, at most 500 each) with `TERM=xterm-256color` and `JWN_PANE_ID` set (`identity::env_var`), and replies `result: {pane}`. The pane node's `screen` is the vt100 screen as text rows. A shell that exits takes its pane with it, like closing a terminal; `close_pane` and `remove_workspace` end the shells. After a daemon restart the saved panes get a fresh shell with the same ids (the screen and scrollback are not kept). A shell that prints OSC 133 marks (`ESC ] 133 ; D ; <code> BEL` finished, `ESC ] 133 ; C BEL` command started) raises `shell_done` with the exit code, which the next command start ends; shell integration scripts that print them for zsh, bash and fish come later.

Development link: with `JWN_TCP=127.0.0.1:7870` the daemon also listens on loopback TCP, and `adb reverse tcp:7870 tcp:7870` lets a phone on a cable reach it; any process on the machine can use it, so it is off by default and prints a warning. The phone gets pairing, TLS and the tunnel later.

Transport: JSON lines on a Unix socket (mode 0600, in a 0700 dir) whose path comes from `identity::socket_path()` (`crates/identity/src/lib.rs`). `session.json` in `identity::state_dir()` is private to the daemon (written whole, atomically; a file it cannot read is moved to `session.json.bad`, never overwritten). Client side: `crates/wire` (`Client` keeps the node cache and applies patches). The phone later carries the same objects one per WebSocket text message.

## Acceptance
- REQ-1  THE contract SHALL define every message and node as JSON Schema in `contract/schema/contract.schema.json`.
- REQ-2  THE contract crate SHALL parse every fixture in `fixtures/client` and `fixtures/daemon` and write back the identical JSON.
- REQ-3  THE contract crate SHALL validate every such fixture against the schema.
- REQ-4  IF a fixture in `fixtures/invalid` passes the schema, THEN the conformance suite SHALL fail.
- REQ-5  IF the schema has a message type, command, patch op, node type, attention kind or error code with no valid fixture, THEN the conformance suite SHALL fail.
- REQ-6  WHEN a document carries a field the receiver does not know, the receiver SHALL ignore it.
- REQ-7  THE envelope SHALL NOT change; a breaking change SHALL add a new `v` of one node type.
- REQ-8  WHEN a client sends `hello`, the daemon SHALL answer `accepted` with the highest version in common per node type and omit a type with none.
- REQ-9  WHEN a client opens a node with the daemon's `epoch` and a retained `since`, the daemon SHALL send one `patch`; otherwise a `snapshot`.
- REQ-10 WHEN a node's attention changes, the daemon SHALL patch it and every ancestor a client has open.
- REQ-11 THE daemon SHALL compute attention; a client SHALL NOT derive it.
- REQ-12 THE fixtures SHALL be plain files with no Rust dependency, so a Kotlin suite can read the same ones.
- REQ-13 THE product name, env prefix and paths SHALL come from `identity.toml` only.
- REQ-14 WHEN a pane starts, its process SHALL be the user's `$SHELL`.
- REQ-15 IF a daemon is already listening on the socket, THEN a second daemon SHALL refuse to start; a socket nobody listens on SHALL be cleared.
- REQ-16 WHEN the daemon restarts, the session id and workspaces SHALL be the same and the epoch new.
- REQ-17 IF a line is not a contract message, or comes before `hello`, THEN the daemon SHALL send `error` and keep the connection.
- REQ-19 WHEN `open_folder` names an existing directory not yet open, the daemon SHALL add a `folder` workspace and patch the session node.
- REQ-20 WHEN `create_worktree` succeeds, the daemon SHALL have a git worktree beside the repository and a `worktree` workspace for it.
- REQ-21 IF a worktree has changes, THEN `remove_workspace` with `remove_worktree` SHALL fail with `git_failed` and change nothing.
- REQ-22 WHEN a workspace is removed, the daemon SHALL send `gone` for its node to whoever has it open.
- REQ-23 WHEN `new_pane` succeeds, the daemon SHALL have a live PTY running the shell in the workspace folder and a pane node whose `screen` follows its output.
- REQ-24 WHEN the shell of a pane exits, the daemon SHALL remove the pane.
- REQ-25 WHEN a pane's shell prints a command-finished mark, the daemon SHALL raise `shell_done` with its exit code on that pane and every ancestor; WHEN the next command starts, it SHALL end it.
- REQ-26 WHEN a pane or its workspace is removed, the daemon SHALL end its shell.
- REQ-27 WHEN the daemon restarts, each saved pane SHALL get a new shell under the same id.
- REQ-28 WHILE no node a client has open changes, the daemon SHALL send that client nothing.
- REQ-29 WHEN an item is added to or removed from the end of a list in a node, the patch SHALL be one `add` or `remove` op for it, not the whole list.
- REQ-30 THE daemon SHALL be drivable with JSON alone through the `jwn` binary: start, commands, stop, restart, state restored (`crates/jwn/tests/e2e.rs`).
- REQ-31 THE Kotlin contract module SHALL parse every valid fixture, write back the identical JSON, validate it against the same schema, reject the invalid ones, and apply every `patch-cases` file as Rust does (`android/contract`, `ConformanceTest`).
- REQ-32 IF a fixture or the schema changes without the Kotlin types following, THEN the Kotlin suite SHALL fail (Gradle treats `contract/` as a test input).
- REQ-33 WHILE a workspace has attention its phone has not seen, its Home row SHALL show the mark of the most urgent one, with a count when there are several, and one line saying what it is.
- REQ-34 WHEN the person taps that row's go control or an item in Avisos, the phone SHALL open the node the attention names (the pane's tab, not only the workspace) and mark it seen on that phone.
- REQ-35 THE phone SHALL keep a local seen record, node id to revision; it SHALL treat an attention as seen when its `rev` is not above the recorded one, and SHALL drop the record of a node from another epoch.
- REQ-36 THE phone SHALL NOT offer approve or deny controls on Home or Avisos.
- REQ-37 THE pane screens (claude, dev, shell) SHALL stay as in `docs/specs/mobile-prototype.html`; only the key row changes (REQ-38).
- REQ-38 (withdrawn) The pane key row (`^␣ esc tab ↑ ↓ ← → ^C`) is removed for now; where those keys go is OPEN-5. The editor's stop button (`■`, `^C`) stays.
- REQ-39 THE Android client SHALL draw Home, Avisos and a workspace's panes from the shared fixtures, under the application id and label of `identity.toml`, beside any other jw app.
- REQ-40 THE phone's pane tabs and the terminal's pane rows SHALL be labelled with the pane's `title` (the program running in the shell, else the shell); a pane SHALL NOT have a role chosen at creation.
- REQ-41 WHEN a patch does not start at the revision a client holds, or its epoch differs, THEN the client SHALL drop the node and open it again for a snapshot.
- REQ-42 WHERE `<PREFIX>_TCP` names a loopback address, the daemon SHALL serve the same JSON lines there too; IF the address is not loopback, THEN it SHALL refuse to start.
- REQ-43 WHEN the Android client is started with `-e live host:port`, it SHALL draw that daemon's nodes, opening them on demand and retrying a lost connection, and SHALL write to the socket off the UI thread.
- REQ-44 THE terminal client SHALL be laid out as in `docs/prototypes/tui.html`: a 28-column sidebar of numbered workspaces (`●` with panes, `○` without, `↳` for a worktree) with the most urgent unseen mark at the right and the count when there are several, a header row with project, branch and path, each pane of the highlighted workspace as a rounded box (one fills the area, two side by side, from three the first takes the left half and the rest stack), the focused box with a blue border and its unseen attention in its top border, and a status bar with the mode (`MOVE`, `TERM`, `AVISOS`).
- REQ-45 WHILE the terminal client only highlights a workspace it SHALL preview it and mark nothing seen; WHEN Enter opens it, or `h`/`l` focuses a pane, it SHALL mark that workspace and pane seen; `a` SHALL open a box of what waits and Enter there SHALL go to the named pane; `n` SHALL ask the daemon for a new pane.
- REQ-46 WHEN a client sends `mark_seen {node, rev}`, the daemon SHALL end the attentions that node raised at or below `rev` and patch every client that has them; an attention raised later SHALL stay; an unknown node SHALL be `not_found`.
- REQ-47 WHEN the phone or the terminal shows the node an attention names, it SHALL send `mark_seen` for it, so the other client drops that attention too.
- REQ-48 WHILE a program other than the shell is in the foreground of a pane's terminal, the pane node SHALL carry it as `running` (its argv) with `title` its name (`nvim`, `claude`; an interpreter gives way to its script), and WHEN it ends `running` SHALL be absent, `last_program` SHALL hold it and `title` SHALL be the shell's name again.
- REQ-49 WHEN a workspace is opened on the phone before its document arrives, the phone SHALL select its first pane once it does.
- REQ-50 WHEN `new_pane` has a `program`, the daemon SHALL start the shell as `$SHELL -i -c '<program>; exec $SHELL -i'` with each word quoted for that shell, so the program receives its words as given and, when it ends, the pane is the shell it left; IF `program` is empty THEN it SHALL be `bad_request`.
- REQ-51 WHEN the program is an agent (`claude`) that names no conversation, the daemon SHALL start it with its own `--session-id <uuid>`.
- REQ-52 WHEN the daemon restarts, a pane whose agent was running with a known conversation that has a saved transcript SHALL start that agent again as `claude --resume <id>`; an agent that ended, or whose conversation has no transcript, SHALL NOT be started again, and the pane SHALL be a shell.
- REQ-53 THE terminal client SHALL take the mouse: a left click on a workspace row opens it, on a pane's box focuses it, on `avisos N` in the status bar opens the list, on an item of the list goes there, and outside the list closes it; the wheel over the sidebar or the list moves the highlight; a click anywhere else SHALL change nothing.
- REQ-54 WHEN a pane's shell has reported a prompt, the pane node SHALL carry `blocks` and that node SHALL list one block per command the shell ran, oldest first; a pane whose shell has not (an unsupported shell) SHALL have none and clients SHALL draw its screen.
- REQ-55 WHILE a command runs its block SHALL have `running` true and no `code` or `ms`; WHEN it ends it SHALL carry `code`, `ms` and `running` false.
- REQ-56 THE output of a block SHALL be rows of spans with colours and attributes already resolved by the daemon, and SHALL NOT contain the prompt, the typed command or the marks.
- REQ-57 THE daemon SHALL keep at most 100 blocks per pane and 500 rows per block (the start of a longer output is dropped and the block marked `truncated`), and at most 4 MiB of output in all by dropping the oldest blocks.
- REQ-58 WHILE a command uses the alternate screen, the node SHALL be in mode `full` and its block SHALL be `full` with no output kept; clients SHALL then draw the pane's screen.
- REQ-59 THE daemon SHALL give zsh, bash and fish a shell integration that reports, with OSC 133 (`A` prompt, `B` input, `C` output start, `D;<code>` end) and OSC 633 (`E;<base64 command>`, `P;Cwd=<base64 folder>`), what each command was, where it started and how it ended; any other shell SHALL run unchanged.
- REQ-60 A client's editor SHALL send a line with `write` (the text and `\r`) and stop a running program with `write` of `\u0003`; no new command is needed.
- REQ-61 WHILE a pane is open in the terminal client, every key SHALL go to its shell as the bytes a terminal sends (text, Enter, Backspace, Tab, Esc, the arrows, Home, End, Delete, PageUp, PageDown, and Ctrl and Alt combinations); `^␣` SHALL give the keyboard back to the client and Enter SHALL take it again.
- REQ-62 THE terminal client SHALL ask the focused pane to the size of its box with `resize`, once for each size.
- REQ-63 THE phone SHALL have an input line that sends its text and `\r` to the focused pane with `write`; it SHALL be disabled when the data are the fixtures.
- REQ-64 THE clients SHALL send what is typed without waiting for the daemon's reply.
- REQ-65 WHEN a shown pane has a `blocks` node in mode `blocks`, THE terminal client SHALL draw it inside the pane's box as blocks (folder and time, `❯` command, coloured output, a bar green, red or yellow by status) with the editor under them; WHEN the node is in mode `full`, or the pane has none, it SHALL draw the pane's screen.
- REQ-66 WHEN a block is clicked, THE terminal client SHALL select it and stop forwarding keys; WHILE one is selected, `c` SHALL copy its command and `o` its output (OSC 52), `r` SHALL write its command and `\r` to the pane, `j`/`k` SHALL move the selection, and Esc SHALL release it.
- REQ-67 WHILE the last block runs, THE editor SHALL show `■ ^C`, and a click on it SHALL write `\u0003` to the pane.
- REQ-68 THE wheel over a pane drawn as blocks SHALL scroll its blocks, the newest at the bottom when not scrolled.
- REQ-69 WHEN the focused pane has a `blocks` node in mode `blocks`, THE phone SHALL draw it as flat blocks, the newest at the bottom: folder and time (`corriendo N s`, `salió N · t`), the command in bold, the output with its colours, a red bar and tint for a failure, a yellow one while it runs; WHEN the mode is `full`, or the pane has none, it SHALL draw the pane's screen (with a notice in the first case).
- REQ-70 WHEN a block is tapped, THE phone SHALL pick it and show Comando, Salida and Repetir; Repetir SHALL `write` its command and `\r` to the pane, and tapping the block again SHALL let it go.
- REQ-71 THE phone's editor SHALL show chips (folder and `⎇ branch`, or `◌ program` while the last block runs); WHILE it runs, a `■` button SHALL `write` `\u0003`.
- REQ-18 IF a client's outbox is full, THEN the daemon SHALL disconnect that client and no other.

REQ-1 to REQ-7, REQ-12 and REQ-13 are enforced by N1 (`crates/contract/tests/conformance.rs`, `crates/identity`). REQ-8, 9, 10, 11, 15, 16, 17 are enforced by N2 (`crates/daemon/tests/session.rs`, `src/core.rs` tests); REQ-19 to REQ-22 are enforced by N3 (`crates/daemon/tests/workspaces.rs`, against real git); REQ-18 is implemented (bounded outbox) and untested; REQ-14 and REQ-23 to REQ-27 are enforced by N4 (`crates/daemon/tests/panes.rs`, real `/bin/sh` PTYs). REQ-69 to REQ-71 by N20 (`android/client-core` `BlocksTest` for the labels, colours and paths; the Compose screens are built and linted, and were checked by hand on the phone against a real zsh: tap, actions, `■` stopping a `sleep`; Comando and Salida copy were not checked). The phone's `corriendo N s` counts from the daemon's start time against the phone's clock, so a skewed clock shifts it (never below 0). REQ-65 to REQ-68 by N19 (`crates/jwn/src/tui.rs` and `tui/shell.rs` tests on the `snapshot-blocks.json` fixture; the drawing was not tried by eye in a real terminal yet). REQ-61 to REQ-64 by N18 (`crates/jwn/src/tui.rs` tests; typing from both clients, `^C` and the leader checked in the real terminal client and on the phone by `scripts/e2e-live.py`, 39 checks). REQ-54 to REQ-59 by N17 (`crates/daemon/tests/blocks.rs` against real zsh and bash, unit tests of the scanner and the block store; **the fish script is written but was not run: fish is not installed here**). REQ-48 and REQ-49 by N13 (`crates/daemon/tests/panes.rs` with real `sleep`, `src/procs.rs`, `android/client-core`; checked by hand with `claude`, `nvim` and `sleep` on the phone). REQ-33 to REQ-35 and REQ-39 to REQ-41 by N8 (`android/client-core` tests, `just android`; the Compose screens are built and linted, not UI-tested). REQ-31 and REQ-32 by N6 (`android/contract`, `just android`). REQ-28 to REQ-30 by N5 (`tests/session.rs`, `contract/src/patch.rs` tests, `crates/jwn/tests/e2e.rs`).

---
## Mobile UI (the simple version)
A pane is a zsh (the user's `$SHELL`); claude or nvim run inside it, so the tab shows what runs there and the screen is a terminal. The claude-style conversation view comes when the daemon detects an agent in a pane, not from a role. Design baseline and the thing to approve: `docs/prototypes/mobile.html` (REQ-33 to REQ-38). The mark glyphs are `!` permission, `?` question, `◆` plan, `❯` command done (`✗` on a non-zero exit), `✓` agent done, `…` screen, `⚑` merged, `✗` CI failed; the existing `✻` agent and `⚡` dev stay. Only `shell_done` has a producer in the daemon today.

## Rationale
- RAT-1 **Fixtures, not generated code, are the contract.** Generated Kotlin from a schema is poor and hides drift; two hand-written sets of types checked against the same files cannot drift without a red test. The round trip (parse then write equals the file) catches a field one side drops.
- RAT-2 **A frozen envelope with versioned documents** (the user's point) means no forced daemon stop when a client or a document changes. The old jw bumped `PROTOCOL` 25 times, each refusing the old peer. A new daemon binary still needs a restart to run (it owns the PTYs), but nothing forces it for a JSON change.
- RAT-3 **JSON lines**, not length-prefixed frames with a binary kind: readable with `nc -U`, no framing code. Raw terminal bytes are not needed while a pane's screen is a node of text rows; add a stream only when a UI proves it needs one.
- RAT-4 **Looking is shared** (the user's call, replacing the first proposal of a per-client seen): if the phone cleared an aviso and the terminal kept showing it, the person would clear it twice. `mark_seen` carries the revision looked at, so an attention raised after the look stays. The cost is that a person who looks on one device no longer sees it as new on another.
- RAT-5 **RFC 6902 subset** is known, small, and easy to apply in Kotlin; array edits go by index and are safe because a patch always applies on exactly `from`.
- RAT-6 **One daemon is one session** in v1: a second session is a second daemon and socket (`JWN_SOCKET`). The session node's id is stable across restarts; its `epoch` is not.
- RAT-7 **The shell is the pane's process, and a program is started by it, not typed at it**: `$SHELL -i -c '<program>; exec $SHELL -i'` has no keystrokes to race the prompt or echo, still reads the rc files (`-i`), keeps the pane alive when the program ends (`exec` turns the same process into a shell), and does not depend on the shell's prompt. Each word is single-quoted for the shell that reads it (fish escapes inside the quotes, the POSIX shells close and reopen them). The id passed to a new agent is explicit, so two agents in one folder never share a transcript, and a restart resumes with `--resume`, never a reused `--session-id`.

## Measurements (N5, `jwn bench`)
25 workspaces × 2 panes, real `/bin/sh` shells, release build, Unix socket, one machine (reproduce with `cargo run --release -p jwn -- bench`).

| What | Measured | The brain's model |
|---|---|---|
| level 0 snapshot (session, 25 summaries) | 3.9 KB | about 17 KB |
| level 1 (one workspace) / level 2 (one pane, 24×80) | 0.4 KB / 0.4 KB | |
| every node opened (1 + 25 + 50) | 36 KB | |
| patch: rename, or add a workspace | 0.2 KB / 0.3 KB | about 0.3 KB |
| patch: a command ends with an exit code (3 patches: pane, workspace, session) | 1.3 KB | |
| idle, 76 nodes open | 0 bytes | about 34 KB per 10 min |
| resume check with a current cache | 0.25 KB (request and empty patch) | |
| polling the session snapshot every 10 s for 10 min | 234 KB (resume checks: 15 KB) | about 3 MB |
| command to reply / visible on a second client (p50, p95) | 0.5 ms, 1.6 ms / 0.5 ms, 1.7 ms | |
| keystrokes to the output visible on a second client (p50, p95) | 28 ms, 31 ms (20 ms of it is the coalescing window) | |

Corrections to the model: the level 0 document is smaller than assumed (3.9 KB, because panes are not in it); pushing makes idle cost zero instead of about 34 KB, and a phone that only polls pays 15 KB per 10 minutes with resume checks. The first measurement showed a 4.1 KB patch for adding one workspace because a list that changed length was replaced whole; `diff` now appends and trims (REQ-29, 0.3 KB).

- RAT-8 **Spans, not bytes.** The old jw kept each block's raw bytes and every phone ran its own terminal emulator (a Kotlin module with golden tests to keep it equal to the daemon's). Resolving colours once in the daemon (it already runs vt100) means clients draw runs of text and the terminal client, the phone and a future web client cannot disagree. The cost is a larger document per block, bounded by REQ-57.
- RAT-9 **Standard marks.** OSC 133 is what other terminals' shell integrations speak; the command text and folder use OSC 633 (VS Code's `E` and `P`) because 133 carries neither. The old jw used a private OSC 7770 and only for zsh; one set of marks now covers three shells.
- RAT-10 **A separate node, a field on the pane.** Blocks can be large and change on every line of output; keeping them out of the pane node keeps the pane's screen patches small and lets a client that only draws the screen never open them. The pane says where they are (`blocks`) so a client does not guess.
- RAT-11 **The editor is the client's, the line is the shell's.** The shell keeps its history, completion and key bindings; a client sends the finished line. Mirroring the shell's own buffer both ways (what the old jw did with zle's `$BUFFER`) is left for later (OPEN-6).

## Risks
- RISK-1 Index-based patch paths break if a client applies a patch to a node at a revision other than `from`. Mitigation: a client that sees a gap drops the node and reopens it for a snapshot.
- RISK-2 Round-trip equality forbids unknown fields inside valid fixtures, so forward-compatibility is tested by one synthetic test (`unknown_fields_are_ignored`), not by fixtures.
- RISK-3 `Attention.rev` assumes `rev` is monotonic per node across restarts; a restart changes `epoch`, so caches and *seen* maps are keyed by `(epoch, id)` and discarded on change.
- RISK-5 Patches are diffed from whole rebuilt documents on every change: simple and always consistent with the snapshot, but O(documents) per change. Fine at 25 workspaces; N5 measures it.
- RISK-6 `jwn stop` ends the daemon with SIGTERM and does not remove the socket or pid file; the next start clears a stale socket, and `stop` only signals a pid while the socket answers.
- RISK-7 `restore` drops a pane whose shell cannot start (say its folder is gone) and saves that, so a transient failure loses the pane; its workspace stays.
- RISK-8 A pane's `cwd` is where its shell started, not where it is now (needs shell integration); `title` is the shell's name until OSC titles are read.
- RISK-9 The loopback TCP listener has no authentication: any local process controls every pane. Development only; the real phone link needs pairing and TLS first.
- RISK-10 The Android live source is a development link (fixed address, no pairing); it has no UI tests, and it was checked by hand on a phone against the terminal client.
- RISK-11 The program is the foreground process group of the terminal, looked at every 0.7 s with one `ps`; its argv is `ps` output split on spaces (quotes are lost), and a program shorter than the interval can be missed. Unix only.
- RISK-12 Only an agent with a known conversation comes back: one started through `new_pane` (we give it an id) or typed with `--resume <id>`/`--session-id <id>`. A plain `claude` typed at a prompt is shown as running but not resumed. Whether a conversation exists is a look for `<id>.jsonl` under Claude's `projects/` directory, which is Claude's layout, not a contract.
- RISK-17 A pane has one size, and the last client to ask sets it: the terminal client fits the pane to its box when it takes the keyboard, so the phone then draws a screen of that size. Per-client sizes need blocks (the phone draws spans, not the screen) or a screen per client.
- RISK-16 The fish integration (`shell/fish/jwn.fish`) has not been run: fish is not installed on the machine it was written on. The test skips when fish is missing and runs when it is there.
- RISK-13 (measured: three blocks, one of 300 lines, are a 9 KB snapshot, and a short command is patches of about 330 and 270 bytes; the cost for a 100-block pane is not measured yet.) The daemon rebuilds and diffs whole documents on each change (RISK-5); a `blocks` node with 100 blocks of 500 rows is large. A command printing continuously changes one block every 20 ms. Measure with the real node (N17) and, if it hurts, diff only the changed block.
- RISK-14 The integration adds hooks to the user's shell (zsh `ZDOTDIR` shim, bash `--rcfile` wrapper, fish `--init-command`) and the user's own rc files must still run. Prompt frameworks (starship, powerlevel10k) also use these hooks; the order may matter.
- RISK-15 A program that redraws in place (progress bars with cursor moves) is rendered by the daemon's vt100, so its rows are the final screen, not every intermediate frame.
- RISK-4 The Kotlin suite proves the types and the patch rules, not a live connection: no Kotlin client has talked to the daemon yet.

## Open questions
- OPEN-1 Retention: how many revisions per node the daemon keeps for resume (proposed 256).
- OPEN-3 The permission and question screens inside the claude pane still offer their options; whether they change too is the user's call (the prototype leaves them).
- OPEN-4 The terminal client's workspace list has no prototype yet; the phone comes first.
- OPEN-6 Mirroring the shell's line buffer into the clients' editors (what you type at a terminal shows in the phone's editor) and the other way round.
- OPEN-7 History and completion in the editors: from the shell, or kept by the client.
- OPEN-5 Where the special keys (`esc`, `tab`, arrows, `^C` beyond the stop button, `^D`, `^R`, …) go on the phone now that the key row is removed (the user: think about it later).
- OPEN-2 Whether a `pane` screen needs styled cells (colour, bold) before the first UI; v1 is text only.

## Corrections
- A client that resumed with the connection's epoch instead of its cached document's epoch would keep a dead daemon's data (found by test in N2); the client now uses the document's.
- The schema's `$id` was the relative `contract.schema.json`; the Rust validator accepted it and the Kotlin one refused it (not an absolute URI). It is now `https://schema.jwn.invalid/contract.schema.json`.
- Gradle called the Kotlin test task up to date when only a fixture changed, so drift would have passed silently; `contract/` is now declared a task input (found by mutating a fixture).
- Patch application had only Rust unit tests; it now has shared `fixtures/patch-cases/` that both languages apply (REQ-31).
- Running the live app on a phone found three bugs no test had: a socket write on the UI thread crashed it (`NetworkOnMainThreadException`; writes now go through their own thread); Compose's strong skipping meant a node arriving did not redraw the screen (the screens now read the source's tick themselves); and a daemon started inside a terminal window dies with the window (start it detached).
- Settle who clears an attention: the task proposed a per-client *seen* and the first build did that (`e2e-live.py` even tested it). The user then asked that clearing on the phone show on the terminal and the other way round, so it is shared through `mark_seen` (REQ-46, REQ-47).
- The QA bugs B-28, B-29 and B-30 (an exited agent looking alive, a restart failing with "Session ID … already in use", two Claudes sharing one transcript) are closed by design here: the pane is always a shell, a restart uses `--resume` with an id given at launch, and every agent has its own id (REQ-50 to REQ-52, tested with a stand-in `claude` that saves a transcript like the real one; the real one was checked to start with the injected id and be detected).
- The task note said a binary kind and a PROTOCOL integer were needed for the phone; they are not part of this contract.

## Tests
`cargo test -p contract` (`crates/contract/tests/conformance.rs`): fixtures round-trip and validate, invalid fixtures are rejected, every schema variant has a fixture, unknown fields are ignored, Rust enums match the schema. Mutating a fixture or a Rust field turns it red.

## Parts
| Part | What | REQs | Main files |
|---|---|---|---|
| N0 ✓ `5b73558` | skeleton, freeze rule, identity, `just ci` | 13 | `identity.toml`, `crates/identity` |
| N1 ✓ `9b1e7bd` | node and message spec, JSON Schema, fixtures, Rust conformance | 1–7, 12 | `contract/`, `crates/contract` |
| N2 ✓ `759e5d2` | daemon core: socket, hello/epoch, tree, persistence, patches | 8–11, 15–18 | `crates/{wire,daemon,jwn}` |
| N3 ✓ `6b0f336` | folders and git worktrees as workspaces | 19–22 | `crates/daemon/src/{git,workspaces}.rs` | |
| N4 ✓ `43eab72` | panes: PTY shell, screen node, input, resize, close, restore | 14, 23–27 | `crates/daemon/src/{pty,panes}.rs` |
| N5 ✓ `0921e63` | wire measurements (`jwn bench`) and the JSON-only end-to-end test | 28–30 | `crates/jwn/src/bench.rs`, `crates/jwn/tests/e2e.rs` |
| N6 | the Kotlin conformance suite: types, schema validation, patch cases | 31, 32 | `android/contract`, `contract/fixtures/patch-cases` |
| N7 ✓ `2570339` | the mobile UI spec: attention marks, go row, Avisos, per-phone seen, slim key row | 33–38 | `docs/prototypes/mobile.html` |
| N8 ✓ `76e1353` | the Android client on fixtures: `client-core` (seen, navigator, node cache) and the Compose app | 39–41 | `android/client-core`, `android/app` |
| N9 | the live link: loopback TCP listener, `adb reverse`, the Android live source | 42, 43 | `crates/daemon/src/server.rs`, `android/app/.../Data.kt` |
| N10 ✓ `88f6dd3` | the live link's first terminal client and a design pass on the phone (JetBrains Mono, aligned marks) | | `crates/jwn/src/tui.rs`, `android/app/.../Screens.kt` |
| N11 | the terminal client redone as the approved prototype: today's jw frame, panes as boxes, avisos box | 44, 45 | `crates/jwn/src/tui.rs` |
| N12 | looking is shared (`mark_seen`), the terminal reconnects, and `scripts/e2e-live.py` checks daemon, terminal and phone against each other | 46, 47 | `crates/daemon/src/commands.rs`, `scripts/e2e-live.py` |
| N13 | the program running in each pane is detected (`running`, `last_program`, `title`); the phone selects a first pane that arrives late | 48, 49 | `crates/daemon/src/procs.rs` |
| N14 | programs start inside the shell through a wrapper (`new_pane program`); an agent gets its own conversation id and is resumed after a restart | 50–52 | `crates/daemon/src/launch.rs`, `crates/daemon/tests/launch.rs` |
| N15 | the terminal client takes the mouse (clicks and the wheel), tested in the real client through a pty | 53 | `crates/jwn/src/tui.rs`, `scripts/e2e-live.py` |
| N16 ✓ `19e6f03` | the spec and the contract of the shell blocks: the `blocks` node, schema, fixtures, Rust and Kotlin types, conformance | 54–60 | `contract/`, `crates/contract`, `android/contract` |
| N17 ✓ (this commit) | the shell integration (zsh, bash, fish) and the blocks in the daemon | 54–59 | `crates/daemon/src/{blocks,integration}.rs`, `crates/daemon/shell/` |
| N18 ✓ | typing into panes: the terminal client forwards keys (leader `^␣`), fits the pane to its box, and the phone has an input line | 60–64 | `crates/jwn/src/tui.rs`, `android/app` |
| N19 ✓ | the blocks view in the terminal client | 65–68 | `crates/jwn/src/tui.rs`, `crates/jwn/src/tui/shell.rs` |
| N20 ✓ (this commit) | the blocks view on the phone | 69–71 | `android/app`, `android/client-core` |
| N21 | opening a pane with a choice (shell, claude, nvim) and resizing a pane to its client | | |
