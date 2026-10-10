//! What the daemon has yet to write to one client (REQ-76). A client that
//! reads slower than its panes print must not grow the daemon's memory nor
//! hold up the panes: once the bytes waiting pass `HIGH`, that client's
//! output for the pane is dropped, and when it has caught up (under `LOW`)
//! the writer asks for a fresh Snapshot of the pane instead.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

use crate::proto::{DaemonMsg, PaneId, raw_len};

pub const HIGH: usize = 8 << 20;
pub const LOW: usize = 1 << 20;

/// The sending end, cloned into every subscription of the client.
#[derive(Clone)]
pub struct Tx(Arc<Shared>);

struct Shared {
    q: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Default)]
struct Queue {
    msgs: VecDeque<DaemonMsg>,
    /// Bytes of terminal output waiting.
    bytes: usize,
    closed: bool,
    /// Panes whose output was dropped; each needs a Snapshot.
    stale: BTreeSet<PaneId>,
}

/// The client went away.
#[derive(Debug)]
pub struct Closed;

impl Tx {
    pub fn new() -> Self {
        Tx(Arc::new(Shared {
            q: Mutex::new(Queue::default()),
            ready: Condvar::new(),
        }))
    }

    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.0.q.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn send(&self, msg: DaemonMsg) -> Result<(), Closed> {
        let mut q = self.queue();
        if q.closed {
            return Err(Closed);
        }
        if let DaemonMsg::Output { pane, bytes } = &msg {
            let pane = *pane;
            if q.stale.contains(&pane) {
                return Ok(());
            }
            if q.bytes + bytes.len() > HIGH {
                q.stale.insert(pane);
                let before = q.msgs.len();
                q.msgs
                    .retain(|m| !matches!(m, DaemonMsg::Output { pane: p, .. } if *p == pane));
                if q.msgs.len() != before {
                    q.bytes = q.msgs.iter().map(raw_len).sum();
                }
                return Ok(());
            }
        }
        q.bytes += raw_len(&msg);
        q.msgs.push_back(msg);
        self.0.ready.notify_one();
        Ok(())
    }

    /// Queues the Snapshot that brings a stale pane back. Called with the
    /// pane's state locked, so its next output comes after it.
    pub fn resync(&self, pane: PaneId, snapshot: DaemonMsg) {
        let mut q = self.queue();
        if q.closed || !q.stale.remove(&pane) {
            return;
        }
        q.bytes += raw_len(&snapshot);
        q.msgs.push_back(snapshot);
        self.0.ready.notify_one();
    }

    /// Stops waiting to resync a pane that no longer exists.
    pub fn forget(&self, pane: PaneId) {
        self.queue().stale.remove(&pane);
    }

    /// The next message, waiting for one; `None` once closed.
    pub fn recv(&self) -> Option<DaemonMsg> {
        let mut q = self.queue();
        loop {
            if q.closed {
                return None;
            }
            if let Some(m) = q.msgs.pop_front() {
                q.bytes -= raw_len(&m);
                return Some(m);
            }
            q = self.0.ready.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The panes to resync, once the client has caught up.
    pub fn stale(&self) -> Vec<PaneId> {
        let q = self.queue();
        if q.bytes >= LOW {
            return Vec::new();
        }
        q.stale.iter().copied().collect()
    }

    pub fn close(&self) {
        self.queue().closed = true;
        self.0.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(pane: PaneId, n: usize) -> DaemonMsg {
        DaemonMsg::Output {
            pane,
            bytes: vec![b'x'; n],
        }
    }

    #[test]
    fn a_slow_client_drops_output_then_resyncs() {
        let tx = Tx::new();
        tx.send(out(1, HIGH - 10)).unwrap();
        tx.send(DaemonMsg::Prompted { pane: 1 }).unwrap();
        // Past HIGH: pane 1's output goes, the other message stays.
        tx.send(out(1, 100)).unwrap();
        tx.send(out(1, 5)).unwrap();
        assert_eq!(tx.queue().bytes, 0);
        assert_eq!(tx.queue().msgs.len(), 1);
        assert_eq!(tx.stale(), [1]);

        assert!(matches!(tx.recv(), Some(DaemonMsg::Prompted { .. })));
        let snap = DaemonMsg::Snapshot {
            pane: 1,
            role: "shell".into(),
            cols: 80,
            rows: 24,
            bytes: b"screen".to_vec(),
        };
        tx.resync(1, snap.clone());
        assert!(tx.stale().is_empty());
        tx.send(out(1, 3)).unwrap();
        assert_eq!(tx.recv(), Some(snap));
        assert_eq!(tx.recv(), Some(out(1, 3)));
    }

    #[test]
    fn closed_refuses() {
        let tx = Tx::new();
        tx.close();
        assert!(tx.send(out(1, 1)).is_err());
        assert_eq!(tx.recv(), None);
    }
}
