# jw — a terminal for working with coding agents in parallel

This repository is the jw that was written from scratch as `jwn` under `next/` and took the place of
the old jw (0.0.26 and before) in N157. It took the name `jw` in N158 (the swap). The old code and its specs are in the git history before the
commit that removed them; they are read for what they did and why, never pasted.

## Rules
- **Identity lives in `identity.toml` only** (and in `identity.test.toml`, its variant for the
  test install, N190). Names, dirs, socket, env prefix, applicationId
  come from the `identity` crate. Never spell `jw`, `JW_` or a path by hand
  (the exceptions are listed in `identity.toml`, each with a test).
- **`just jwtest` builds a test install** (`jwtest`: its own daemon, socket, state, `JWTEST_*`
  and skills) that runs beside the production `jw` and never writes it. Try a branch with it.
- **The JSON documents are the contract** (`contract/schema`, `contract/fixtures`). The
  envelope never changes; node documents are versioned per type (`v`). See `docs/spec.md`.
- **A pane is a terminal first**: always the user's `$SHELL`; a program runs inside it.
- **Spec before code** (`docs/spec.md`: Contract, Interfaces, REQ-n, RISK-n, parts table).
  UI parts need an approved interactive HTML prototype before spec or code.
- **`just ci` green before every commit.** One local commit per part.
- **Never push or open a PR** unless asked. **Never `pkill jw`**: tests use their own
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
| `crates/jw` | the binary: `daemon`, `stop`, `watch`, `cmd`, `bench`; the JSON end-to-end test |

Never reach a real daemon from a test: use a private socket dir (`JW_SOCKET`, `JW_STATE_DIR`) and stop it with `jw stop` (it signals only the pid recorded in that dir, and only while the socket answers).
| `android/contract` | the Kotlin types of the JSON documents and `ConformanceTest`, which reads the same `contract/` files as the Rust suite |
| `android/client-core` | what the phone knows: the seen record, the node cache, Home rows, the navigator, and the changes screen's model and file tree (pure Kotlin, tested on the JVM) |
| `android/app` | the Compose app (`dev.jw.next`, from `identity.toml`); draws the shared fixtures for now |
| `crates/jw/src/tui/sections.rs` | the `/` list and the `/skills` list in sections (REQ-795 to REQ-803): the section of a command, the window of eight lines with its titles, and the jump of `RePág`/`AvPág`; pure, drawn by `tui/conversation.rs` |
| `crates/jw/src/update.rs`, `version.rs` | `jw update` and `jw android install --debug` (the repository of `identity.toml` fetched into a temporary directory, built, installed, deleted; signals and the guard of the deletion), and `jw --version` with the warning of a daemon of another version (N182) |
| `crates/jw/src/agent/{roles,skillfile,plugins,skillset}.rs`, `agent/skillset/{catalog,lock,github}.rs`, `crates/jw/roles/*.md`, `crates/jw/skill/ROLES.md` | the skillset of N184 and N184b: one catalog of skills, roles and plugins (embedded, `github`, `plugin`) that `jw skillset ls/on/off` lists, installs and removes, with the lock of what came from GitHub, the roles' layers (project, user, built-in) and `jw role ls/show`, the mark that lets `skill install` and `jw update` keep a skill the person edited, `claude plugin` for the plugins, the built-in roles (`brain`, `researcher`) and the `/agent` skill |
| `crates/jw/src/tui/shell.rs` | the shell as blocks in the terminal client: colours, block lines, OSC 52 |
| `crates/jw/src/tui/{changes,diff_lines}.rs` | the diff viewer of the terminal client: its state, keys and drawing, and the pure part (a `diff` node as lines, the tree of files, words that changed, syntax tones) |
| `crates/jw/src/tui.rs` | the terminal client (`jw tui`): a pure `View` tested on the fixtures, and the loop that moves bytes |
| `crates/daemon/src/agents.rs`, `launch.rs`; `crates/jw/src/hook.rs` | an agent's hooks: the settings file claude starts with, `jw hook <event>` (what claude runs) and the attention the daemon makes of `agent_event` |
| `crates/jw/src/agent.rs`, `crates/jw/skill/SKILL.md` | the commands an agent in a pane uses (`ls`, `pane`, `read`, `prompt`, `worktree`, `skill`) as clients of the daemon, and the skill they install |
| `crates/daemon/src/project.rs`, `start.rs` | a repository's `.jw.toml` (where worktrees go, the branch's template, the setup, the layout and the agent) read afresh, and `project_config` and `start_workspace`, which start a worktree's panes |
| `crates/daemon/src/conversation.rs`, `engine.rs`, `crates/jw/src/claude.rs`, `crates/daemon/shell/*` | a pane that runs `claude` as a conversation: the state a `conversation` node shows and the reducer that turns what the engine (Claude Code run headless) says into it, the engine's process and its threads and the commands that reach it, `claude` typed in a pane as the binary's own (the shell integration's function and `jw claude`, with its plain-terminal version); what the engine says is in `docs/claude-engine.md` |
| `crates/daemon/src/claude_update.rs` | keeping `claude` up to date: at the daemon's start and every few hours `claude --version`, `claude update`, `claude --version`, the `update_claude` job and the version the conversations compare their engine with (`claude_update` and `claude_update_hours` of `config.toml`, read afresh; `$<PREFIX>_CLAUDE_UPDATE=0` turns it off) |
| `crates/daemon/src/github.rs` | what GitHub says of a worktree's branch (`gh`): the pull request, merged, failing checks, as attention |
| `crates/daemon/src/markdown.rs` | a Markdown file of a workspace, or the plan that waits in a conversation, read and parsed (`pulldown-cmark`) into the blocks of a `MarkdownDoc` that the clients draw (`read_markdown`, `read_plan`) |
| `crates/daemon/src/diff.rs`, `changes.rs` | the changes of a workspace: git read into `diff` and `commits` nodes (three scopes, the 2 s poll of `uncommitted`), and the commands that open, refresh and mark them |
| `crates/daemon/src/blocks.rs`, `integration.rs`, `shell/` | the shell blocks: the scanner for OSC 133/633 marks, the per-pane block store (vt100 into styled spans), and the zsh, bash and fish scripts that report prompts and commands |
