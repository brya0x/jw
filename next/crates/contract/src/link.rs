//! The messages between a phone and the bridge, before the daemon's protocol starts.

use serde::{Deserialize, Serialize};

/// What a phone sends first: spend a pairing token, or prove a paired device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PhoneMsg {
    Pair {
        token: String,
        name: String,
        /// Digits the phone shows and the person at the PC compares.
        code: String,
    },
    Auth {
        device: String,
        secret: String,
    },
}

/// Why the bridge refused a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Denied {
    BadToken,
    Declined,
    Timeout,
    BadSecret,
    Revoked,
    LockedOut,
}

/// What the bridge sends a phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum BridgeMsg {
    Paired {
        device: String,
        /// Shown once: the bridge keeps only its hash.
        secret: String,
    },
    Denied {
        reason: Denied,
    },
    /// The daemon's protocol starts: the stream now carries `ClientMsg` and `DaemonMsg`.
    Ready,
    /// Where the PC can be reached; sent after `ready` and whenever it changes.
    Endpoints {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lan: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tunnel: Option<String>,
    },
}
