use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use contract::patch::apply_patch;
use contract::{ClientMsg, Cmd, DaemonMsg, ErrorBody, Id, Outcome, Peer};
use serde_json::Value;

use crate::Conn;

#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    /// The daemon sent something that is not a contract message.
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "{e}"),
            ClientError::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        ClientError::Io(e)
    }
}

/// A client with a node cache. Snapshots, patches and `gone` update `nodes` as they
/// arrive, whichever call happens to be reading.
pub struct Client {
    conn: Conn,
    /// The daemon's name and version, as it said them in its `hello` (REQ-988).
    pub daemon: Peer,
    pub epoch: String,
    pub root: Id,
    pub accepted: BTreeMap<String, u32>,
    /// Node id -> its document as last seen.
    pub nodes: HashMap<Id, Value>,
    /// Bytes read from the daemon so far, newlines included.
    pub bytes_in: u64,
    next_cmd: u64,
}

impl Client {
    pub fn connect(path: &Path, name: &str) -> Result<Client, ClientError> {
        let mut conn = Conn::connect(path)?;
        let accepts = crate::default_accepts();
        conn.send(&ClientMsg::Hello {
            client: Peer {
                name: name.into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            accepts,
        })?;
        let line = conn
            .recv_line()?
            .ok_or_else(|| ClientError::Protocol("closed before hello".into()))?;
        let bytes_in = line.len() as u64 + 1;
        match serde_json::from_str(&line).map_err(|e| ClientError::Protocol(e.to_string()))? {
            DaemonMsg::Hello {
                daemon,
                epoch,
                root,
                accepted,
                ..
            } => Ok(Client {
                conn,
                daemon,
                epoch,
                root,
                accepted,
                nodes: HashMap::new(),
                bytes_in,
                next_cmd: 1,
            }),
            other => Err(ClientError::Protocol(format!(
                "expected hello, got {other:?}"
            ))),
        }
    }

    /// Open a node, resuming from the cached copy when there is one.
    pub fn open(&mut self, node: &str) -> Result<(), ClientError> {
        // Resume under the epoch the cached copy came from, not the current one.
        let cached = self
            .nodes
            .get(node)
            .and_then(|v| Some((v["epoch"].as_str()?.to_owned(), v["rev"].as_u64()?)));
        let (epoch, since) = match cached {
            Some((epoch, rev)) => (Some(epoch), Some(rev)),
            None => (None, None),
        };
        self.conn.send(&ClientMsg::Open {
            node: node.into(),
            epoch,
            since,
        })?;
        Ok(())
    }

    pub fn close(&mut self, node: &str) -> Result<(), ClientError> {
        self.conn.send(&ClientMsg::Close { node: node.into() })?;
        Ok(())
    }

    /// Send a command without waiting for its reply (typing must not wait on the daemon).
    /// The reply is read, and ignored, by whatever call reads next.
    pub fn send(&mut self, cmd: Cmd) -> Result<(), ClientError> {
        let id = format!("c{}", self.next_cmd);
        self.next_cmd += 1;
        self.conn.send(&ClientMsg::Cmd { id, cmd })?;
        Ok(())
    }

    /// Send a command and read until its reply arrives, applying updates on the way.
    pub fn cmd(&mut self, cmd: Cmd) -> Result<Result<Option<Value>, ErrorBody>, ClientError> {
        self.cmd_within(cmd, Duration::from_secs(30))
    }

    /// Like `cmd`, for one the daemon may take longer than 30 seconds to answer. `wait` is for the
    /// whole call: messages that are not the reply do not start it again, so a daemon that keeps
    /// patching and never answers (an older one that does not know the command) ends it too.
    pub fn cmd_within(
        &mut self,
        cmd: Cmd,
        wait: Duration,
    ) -> Result<Result<Option<Value>, ErrorBody>, ClientError> {
        let id = format!("c{}", self.next_cmd);
        self.next_cmd += 1;
        self.conn.send(&ClientMsg::Cmd {
            id: id.clone(),
            cmd,
        })?;
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = if left.is_zero() {
                None
            } else {
                self.poll(left)?
            };
            let Some(msg) = msg else {
                return Err(ClientError::Protocol("no reply".into()));
            };
            if let DaemonMsg::Reply { id: got, outcome } = msg
                && got == id
            {
                return Ok(match outcome {
                    Outcome::Ok { result, .. } => Ok(result),
                    Outcome::Err { error, .. } => Err(error),
                });
            }
        }
    }

