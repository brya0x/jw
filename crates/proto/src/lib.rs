//! What jw's daemon and its clients say to each other over the unix socket
//! (docs/specs/rust-tui.md, Interfaces → Protocol), how it is framed, and
//! the client end of the socket.

pub mod client;
pub mod kitty;
pub mod proto;

// The code says `crate::layout`, as it did when this was one crate.
use jw_core::layout;
