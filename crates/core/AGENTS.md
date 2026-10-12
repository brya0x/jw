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
| `src/program.rs` | which foreground programs are agents: `is_agent_program`, `is_agent_pane` (addendum 17) |
| `src/stream.rs` | what opening a workspace starts: `Stream` (`:19`; `resolve_in` for a session that is not the process's, `open_plan` the panes with their sizes, shared by the CLI and the daemon's phone writes), its panes, env, `Stamp` (whether a resolved one is still fresh), the agent's command with `--session-id` and hooks (`hooks`, `:323`), nvim's `--listen` (`listen`, `:292`) |
| `src/actions.rs` | worktrees: `project_named`, `valid_branch`, `new_stream` (`:85`; `NewOptions.session` registers it in a session), `rename` (`:198`), `rm_plan`/`rm` (`:333`/`:361`), `sync` (`:436`), `done_plan` (`:490`) |
| `src/connectors/git.rs`, `github.rs` | git through the `git` binary, plus `head_of` (`:298`), which reads `.git` without running anything; PRs through `gh` |
| `src/ago.rs` | an age in words ("2 min ago") for lists of devices |
| `src/sidebar.rs` | a session's workspaces grouped as the sidebar shows them (`rows`), and the items and session counts the phone is told (`items`, `sessions`) |
| `src/qr.rs` | a QR code as a grid of modules with a quiet zone, and its half-block cells (`cells`); the CLI and the TUI each draw them |
| `src/diff.rs` | a worktree's changes in one `Scope` (General, Uncommitted, a commit: addendum 19) and its `commits`, for the diff viewer and, within the phone's limits (`report`, `commit_report`, `Pick`), for the bridge's `diff` and `commits` answers. Every git call goes through `git_bounded`/`git_capped` (`Bounds`: output, deadline, file size; clean environment, no textconv or fsmonitor, filter drivers off, its own process group, cancelled with the phone); an untracked file is read only if it is a regular file, never through a symlink (`read_beneath`). Past `DIFF_CAP` a view lists its files only (`Loaded::TooLarge`) |
| `src/fsview.rs`, `src/md.rs` | the PC's files for a phone (mobile REQ-77 to 81): `fsview::list` and `read` (any path the daemon's user can read, absolute or `~`, no `..`; at most 2000 entries, 1 MiB, 5 s on a thread of its own that is let go of, a link followed when named, a FIFO or device never read), and `md::blocks`, Markdown as blocks with the parser the TUI uses; `phone_*.json` in `android/proto/src/test/resources` are held to their serialisation by a test |
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
