//! The bridge a phone reaches the daemon through: WebSocket in, daemon lines out, with
//! pairing and device secrets in front (REQ-72 to REQ-79). It knows the daemon only as a
//! Unix socket to connect to, after a phone has proved itself.

pub mod auth;
pub mod bridge;
pub mod tls;
pub mod tunnel;
