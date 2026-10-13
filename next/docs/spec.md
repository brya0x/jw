---
status: built (N0–N5); UIs and the Android conformance suite are next
scope: [next/contract, next/crates/contract, next/crates/daemon]
depends_on: []
supersedes: []
---
# next: a daemon that serves a tree of JSON nodes, for any client

## Contract
One daemon per session owns every terminal. Clients (TUI, phone, CLI) connect over one stream and exchange JSON objects, one per line (`t`-tagged). The state is a tree of **nodes** opened on demand: `session` (level 0: workspaces with a summary) → `workspace` (level 1: its panes) → `pane` (level 2: its screen). Every node has `id` (uuid), `v` (its document version), `epoch` (changes at every daemon start), `rev` (counts changes), and `attention`.

**The envelope never changes** (`hello`, `open`/`close`, `snapshot`/`patch`/`gone`, `cmd`/`reply`, `error`). Evolution happens in node documents: additive fields never bump `v` and receivers ignore unknown fields; a breaking change adds `v+1` of that one node type, served beside the old. At `hello` the client lists the versions it reads per node type (`accepts`) and the daemon answers the version it will serve (`accepted`); a type without a common version is not served. There is no global protocol number.

A client opens a node and gets a `snapshot`, then `patch`es (RFC 6902 subset `add`/`remove`/`replace`, `from`→`to` rev) until it closes it. A patch never edits `rev`: the client applies the ops to the cached document it holds at `from`, then sets `rev` to `to`; any other state means reopen for a snapshot. Opening with `epoch`+`since` resumes from a cache: the daemon replies one `patch` (empty when nothing changed) when the epoch matches and the revision is still retained, else a `snapshot`. The epoch sent is the one in the cached document, never the connection's. Clients cache nodes by `(epoch, id)`.

**Attention** is a typed item the daemon computes once and a client never re-derives: `question`, `plan`, `permission`, `shell_done` (with `exit_code`), `agent_done`, `screen`, `merged`, `ci_failed`. Each names the node that opens when tapped and the `rev` that raised it. A node's `attention` lists its whole subtree, so it bubbles to the session. Each client keeps its own *seen* (node → rev); the daemon never learns who saw what. An attention lasts as long as its cause (a `shell_done` ends when the next command starts), so the daemon drops it then; whether a person looked at it is the client's *seen*. There is no approve/deny anywhere.

A **pane is a terminal**: its process is always the user's `$SHELL`; a program runs inside it (`running`, `last_program` are reserved for that and absent in v1).

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

Commands (`cmd`): `rename_session`, `open_folder`, `create_worktree`, `remove_workspace`, `new_pane`, `close_pane`, `write`, `resize`. Error codes: `bad_request`, `not_found`, `unsupported`, `conflict`, `git_failed`, `io`.

Command semantics (N3): `open_folder` takes an absolute path of an existing directory and opens it once (`conflict` if open; `not_found`, `bad_request` for a missing path, a file or a relative path). `create_worktree` makes `<repo parent>/<repo name>-wt/<branch with / → ->` with `git worktree add` (a new branch from `base`, or an existing branch checked out, never both) and opens it as `<repo>: <branch>`; names that are options or not branch names are `bad_request`, git's own message is `git_failed`. `remove_workspace` closes a workspace; with `remove_worktree` it first runs `git worktree remove` without force, so a worktree with changes stays and so does its workspace. A successful open replies `result: {workspace, path}`.

Panes (N4): `new_pane` starts the user's `$SHELL` (else `/bin/sh`) in the workspace folder on a PTY (24×80 until `resize`, at most 500 each) with `TERM=xterm-256color` and `JWN_PANE_ID` set (`identity::env_var`), and replies `result: {pane}`. The pane node's `screen` is the vt100 screen as text rows. A shell that exits takes its pane with it, like closing a terminal; `close_pane` and `remove_workspace` end the shells. After a daemon restart the saved panes get a fresh shell with the same ids (the screen and scrollback are not kept). A shell that prints OSC 133 marks (`ESC ] 133 ; D ; <code> BEL` finished, `ESC ] 133 ; C BEL` command started) raises `shell_done` with the exit code, which the next command start ends; shell integration scripts that print them for zsh, bash and fish come later.

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
- REQ-18 IF a client's outbox is full, THEN the daemon SHALL disconnect that client and no other.

