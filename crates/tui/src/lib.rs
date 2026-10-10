//! jw's TUI client (docs/specs/rust-tui.md, Screen): the sidebar, the
//! panes it draws from the daemon's screens, the pickers, the diff and
//! Markdown viewers, the settings screen and the themes.

pub mod settings;
pub mod theme;
pub mod tui;
pub mod view;

// The code says `crate::proto`, `crate::stream`…, as it did when this was
// one crate.
use jw_core::{actions, connectors, core, diff, folders, layout, session, stream};
use jw_proto::{client, kitty, proto};
