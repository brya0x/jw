# next/ — the new jw, written from scratch

## CODE FREEZE on the old jw
Everything outside `next/` (the `crates/`, `android/`, root manifests, old specs) is
**frozen: only security and blocking fixes.** New work goes in `next/`.
Nothing from jw 0.0.26 files is copied into `next/`; the old code and specs are read for
what they do and why, never pasted.

## Rules
- **Identity lives in `identity.toml` only.** Names, dirs, socket, env prefix, applicationId
  come from the `identity` crate. Never spell `jwn`, `JWN_` or a path by hand.
- **The JSON documents are the contract** (`contract/schema`, `contract/fixtures`). The
  envelope never changes; node documents are versioned per type (`v`). See `docs/spec.md`.
- **A pane is a terminal first**: always the user's `$SHELL`; a program runs inside it.
- **Spec before code** (`docs/spec.md`: Contract, Interfaces, REQ-n, RISK-n, parts table).
  UI parts need an approved interactive HTML prototype before spec or code.
- **`just ci` (in `next/`) green before every commit.** One local commit per part.
- **Never push or open a PR** unless asked. **Never `pkill jw`/`jwn`**: tests use their own
  socket dir and kill only the pid they recorded.
- Never bind a key with Alt (AeroSpace owns it).

## Map
| Crate | Owns |
|---|---|
| `crates/identity` | the product identity and the paths/env names derived from it |
| `crates/contract` | the Rust types of the JSON documents; `tests/conformance.rs` checks them against `contract/` |
| `crates/wire` | the transport (JSON lines on a Unix socket) and `Client`, a client with a node cache |
| `crates/web` | the phone link: pairing, the paired devices, and the WebSocket bridge to the daemon socket; it never talks to the daemon before a phone is authenticated |
| `crates/daemon` | the session, the node tree, patches, git worktrees, PTY panes |
| `crates/jwn` | the binary: `daemon`, `stop`, `watch`, `cmd`, `bench`; the JSON end-to-end test |

Never reach a real daemon from a test: use a private socket dir (`JWN_SOCKET`, `JWN_STATE_DIR`) and stop it with `jwn stop` (it signals only the pid recorded in that dir, and only while the socket answers).
| `android/contract` | the Kotlin types of the JSON documents and `ConformanceTest`, which reads the same `contract/` files as the Rust suite |
| `android/client-core` | what the phone knows: the seen record, the node cache, Home rows, the navigator (pure Kotlin, tested on the JVM) |
| `android/app` | the Compose app (`dev.jw.next`, from `identity.toml`); draws the shared fixtures for now |
| `crates/jwn/src/tui/shell.rs` | the shell as blocks in the terminal client: colours, block lines, OSC 52 |
| `crates/jwn/src/tui.rs` | the terminal client (`jwn tui`): a pure `View` tested on the fixtures, and the loop that moves bytes |
| `crates/daemon/src/diff.rs`, `changes.rs` | the changes of a workspace: git read into `diff` and `commits` nodes (three scopes, the 2 s poll of `uncommitted`), and the commands that open, refresh and mark them |
| `crates/daemon/src/blocks.rs`, `integration.rs`, `shell/` | the shell blocks: the scanner for OSC 133/633 marks, the per-pane block store (vt100 into styled spans), and the zsh, bash and fish scripts that report prompts and commands |