    /// Read one message (waiting up to `timeout`), apply it to the cache and return it.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<DaemonMsg>, ClientError> {
        self.conn.set_read_timeout(Some(timeout))?;
        let line = match self.conn.recv_line() {
            Ok(Some(line)) => line,
            Ok(None) => return Err(ClientError::Protocol("daemon closed the connection".into())),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        self.bytes_in += line.len() as u64 + 1;
        let msg: DaemonMsg =
            serde_json::from_str(&line).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.apply(&msg)?;
        Ok(Some(msg))
    }

    /// Keep reading until `done` holds for the cache, or `timeout` passes.
    pub fn wait_until(
        &mut self,
        timeout: Duration,
        mut done: impl FnMut(&Client) -> bool,
    ) -> Result<bool, ClientError> {
        let deadline = Instant::now() + timeout;
        while !done(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            self.poll(left.min(Duration::from_millis(200)))?;
        }
        Ok(true)
    }

    fn apply(&mut self, msg: &DaemonMsg) -> Result<(), ClientError> {
        match msg {
            DaemonMsg::Snapshot { node } => {
                let doc =
                    serde_json::to_value(node).map_err(|e| ClientError::Protocol(e.to_string()))?;
                let id = contract::node_id(node).clone();
                self.nodes.insert(id, doc);
            }
            DaemonMsg::Patch {
                node,
                epoch,
                from,
                to,
                ops,
            } => {
                let usable = self.nodes.get(node).is_some_and(|d| {
                    d["epoch"] == epoch.as_str() && d["rev"].as_u64() == Some(*from)
                });
                let applied = usable && {
                    let doc = self.nodes.get_mut(node).expect("checked above");
                    apply_patch(doc, ops, *to).is_ok()
                };
                if !applied {
                    // A gap, or a cache from another epoch: drop it and take a snapshot.
                    self.nodes.remove(node);
                    self.conn.send(&ClientMsg::Open {
                        node: node.clone(),
                        epoch: None,
                        since: None,
                    })?;
                }
            }
            DaemonMsg::Gone { node } => {
                self.nodes.remove(node);
            }
            DaemonMsg::Hello { .. } | DaemonMsg::Reply { .. } | DaemonMsg::Error { .. } => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// A daemon that says hello, hears the command, and then keeps sending messages that are not
    /// its reply (what an older daemon that does not know a command does, with live nodes open).
    fn chatty_daemon(path: &Path) -> std::thread::JoinHandle<()> {
        let listener = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut out = stream;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let hello = DaemonMsg::Hello {
                daemon: Peer {
                    name: "test".into(),
                    version: "0".into(),
                },
                epoch: "e".into(),
                root: "root".into(),
                accepted: BTreeMap::new(),
            };
            writeln!(out, "{}", serde_json::to_string(&hello).unwrap()).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            let chatter =
                r#"{"t":"error","code":"bad_request","message":"not a contract message"}"#;
            while writeln!(out, "{chatter}").is_ok() {
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    }

    /// REQ-619: the wait of `cmd_within` is a deadline for the whole call, not for each message.
    #[test]
    fn a_command_that_is_never_answered_ends_at_its_deadline_whatever_else_the_daemon_sends() {
        let dir = std::env::temp_dir().join(format!("jwn-wire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let _ = std::fs::remove_file(&path);
        let daemon = chatty_daemon(&path);
        let mut c = Client::connect(&path, "test").unwrap();
        // REQ-988: the client keeps who said hello
        assert_eq!(
            (c.daemon.name.as_str(), c.daemon.version.as_str()),
            ("test", "0")
        );
        let started = Instant::now();
        let err = c
            .cmd_within(
                Cmd::ReadFile {
                    workspace: "w".into(),
                    path: "a.rs".into(),
                },
                Duration::from_millis(400),
            )
            .unwrap_err();
        let took = started.elapsed();
        assert!(
            matches!(&err, ClientError::Protocol(m) if m == "no reply"),
            "{err}"
        );
        assert!(
            took >= Duration::from_millis(400) && took < Duration::from_millis(1500),
            "{took:?}"
        );
        drop(c);
        daemon.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
