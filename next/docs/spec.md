---
status: draft
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
- REQ-18 IF a client's outbox is full, THEN the daemon SHALL disconnect that client and no other.

REQ-1 to REQ-7, REQ-12 and REQ-13 are enforced by N1 (`crates/contract/tests/conformance.rs`, `crates/identity`). REQ-8, 9, 10, 11, 15, 16, 17 are enforced by N2 (`crates/daemon/tests/session.rs`, `src/core.rs` tests); REQ-18 is implemented (bounded outbox) and untested; REQ-14 lands with N4.

---
## Rationale
- RAT-1 **Fixtures, not generated code, are the contract.** Generated Kotlin from a schema is poor and hides drift; two hand-written sets of types checked against the same files cannot drift without a red test. The round trip (parse then write equals the file) catches a field one side drops.
- RAT-2 **A frozen envelope with versioned documents** (the user's point) means no forced daemon stop when a client or a document changes. The old jw bumped `PROTOCOL` 25 times, each refusing the old peer. A new daemon binary still needs a restart to run (it owns the PTYs), but nothing forces it for a JSON change.
- RAT-3 **JSON lines**, not length-prefixed frames with a binary kind: readable with `nc -U`, no framing code. Raw terminal bytes are not needed while a pane's screen is a node of text rows; add a stream only when a UI proves it needs one.
- RAT-4 **Seen is per client** because the same attention is read on a phone and on a laptop at different times; a server-side clear would hide it from one of them. `Attention.rev` makes "seen" a comparison, not a message.
- RAT-5 **RFC 6902 subset** is known, small, and easy to apply in Kotlin; array edits go by index and are safe because a patch always applies on exactly `from`.
- RAT-6 **One daemon is one session** in v1: a second session is a second daemon and socket (`JWN_SOCKET`). The session node's id is stable across restarts; its `epoch` is not.
- RAT-7 **The shell is the pane's process** (the program is launched as `$SHELL -i -c '<program>; exec $SHELL -i'`) so no keystrokes race the prompt, the exit status is exact and the pane survives the program. Detail lands with the first feature that launches a program.

## Risks
- RISK-1 Index-based patch paths break if a client applies a patch to a node at a revision other than `from`. Mitigation: a client that sees a gap drops the node and reopens it for a snapshot.
- RISK-2 Round-trip equality forbids unknown fields inside valid fixtures, so forward-compatibility is tested by one synthetic test (`unknown_fields_are_ignored`), not by fixtures.
- RISK-3 `Attention.rev` assumes `rev` is monotonic per node across restarts; a restart changes `epoch`, so caches and *seen* maps are keyed by `(epoch, id)` and discarded on change.
- RISK-5 Patches are diffed from whole rebuilt documents on every change: simple and always consistent with the snapshot, but O(documents) per change. Fine at 25 workspaces; N5 measures it.
- RISK-6 `jwn stop` ends the daemon with SIGTERM and does not remove the socket or pid file; the next start clears a stale socket, and `stop` only signals a pid while the socket answers.
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
| N2 | daemon core: socket, hello/epoch, tree, persistence, patches | 8–11, 15–18 | `crates/{wire,daemon,jwn}` |
| N3 | folders and git worktrees as workspaces | | |
| N4 | panes: PTY shell, screen node, input, resize, close, restore | 14 | |
| N5 | wire measurements and the JSON-only end-to-end script | | |