REQ-1 to REQ-7, REQ-12 and REQ-13 are enforced by N1 (`crates/contract/tests/conformance.rs`, `crates/identity`). REQ-8, 9, 10, 11, 15, 16, 17 are enforced by N2 (`crates/daemon/tests/session.rs`, `src/core.rs` tests); REQ-19 to REQ-22 are enforced by N3 (`crates/daemon/tests/workspaces.rs`, against real git); REQ-18 is implemented (bounded outbox) and untested; REQ-14 and REQ-23 to REQ-27 are enforced by N4 (`crates/daemon/tests/panes.rs`, real `/bin/sh` PTYs). REQ-28 to REQ-30 by N5 (`tests/session.rs`, `contract/src/patch.rs` tests, `crates/jwn/tests/e2e.rs`).

---
## Rationale
- RAT-1 **Fixtures, not generated code, are the contract.** Generated Kotlin from a schema is poor and hides drift; two hand-written sets of types checked against the same files cannot drift without a red test. The round trip (parse then write equals the file) catches a field one side drops.
- RAT-2 **A frozen envelope with versioned documents** (the user's point) means no forced daemon stop when a client or a document changes. The old jw bumped `PROTOCOL` 25 times, each refusing the old peer. A new daemon binary still needs a restart to run (it owns the PTYs), but nothing forces it for a JSON change.
- RAT-3 **JSON lines**, not length-prefixed frames with a binary kind: readable with `nc -U`, no framing code. Raw terminal bytes are not needed while a pane's screen is a node of text rows; add a stream only when a UI proves it needs one.
- RAT-4 **Seen is per client** because the same attention is read on a phone and on a laptop at different times; a server-side clear would hide it from one of them. `Attention.rev` makes "seen" a comparison, not a message.
- RAT-5 **RFC 6902 subset** is known, small, and easy to apply in Kotlin; array edits go by index and are safe because a patch always applies on exactly `from`.
- RAT-6 **One daemon is one session** in v1: a second session is a second daemon and socket (`JWN_SOCKET`). The session node's id is stable across restarts; its `epoch` is not.
- RAT-7 **The shell is the pane's process** (the program is launched as `$SHELL -i -c '<program>; exec $SHELL -i'`) so no keystrokes race the prompt, the exit status is exact and the pane survives the program. Detail lands with the first feature that launches a program.

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

## Risks
- RISK-1 Index-based patch paths break if a client applies a patch to a node at a revision other than `from`. Mitigation: a client that sees a gap drops the node and reopens it for a snapshot.
- RISK-2 Round-trip equality forbids unknown fields inside valid fixtures, so forward-compatibility is tested by one synthetic test (`unknown_fields_are_ignored`), not by fixtures.
- RISK-3 `Attention.rev` assumes `rev` is monotonic per node across restarts; a restart changes `epoch`, so caches and *seen* maps are keyed by `(epoch, id)` and discarded on change.
- RISK-5 Patches are diffed from whole rebuilt documents on every change: simple and always consistent with the snapshot, but O(documents) per change. Fine at 25 workspaces; N5 measures it.
- RISK-6 `jwn stop` ends the daemon with SIGTERM and does not remove the socket or pid file; the next start clears a stale socket, and `stop` only signals a pid while the socket answers.
- RISK-7 `restore` drops a pane whose shell cannot start (say its folder is gone) and saves that, so a transient failure loses the pane; its workspace stays.
- RISK-8 A pane's `cwd` is where its shell started, not where it is now (needs shell integration); `title` is the shell's name until OSC titles are read.
- RISK-4 Nothing yet proves the Kotlin side parses the fixtures; REQ-12 only makes it possible. The Android phase adds that suite.

## Open questions
- OPEN-1 Retention: how many revisions per node the daemon keeps for resume (proposed 256).
- OPEN-2 Whether a `pane` screen needs styled cells (colour, bold) before the first UI; v1 is text only.

## Corrections
- A client that resumed with the connection's epoch instead of its cached document's epoch would keep a dead daemon's data (found by test in N2); the client now uses the document's.
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
| N5 ✓ (this commit) | wire measurements (`jwn bench`) and the JSON-only end-to-end test | 28–30 | `crates/jwn/src/bench.rs`, `crates/jwn/tests/e2e.rs` |
