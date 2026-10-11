# jw-tui

The TUI client: it draws the daemon's screens, the sidebar, header and status bar, the
pickers, the diff and Markdown viewers, and the settings screen. It never depends on the
daemon; it only talks to it through jw-proto.

## What lives where

| File | What |
|---|---|
| `src/tui/mod.rs` | `run` (`:211`) and `App`: `on_key` (`:747`), `leader_key` (`:812`), `on_daemon` (`:672`), `reload` (`:432`, the sidebar's rows), `open_entry` (`:922`), `start_panes` (`:965`), `set_tree` (`:994`), `marks` (`:540`), sessions (`ask_session` `:1636`, `switch_session` `:1731`), the header's git (`find_here` `:866`) |
| `src/tui/trace.rs` | `JW_TRACE=<file>`: timed marks and spans for measuring a switch (`scripts/bench-switch.py` reads them); one load and a branch when off |
| `src/tui/draw.rs` | `draw` (`:15`): `sidebar`, `header`, `status`, `banner` (copied), `which` (the key popup) |
| `src/tui/finder.rs` | the pickers: folders, workspaces, files, sessions |
| `src/tui/modal.rs` | confirmations and the name inputs; `^␣ p`'s save-layout modal |
| `src/tui/setup.rs` | the setup step of a project's first worktree: the layout builder, agent and setup, written to `.jw.toml` (addendum 10) |
| `src/tui/settings_view.rs` | `^␣ ,` |
| `src/tui/mobile.rs` | mobile (spec: mobile.md): `App::mobile` keeps the daemon's latest `MobileState` and the pair questions waiting; the Mobile modal (`^␣ m`) and the question "pair this phone?" are drawn from it; `status_right` is the bar's indicator. `MobileWatch` is sent right after `Hello` |
| `src/tui/diffview.rs`, `mdview.rs` | the viewers, drawn as panes with no process (`view:` roles) |
| `src/tui/select.rs` | selecting text in a pane (addendum 12): the selection as lines of the history, the text it covers, words and lines, the OSC 52 copy |
| `src/tui/keys.rs` | the leader, and encoding keys and mouse events for the panes' programs |
| `src/settings.rs` | `settings.json`: `get` (`:203`), `save` (`:187`), `ACTIONS` (`:48`, the rebindable keys) |
| `src/theme.rs` | palettes: `p()` for every colour, `load` (`:197`) for the themes the settings name, `look` for the phone (`ready.palettes`). Every theme is JSON: the built-ins are `themes/*.json` (`BUILTIN`), checked against `themes/schema.json` by a test |
| `src/view/` | syntax highlighting and Markdown rendering |

## Keep in mind

- **The look and the keys follow the approved prototype** (`docs/specs/rust-tui.md`, Screen).
  A UI change starts with an interactive HTML prototype the user approves.
- **Never bind Alt.** Keys after the leader come from `settings::ACTIONS`; add new ones there
  so they can be rebound and show in the popup.
- **Every colour comes from `theme::p()`.** Never hard-code one. The one exception is the
  mobile QR, black on white in any theme because a scanner needs the contrast.
- **A pair question never takes keys from something being typed.** It opens only over
  nothing or the Mobile modal, swallows keys for half a second, and only `y` pairs.
- **Nothing slow on the UI thread.** git, gh and file walks go through `App::background`,
  except reading `.git/HEAD` (`head_of`).

## Tests

- **Unit tests:** `cargo test -p jw-tui`.
- **The screen:** `cargo build --workspace --examples`, then for example
  `scripts/drive.py /tmp/jq1 lead:o 'type:~' type:demo key:enter wait:1.5 dump`.
  - Use a fresh, short dir.
  - Look at the dump, not just the exit code.
  - Kill that dir's daemon afterwards (`kill $(cat /tmp/jq1/run/jw/jw.pid)`).
