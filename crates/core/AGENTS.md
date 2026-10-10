# jw-core

jw's model, with no terminal, no daemon and no UI. Everything else builds on it, and it
depends on nothing of jw's.

## What lives where

| Module | What |
|---|---|
| `src/core/config.rs` | the project TOML: `load` looks in `.jw.toml`, then `~/.config/jw/<project>.toml`, then defaults; `save_layout` rewrites only `[layout]`'s tree |
| `src/core/registry.rs` | `workspaces.json`, every worktree: `Entry` (`:15`), `Registry` (`:63`), `default_path` (`:71`, seeds from Go's `registry.json` once), `state_dir` (`:89`) |
| `src/layout.rs` | `Node`, the layout as written in TOML (`:14`), and `Tree<L>`, the live split tree with every operation the daemon applies (`:155`) |
| `src/session.rs` | named sessions: `current` (`:26`), `create` (`:206`), `rename` (`:66`), `list` (`:189`); their files live in `sessions/<name>/` |
| `src/folders.rs` | a session's folders: `load` (`:115`), `edit` (`:149`), `entry_for` (`:172`) |
| `src/stream.rs` | what opening a workspace starts: `Stream` (`:19`), its panes, env, the agent's command with `--session-id` and hooks (`hooks`, `:323`), nvim's `--listen` (`listen`, `:292`) |
| `src/actions.rs` | worktrees: `new_stream` (`:85`), `rename` (`:198`), `rm_plan`/`rm` (`:333`/`:361`), `sync` (`:436`), `done_plan` (`:490`) |
| `src/connectors/git.rs`, `github.rs` | git through the `git` binary, plus `head_of` (`:298`), which reads `.git` without running anything; PRs through `gh` |
| `src/diff.rs` | a worktree's changes against its base, for the diff viewer |
| `src/init.rs` | drafting a project config (`detect`), and `write_project`, the `.jw.toml` of the setup step |

## Keep in mind

- **Files in `~/.local/state/jw` are shared by the TUI, the CLI and the daemon.** Write them
  with a temp file and a rename (see `Registry::save`).
- **A session's `main` is stored as the empty string in `Entry.session`.** Use
  `session::owns` and `session::stored`, never compare names by hand.
- **Folder workspace ids carry their session outside `main`** (`folders::id_in`). Build them
  with that function.
- **`testdata/*.go.*` are files the Go jw wrote.** Configs and registries in the wild still
  look like this, so they stay readable.

## Tests

`cargo test -p jw-core`. Git tests make temp repos with `connectors::git::tests::new_test_repo`
and `must_git`. Nothing here may need a daemon or a terminal.
