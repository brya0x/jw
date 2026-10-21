# jw — a terminal for working with coding agents in parallel

This repository is the jw that was written from scratch under `next/` (jwn) and took the place of
the old jw (0.0.26 and before) in N157. The old code and its specs are in the git history before the
commit that removed them; they are read for what they did and why, never pasted.

## Rules
- **Identity lives in `identity.toml` only.** Names, dirs, socket, env prefix, applicationId
  come from the `identity` crate. Never spell `jwn`, `JWN_` or a path by hand.
- **The JSON documents are the contract** (`contract/schema`, `contract/fixtures`). The
  envelope never changes; node documents are versioned per type (`v`). See `docs/spec.md`.
- **A pane is a terminal first**: always the user's `$SHELL`; a program runs inside it.
- **Spec before code** (`docs/spec.md`: Contract, Interfaces, REQ-n, RISK-n, parts table).
  UI parts need an approved interactive HTML prototype before spec or code.
- **`just ci` green before every commit.** One local commit per part.
- **Never push or open a PR** unless asked. **Never `pkill jw`/`jwn`**: tests use their own
  socket dir and kill only the pid they recorded.
- **CI** is `.github/workflows/ci.yml`: it runs the recipes of the `justfile` on pull requests and on pushes to `main`.
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
| `android/client-core` | what the phone knows: the seen record, the node cache, Home rows, the navigator, and the changes screen's model and file tree (pure Kotlin, tested on the JVM) |
| `android/app` | the Compose app (`dev.jw.next`, from `identity.toml`); draws the shared fixtures for now |
| `crates/jwn/src/tui/shell.rs` | the shell as blocks in the terminal client: colours, block lines, OSC 52 |
| `crates/jwn/src/tui/{changes,diff_lines}.rs` | the diff viewer of the terminal client: its state, keys and drawing, and the pure part (a `diff` node as lines, the tree of files, words that changed, syntax tones) |
| `crates/jwn/src/tui.rs` | the terminal client (`jwn tui`): a pure `View` tested on the fixtures, and the loop that moves bytes |
| `crates/daemon/src/agents.rs`, `launch.rs`; `crates/jwn/src/hook.rs` | an agent's hooks: the settings file claude starts with, `jwn hook <event>` (what claude runs) and the attention the daemon makes of `agent_event` |
| `crates/jwn/src/agent.rs`, `crates/jwn/skill/SKILL.md` | the commands an agent in a pane uses (`ls`, `pane`, `read`, `prompt`, `worktree`, `skill`) as clients of the daemon, and the skill they install |
| `crates/daemon/src/project.rs`, `start.rs` | a repository's `.jw.toml` (where worktrees go, the branch's template, the setup, the layout and the agent) read afresh, and `project_config` and `start_workspace`, which start a worktree's panes |
| `crates/daemon/src/conversation.rs`, `engine.rs`, `crates/jwn/src/claude.rs`, `crates/daemon/shell/*` | a pane that runs `claude` as a conversation: the state a `conversation` node shows and the reducer that turns what the engine (Claude Code run headless) says into it, the engine's process and its threads and the commands that reach it, `claude` typed in a pane as the binary's own (the shell integration's function and `jwn claude`, with its plain-terminal version); what the engine says is in `docs/claude-engine.md` |
| `crates/daemon/src/github.rs` | what GitHub says of a worktree's branch (`gh`): the pull request, merged, failing checks, as attention |
| `crates/daemon/src/markdown.rs` | a Markdown file of a workspace, or the plan that waits in a conversation, read and parsed (`pulldown-cmark`) into the blocks of a `MarkdownDoc` that the clients draw (`read_markdown`, `read_plan`) |
| `crates/daemon/src/diff.rs`, `changes.rs` | the changes of a workspace: git read into `diff` and `commits` nodes (three scopes, the 2 s poll of `uncommitted`), and the commands that open, refresh and mark them |
| `crates/daemon/src/blocks.rs`, `integration.rs`, `shell/` | the shell blocks: the scanner for OSC 133/633 marks, the per-pane block store (vt100 into styled spans), and the zsh, bash and fish scripts that report prompts and commands |
