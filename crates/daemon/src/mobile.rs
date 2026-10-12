//! Mobile, hosted by the daemon (docs/specs/mobile.md, REQ-33 to 40): the
//! bridge a phone connects to, the Cloudflare tunnel in front of it, and the
//! question "pair this phone?" put to whoever watches.
//!
//! It starts only on `MobileStart` and never by itself (REQ-33). Everything
//! here has locks and threads of its own: no pane, workspace or session lock
//! is ever held, and none is held while waiting for the network, the tunnel
//! or a person (REQ-40). `Core` is only ever held for a few instructions.
//!
//! Every start has an epoch. Stopping, or failing, bumps it, and a start
//! or ticker thread of an older epoch does nothing more: a stop during
//! `starting` cannot be undone by the start it interrupted.

use std::collections::HashMap;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use jw_core::core::config;
use jw_core::core::registry;
use jw_core::sidebar;
use jw_web::auth::Store;
use jw_web::bridge::{Bridge, Config, Endpoints, Lister, Look, Query, Timings};
use jw_web::tls;
use jw_web::tunnel::{self, Step, Tunnel};

use crate::outbox::Tx;
use crate::proto::{DaemonMsg, MobileDevice, MobileState};

/// What a test runs at a named point of the service.
pub type Hook = Arc<dyn Fn(&str) + Send + Sync>;

/// Where the wifi listener goes (REQ-56). Tests never let it detect: it would
/// bind the address of whatever network the developer is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lan {
    /// The private address of the interface the default route uses.
    Detect,
    /// No wifi listener: the tunnel only.
    Off,
    /// This address (tests use loopback).
    At(Ipv4Addr),
    /// The address written in this file each time it is asked, `off` or
    /// nothing meaning none: a test changes the PC's address by writing it.
    File(PathBuf),
}

impl Lan {
    /// `JW_LAN_ADDR`: unset is `Detect`, `off` is `Off`, an IPv4 address is
    /// `At`. A seam for the integration tests, which run the real binary; no
    /// user setting is meant to set it.
    pub fn from_env() -> Self {
        match std::env::var("JW_LAN_ADDR").ok().as_deref().map(str::trim) {
            None | Some("") => Lan::Detect,
            Some("off") => Lan::Off,
            Some(path) if path.starts_with("file:") => Lan::File(path["file:".len()..].into()),
            Some(addr) => addr.parse().map_or(Lan::Off, Lan::At),
        }
    }

    fn addr(&self) -> Option<Ipv4Addr> {
        match self {
            Lan::Detect => tls::lan_ip(),
            Lan::Off => None,
            Lan::At(ip) => Some(*ip),
            Lan::File(path) => std::fs::read_to_string(path).ok()?.trim().parse().ok(),
        }
    }
}

/// The waits, which tests shorten.
#[derive(Clone)]
pub struct Tuning {
    /// How long the person at the PC has to answer "pair this phone?"
    /// (REQ-36).
    pub ask_within: Duration,
    /// How often the state is looked at again while mobile is on.
    pub tick: Duration,
    pub waits: tunnel::Waits,
    /// Another `mobile.json` than the one in jw's config dir (tests).
    pub store: Option<PathBuf>,
    /// Called with `start` as the start thread begins and with `stop` as a
    /// stop's slow part begins (tests: to stall or to panic there).
    pub hook: Option<Hook>,
    /// Where request ids come from, as hex (tests: one that fails). `None` is
    /// the system's random bytes.
    pub random: Option<fn() -> std::io::Result<String>>,
    /// Where the wifi listener goes.
    pub lan: Lan,
    /// How often the private address is read again while mobile is on
    /// (REQ-69).
    pub lan_every: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            ask_within: Duration::from_secs(60),
            tick: Duration::from_secs(1),
            waits: tunnel::Waits::default(),
            store: None,
            hook: None,
            random: None,
            lan: Lan::from_env(),
            // The address is read again this often (REQ-69); a test that moves
            // it by a file does not wait half a minute.
            lan_every: if matches!(Lan::from_env(), Lan::File(_)) {
                Duration::from_millis(200)
            } else {
                Duration::from_secs(30)
            },
        }
    }
}

impl Tuning {
    fn hook(&self, stage: &str) {
        if let Some(hook) = &self.hook {
            hook(stage);
        }
    }
}

/// What a phone's `workspaces` and `sessions` are answered from: the registry
/// and the folders of the sessions on disk, as the TUI's sidebar groups them
/// (REQ-12). Read at each ask, so what the TUI changed is what the phone sees.
fn lister() -> Lister {
    // What the error says is for a phone, not for a log: no path of the PC's
    // in it. The cause is the PC's to find.
    Arc::new(|query, cancel| {
        let state = registry::state_dir().map_err(|_| "jw has no state folder here")?;
        let reg = registry::default_path().map_err(|_| "jw has no state folder here")?;
        let value = match query {
            Query::Workspaces(name) => {
                if !sidebar::known(name) || !sidebar::names(&state).contains(name) {
                    return Err(format!("no session named {name}"));
                }
                let rows = sidebar::load(&state, &reg, name)
                    .map_err(|_| format!("the workspaces of {name} could not be read"))?;
                let mut items = sidebar::items(&rows, name);
                let truncated = items.len() > sidebar::MAX_ITEMS;
                items.truncate(sidebar::MAX_ITEMS);
                serde_json::json!({ "items": items, "truncated": truncated })
            }
            Query::Sessions => {
                let items = sidebar::sessions(&state, &reg)
                    .map_err(|_| "the sessions could not be read".to_string())?;
                serde_json::json!({ "items": items })
            }
            Query::Diff(id) => {
                let entry = sidebar::find(&state, &reg, id)
                    .map_err(|_| "the workspaces could not be read".to_string())?
                    .ok_or_else(|| format!("no workspace {id}"))?;
                // git's and jw's own messages name directories of the PC; the
                // phone is told the workspace's name instead.
                let report = jw_core::diff::report(&entry, cancel)
                    .map_err(|e| format!("{e:#}").replace(&entry.path, &entry.name))?;
                serde_json::to_value(report)
                    .map_err(|_| "the diff could not be made".to_string())?
            }
        };
        Ok(value)
    })
}

/// Who answered a pair question (a client id) and what; `CANCELLED` when
/// nobody did because mobile ended.
type Answer = (u64, bool);
const CANCELLED: Answer = (u64::MAX, false);

/// The wifi listener while there is one.
#[derive(Clone)]
struct LanInfo {
    host: String,
    port: u16,
    fp: String,
}

/// The tunnel's own phase, apart from mobile's (REQ-67): with the wifi up it
/// can fail and mobile is still ready.
#[derive(Clone)]
enum Tun {
    Off,
    Starting(String),
    Ready,
    Failed(String),
}

enum Phase {
    Off,
    /// How far it got: `launching`, `started`, `registered` or `resolving`.
    Starting(String),
    Ready,
    Failed(String),
}

struct Core {
    epoch: u64,
    phase: Phase,
    read_only: bool,
    bridge: Option<Arc<Bridge>>,
    tunnel: Option<Tunnel>,
    host: String,
    token: String,
    lan: Option<LanInfo>,
    tun: Tun,
}

struct Inner {
    /// The daemon's own socket: the bridge reaches it as any client does.
    socket: PathBuf,
    look: Look,
    tuning: Tuning,
    core: Mutex<Core>,
    /// Clients that sent `MobileWatch`, until they leave.
    watchers: Mutex<Vec<(u64, Tx)>>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Answer>>>,
    /// The paired phones as the store last said, so a state does not read the
    /// file while a lock is held.
    devices: Mutex<Devices>,
    /// What was sent last, with the countdown left out, so nothing is sent
    /// twice.
    last: Mutex<Option<MobileState>>,
}

/// The store's devices and the stamp of the file they were read from.
#[derive(Default)]
struct Devices {
    stamp: Option<(std::time::SystemTime, u64)>,
    loaded: bool,
    list: Vec<jw_web::auth::Device>,
}

/// The service. Clones share it.
#[derive(Clone)]
pub struct Mobile(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Mobile {
    pub fn new(socket: PathBuf, look: Look, tuning: Tuning) -> Self {
        Self(Arc::new(Inner {
            socket,
            look,
            tuning,
            core: Mutex::new(Core {
                epoch: 0,
                phase: Phase::Off,
                read_only: false,
                bridge: None,
                tunnel: None,
                host: String::new(),
                token: String::new(),
                lan: None,
                tun: Tun::Off,
            }),
            watchers: Mutex::default(),
            pending: Mutex::default(),
            devices: Mutex::default(),
            last: Mutex::default(),
        }))
    }

    /// Makes `client` a watcher and tells it where mobile stands (REQ-35).
    pub fn watch(&self, client: u64, tx: &Tx) {
        self.0.refresh_devices();
        // Under `last`, so a push cannot reach this client before the state
        // it is told first, nor fall between the two.
        let _push = lock(&self.0.last);
        {
            let mut w = lock(&self.0.watchers);
            w.retain(|(c, _)| *c != client);
            w.push((client, tx.clone()));
        }
        let _ = tx.send(DaemonMsg::MobileState(self.0.snapshot()));
    }

    /// A client left.
    pub fn forget(&self, client: u64) {
        let nobody = {
            let mut w = lock(&self.0.watchers);
            w.retain(|(c, _)| *c != client);
            w.is_empty()
        };
        // A question with nobody left to answer it is a no (REQ-36), not a
        // phone waiting out the minute.
        if nobody {
            self.0.cancel_pending();
        }
    }

    /// Starts mobile unless it is starting or ready, and then changes
    /// nothing (REQ-34). Whoever asked is answered with the state either way.
    pub fn start(&self, read_only: bool, from: &Tx) {
        let epoch = {
            let mut core = lock(&self.0.core);
            if matches!(core.phase, Phase::Starting(_) | Phase::Ready) {
                None
            } else {
                core.epoch += 1;
                core.phase = Phase::Starting("launching".into());
                core.read_only = read_only;
                Some(core.epoch)
            }
        };
        let Some(epoch) = epoch else {
            let _ = from.send(DaemonMsg::MobileState(self.0.snapshot()));
            return;
        };
        let inner = Arc::clone(&self.0);
        thread::spawn(move || {
            // A panic must not leave `starting` for good: it is a failure
            // with a reason, and the next start works.
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                inner.tuning.hook("start");
                inner.bring_up(epoch, read_only)
            }));
            match run {
                Ok(Ok(())) => {}
                Ok(Err(why)) => inner.fail(epoch, why),
                Err(_) => inner.fail(
                    epoch,
                    "mobile stopped working unexpectedly; start it again".into(),
                ),
            }
        });
        self.0.push();
    }

    /// Stops mobile (REQ-9). `cloudflared` is killed and every question
    /// answered no before anything that could stall; closing the phones'
    /// sockets and joining the threads happen on another thread, which is
    /// waited for up to `wait` and then left behind, so a stalled phone
    /// cannot keep a signal from ending the daemon in time.
    pub fn stop(&self, wait: Duration) {
        let (bridge, tunnel) = {
            let mut core = lock(&self.0.core);
            core.epoch += 1;
            core.phase = Phase::Off;
            core.host.clear();
            core.token.clear();
            core.lan = None;
            core.tun = Tun::Off;
            (core.bridge.take(), core.tunnel.take())
        };
        if let Some(tunnel) = &tunnel {
            tunnel.kill();
        }
        self.0.cancel_pending();
        self.0.push();
        let (done, finished) = mpsc::channel();
        let inner = Arc::clone(&self.0);
        thread::spawn(move || {
            inner.tuning.hook("stop");
            inner.wind_down(&bridge, &tunnel);
            drop(bridge);
            drop(tunnel);
            let _ = done.send(());
        });
        let _ = finished.recv_timeout(wait);
    }

    /// A new pairing code, which ends the one before (REQ-38).
    pub fn code(&self) -> Result<()> {
        let (bridge, epoch) = {
            let core = lock(&self.0.core);
            match (&core.bridge, &core.phase) {
                (Some(bridge), Phase::Ready) => (Arc::clone(bridge), core.epoch),
                _ => bail!("mobile is not ready"),
            }
        };
        // Outside `core`: the bridge takes locks of its own.
        let token = bridge.new_pairing()?;
        {
            let mut core = lock(&self.0.core);
            if core.epoch == epoch {
                core.token = token;
            }
        }
        self.0.push();
        Ok(())
    }

    /// Applies at once to the phones connected and to new ones (REQ-37).
    pub fn read_only(&self, on: bool) {
        let bridge = {
            let mut core = lock(&self.0.core);
            core.read_only = on;
            core.bridge.clone()
        };
        if let Some(bridge) = bridge {
            bridge.set_read_only(on);
        }
        self.0.push();
    }

    /// The first answer to a pair question wins (REQ-36); later ones find
    /// nothing pending.
    pub fn answer(&self, client: u64, request: u64, yes: bool) {
        // Only a client that watches can have been shown the question.
        if !lock(&self.0.watchers).iter().any(|(c, _)| *c == client) {
            return;
        }
        if let Some(answer) = lock(&self.0.pending).remove(&request) {
            let _ = answer.send((client, yes));
        }
    }

    /// Revokes by id or name (REQ-39); the bridge cuts the phone within its
    /// check interval (REQ-6). Works with mobile off.
    pub fn revoke(&self, device: &str) -> Result<()> {
        let Some(store) = self.0.store() else {
            bail!("no config directory");
        };
        if store.revoke(device)?.is_none() {
            bail!("no device {device:?}");
        }
        self.0.refresh_devices();
        self.0.push();
        Ok(())
    }

    #[cfg(test)]
    fn snapshot(&self) -> MobileState {
        self.0.snapshot()
    }
}

impl Inner {
    fn store(&self) -> Option<Store> {
        let path = match &self.tuning.store {
            Some(path) => path.clone(),
            None => config::dir().ok()?.join("mobile.json"),
        };
        Some(Store::open(path))
    }

    fn store_path(&self) -> Option<PathBuf> {
        match &self.tuning.store {
            Some(path) => Some(path.clone()),
            None => Some(config::dir().ok()?.join("mobile.json")),
        }
    }

    /// Reads the store again when its file changed (a phone paired or came
    /// back, a device was revoked from a terminal) or was never read. A stat
    /// of one file, and a read only when the stamp moved; never called with
    /// `last` or `core` held.
    fn refresh_devices(&self) {
        let Some(path) = self.store_path() else {
            return;
        };
        let stamp = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
        {
            let cache = lock(&self.devices);
            if cache.loaded && cache.stamp == stamp {
                return;
            }
        }
        let list = self
            .store()
            .and_then(|s| s.devices().ok())
            .unwrap_or_default();
        let mut cache = lock(&self.devices);
        *cache = Devices {
            stamp,
            loaded: true,
            list,
        };
    }

    fn current(&self) -> u64 {
        lock(&self.core).epoch
    }

    /// Where mobile stands now. The pairing token is in `payload` (RISK-16).
    fn snapshot(&self) -> MobileState {
        let (phase, step, error, host, token, read_only, bridge, lan, tun) = {
            let core = lock(&self.core);
            let (phase, mut step, mut error) = match &core.phase {
                Phase::Off => ("off", String::new(), String::new()),
                Phase::Starting(s) => ("starting", s.clone(), String::new()),
                Phase::Ready => ("ready", String::new(), String::new()),
                Phase::Failed(e) => ("failed", String::new(), e.clone()),
            };
            // With the wifi up, the tunnel's own steps and failure are told
            // here, while the phase stays `ready` (REQ-67).
            let tun = match &core.tun {
                Tun::Off => "off",
                Tun::Starting(s) => {
                    if phase == "ready" {
                        step = s.clone();
                    }
                    "starting"
                }
                Tun::Ready => "ready",
                Tun::Failed(e) => {
                    if phase == "ready" {
                        error = e.clone();
                    }
                    "failed"
                }
            };
            (
                phase,
                step,
                error,
                core.host.clone(),
                core.token.clone(),
                core.read_only,
                core.bridge.clone(),
                core.lan.clone(),
                tun,
            )
        };
        // Asked of the bridge with `core` let go.
        let (connected, left) = bridge.as_ref().map_or((Vec::new(), None), |b| {
            (b.connected_routes(), b.pairing_left())
        });
        let devices = lock(&self.devices)
            .list
            .clone()
            .into_iter()
            .map(|d| {
                let route = connected
                    .iter()
                    .find(|(id, _)| *id == d.id)
                    .map(|(_, r)| r.as_str().to_string());
                MobileDevice {
                    connected: route.is_some(),
                    route: route.unwrap_or_default(),
                    id: d.id,
                    name: d.name,
                    paired: d.created,
                    last_seen: d.last_seen,
                }
            })
            .collect();
        let live = phase == "ready" && left.is_some();
        // The wifi payload when there is a wifi listener (REQ-58): it does not
        // wait for the tunnel and does not change when the tunnel comes up.
        let payload = match (&lan, live) {
            (Some(l), true) => format!("jw://{}:{}/{token}?fp={}", l.host, l.port, l.fp),
            (None, true) => format!("jw://{host}/{token}"),
            _ => String::new(),
        };
        MobileState {
            phase: phase.into(),
            step,
            payload,
            host,
            expires_in: left.map_or(0, |d| d.as_secs_f64().ceil() as u64),
            read_only,
            devices,
            error,
            custom_tunnel: tunnel::custom_in_use(),
            lan: lan.map_or_else(String::new, |l| format!("{}:{}", l.host, l.port)),
            tunnel: tun.into(),
        }
    }

    /// Where else a phone can reach the PC, as the bridge tells it (REQ-61).
    fn endpoints(&self) -> Endpoints {
        let core = lock(&self.core);
        Endpoints {
            lan: core
                .lan
                .as_ref()
                .map(|l| (l.host.clone(), l.port, l.fp.clone())),
            tunnel: matches!(core.tun, Tun::Ready).then(|| core.host.clone()),
        }
    }

    /// Hands the bridge what `endpoints` says now, with `core` let go: the
    /// bridge takes locks of its own.
    fn sync_endpoints(&self) {
        // Two statements: the first guard must be gone before `endpoints` locks.
        let bridge = lock(&self.core).bridge.clone();
        let endpoints = self.endpoints();
        if let Some(bridge) = bridge {
            bridge.set_endpoints(endpoints);
        }
    }

    /// Sends the state to every watcher when it differs from the last one
    /// sent. The seconds left count down on their own, so only whether there
    /// are any is compared.
    ///
    /// `last` is held from the snapshot to the broadcast, so two pushes
    /// cannot cross: a snapshot taken before a stop must not be sent after
    /// the stop's. Lock order: `last`, then `core` (let go again), then
    /// `watchers`; nothing takes `last` while holding either of the others.
    fn push(&self) {
        // The file is looked at before `last` is taken, never under it.
        self.refresh_devices();
        let mut last = lock(&self.last);
        let state = self.snapshot();
        let key = MobileState {
            expires_in: u64::from(state.expires_in > 0),
            ..state.clone()
        };
        if last.as_ref() == Some(&key) {
            return;
        }
        *last = Some(key);
        self.broadcast(&DaemonMsg::MobileState(state));
    }

    fn broadcast(&self, msg: &DaemonMsg) {
        lock(&self.watchers).retain(|(_, tx)| tx.send(msg.clone()).is_ok());
    }

    /// Kills the tunnel, says no to every question and closes every phone's
    /// socket. Callers that must not stall do this on a thread of their own:
    /// closing a socket is not promised never to block.
    fn wind_down(&self, bridge: &Option<Arc<Bridge>>, tunnel: &Option<Tunnel>) {
        if let Some(tunnel) = tunnel {
            tunnel.kill();
        }
        self.cancel_pending();
        if let Some(bridge) = bridge {
            bridge.close();
        }
    }

    /// Says no to every question waiting.
    fn cancel_pending(&self) {
        let pending: Vec<_> = lock(&self.pending).drain().collect();
        for (_, answer) in pending {
            let _ = answer.send(CANCELLED);
        }
    }

    /// Mobile could not come up, or lost its tunnel: `failed`, with the
    /// reason, unless a stop or a newer start got there first.
    fn fail(self: &Arc<Self>, epoch: u64, why: String) {
        let (bridge, tunnel) = {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return;
            }
            core.epoch += 1;
            core.phase = Phase::Failed(why);
            core.host.clear();
            core.token.clear();
            core.lan = None;
            core.tun = Tun::Off;
            (core.bridge.take(), core.tunnel.take())
        };
        if let Some(tunnel) = &tunnel {
            tunnel.kill();
        }
        self.cancel_pending();
        self.push();
        let inner = Arc::clone(self);
        thread::spawn(move || {
            inner.wind_down(&bridge, &tunnel);
            drop(bridge);
            drop(tunnel);
        });
    }

    /// A step of the tunnel coming up. With no wifi listener it is also
    /// mobile's own step (`starting`); with one, mobile is already `ready` and
    /// only the tunnel's row moves.
    fn step(&self, epoch: u64, text: &str) {
        {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return;
            }
            core.tun = Tun::Starting(text.into());
            if core.lan.is_none() {
                core.phase = Phase::Starting(text.into());
            }
        }
        self.push();
    }

    /// Where jw keeps `mobile.json`, and so the certificate: the test override
    /// of the store's path moves both, so a test never writes in the user's
    /// own config.
    fn config_dir(&self) -> Option<PathBuf> {
        match &self.tuning.store {
            Some(path) => path.parent().map(PathBuf::from),
            None => config::dir().ok(),
        }
    }

    /// Opens the wifi listener on `ip` (REQ-56): the port it had last time
    /// when that is free, so an address a phone stored stays good, else any.
    /// `None` when something is missing (no certificate, no port): mobile then
    /// goes on through the tunnel alone.
    fn open_wifi(&self, bridge: &Bridge, ip: Ipv4Addr) -> Option<LanInfo> {
        let dir = self.config_dir()?;
        let id = tls::load_or_create(&dir).ok()?;
        // The old listener of a stop just before may take a moment to let go of
        // its port; waiting a little keeps a stored address good (REQ-56).
        let remembered = tls::port_hint(&dir).and_then(|p| {
            (0..20).find_map(|n| {
                if n > 0 {
                    thread::sleep(Duration::from_millis(100));
                }
                TcpListener::bind((ip, p)).ok()
            })
        });
        let listener = remembered.or_else(|| TcpListener::bind((ip, 0)).ok())?;
        let port = listener.local_addr().ok()?.port();
        bridge.add_wifi(listener, id.config.clone()).ok()?;
        tls::remember_port(&dir, port);
        Some(LanInfo {
            host: ip.to_string(),
            port,
            fp: id.fp,
        })
    }

    /// The start thread: the bridge, then the wifi listener (ready at once,
    /// with the first code), then the tunnel behind it.
    fn bring_up(self: &Arc<Self>, epoch: u64, read_only: bool) -> Result<(), String> {
        let store = self.store().ok_or("no config directory")?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| format!("binding a local port: {e}"))?;
        // A `Weak`, or the bridge's callback would keep what holds the bridge
        // alive.
        let me = Arc::downgrade(self);
        let bridge = Bridge::start(
            listener,
            self.socket.clone(),
            Config {
                store: Arc::new(store),
                read_only,
                confirm: Arc::new(move |p| me.upgrade().is_some_and(|m| m.ask(p.name, p.code))),
                look: self.look.clone(),
                lister: lister(),
                timings: Timings::default(),
            },
        )
        .map_err(|e| format!("starting the bridge: {e}"))?;
        let port = bridge.addr().port();
        let bridge = Arc::new(bridge);
        {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return Ok(());
            }
            core.bridge = Some(Arc::clone(&bridge));
        }
        self.ticker(epoch);

        // The wifi first: ready before the tunnel can be (REQ-56).
        if let Some(lan) = self
            .tuning
            .lan
            .addr()
            .and_then(|ip| self.open_wifi(&bridge, ip))
        {
            // Outside `core`: the bridge takes locks of its own.
            let token = bridge.new_pairing().map_err(|e| e.to_string())?;
            {
                let mut core = lock(&self.core);
                if core.epoch != epoch {
                    return Ok(());
                }
                core.lan = Some(lan);
                core.token = token;
                core.phase = Phase::Ready;
                // The tunnel's row is there from the first state.
                core.tun = Tun::Starting("launching".into());
            }
            self.sync_endpoints();
            self.push();
        }

        let cancel = || self.current() != epoch;
        let mut on_step = |step: Step| match step {
            Step::Started(tunnel) => {
                // Kept at once, so a stop can kill it while `up` still waits.
                let mut core = lock(&self.core);
                if core.epoch == epoch {
                    core.tunnel = Some(tunnel);
                } else {
                    tunnel.kill();
                }
                drop(core);
                self.step(epoch, "started");
            }
            Step::Registered(_) => self.step(epoch, "registered"),
            Step::Resolving(_) => self.step(epoch, "resolving"),
        };
        self.step(epoch, "launching");
        let up = tunnel::up(port, self.tuning.waits, &cancel, &mut on_step);
        let host = match up {
            Ok((_tunnel, host)) => host,
            Err(e) => {
                // With the wifi up the tunnel failing is only its row (REQ-67).
                let tunnel = {
                    let mut core = lock(&self.core);
                    if core.epoch != epoch {
                        return Ok(());
                    }
                    if core.lan.is_none() {
                        return Err(e.to_string());
                    }
                    core.tun = Tun::Failed(e.to_string());
                    core.tunnel.take()
                };
                if let Some(tunnel) = tunnel {
                    tunnel.kill();
                }
                self.sync_endpoints();
                self.push();
                return Ok(());
            }
        };

        // With no wifi listener the first code is made now, as before.
        let token = if lock(&self.core).lan.is_none() {
            Some(bridge.new_pairing().map_err(|e| e.to_string())?)
        } else {
            None
        };
        {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return Ok(());
            }
            if let Some(token) = token {
                core.token = token;
                core.phase = Phase::Ready;
            }
            core.host = host;
            core.tun = Tun::Ready;
        }
        // Every phone connected is told where the tunnel is (REQ-61).
        self.sync_endpoints();
        self.push();
        Ok(())
    }

    /// Looks at the state every `tick` while this start lasts: phones come
    /// and go, the code runs out, a device is revoked from a terminal,
    /// `cloudflared` may die, and the PC's address may change (REQ-69).
    fn ticker(self: &Arc<Self>, epoch: u64) {
        let me = Arc::clone(self);
        thread::spawn(move || {
            let mut last_lan = std::time::Instant::now();
            loop {
                thread::sleep(me.tuning.tick);
                let lost = {
                    let core = lock(&me.core);
                    if core.epoch != epoch {
                        return;
                    }
                    matches!(core.tun, Tun::Ready)
                        && core.tunnel.as_ref().is_some_and(|t| t.exited())
                };
                if lost {
                    let wifi_up = {
                        let mut core = lock(&me.core);
                        if core.lan.is_some() {
                            core.tun = Tun::Failed("cloudflared stopped".into());
                            core.host.clear();
                            core.tunnel = None;
                            true
                        } else {
                            false
                        }
                    };
                    if !wifi_up {
                        me.fail(epoch, "cloudflared stopped; start mobile again".into());
                        return;
                    }
                    me.sync_endpoints();
                }
                if last_lan.elapsed() >= me.tuning.lan_every {
                    last_lan = std::time::Instant::now();
                    me.recheck_wifi(epoch);
                }
                me.push();
            }
        });
    }

    /// The PC's address may have changed since the wifi listener opened
    /// (DHCP, another network): rebind on the new one, give a new code (the QR
    /// on screen pointed at the old address) and tell the phones (REQ-69).
    fn recheck_wifi(self: &Arc<Self>, epoch: u64) {
        let now = self.tuning.lan.addr();
        let (bridge, was, ready) = {
            let core = lock(&self.core);
            (
                core.bridge.clone(),
                core.lan.as_ref().map(|l| l.host.clone()),
                matches!(core.phase, Phase::Ready),
            )
        };
        let Some(bridge) = bridge else { return };
        if !ready || was == now.map(|ip| ip.to_string()) {
            return;
        }
        let lan = match now {
            Some(ip) => self.open_wifi(&bridge, ip),
            None => {
                bridge.drop_wifi();
                None
            }
        };
        if now.is_some() && lan.is_none() {
            // The address changed but the new listener did not open: the old
            // one is of no use either.
            bridge.drop_wifi();
        }
        let token = lan.as_ref().and_then(|_| bridge.new_pairing().ok());
        {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return;
            }
            core.lan = lan;
            if let Some(token) = token {
                core.token = token;
            }
        }
        self.sync_endpoints();
        self.push();
        // With neither route left there is nothing to offer.
        let none = {
            let core = lock(&self.core);
            core.lan.is_none() && !matches!(core.tun, Tun::Ready)
        };
        if none {
            self.fail(
                epoch,
                "the network address changed and mobile has no way left to be reached".into(),
            );
        }
    }

    /// An id nobody can guess: a client that is not shown the question cannot
    /// answer it by counting. `None` when the system has no random bytes to
    /// give; there is no weaker fallback.
    fn new_request(&self, taken: &HashMap<u64, mpsc::Sender<Answer>>) -> Option<u64> {
        let random = self.tuning.random.unwrap_or(|| jw_web::auth::random_hex(8));
        for _ in 0..8 {
            let id = u64::from_str_radix(&random().ok()?, 16).ok()?;
            if id != 0 && id != CANCELLED.0 && !taken.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Puts "pair this phone?" to every watcher and waits for the first
    /// answer (REQ-36). With nobody watching, or nobody answering in time, or
    /// mobile not running, it is no.
    fn ask(&self, name: &str, code: &str) -> bool {
        let (answer, answered) = mpsc::channel();
        // Registered before the phase is looked at, so a stop either sees
        // this question and cancels it, or has already left `ready`. One
        // question at a time (REQ-36): a second phone, or a second code used
        // meanwhile, is refused rather than queued, so the person is never
        // asked two things at once.
        let request = {
            let mut pending = lock(&self.pending);
            if !pending.is_empty() {
                return false;
            }
            // No random bytes, no question: an id that could be guessed would
            // let a client that is not shown it answer it, so the phone is
            // refused instead.
            let Some(request) = self.new_request(&pending) else {
                return false;
            };
            pending.insert(request, answer);
            request
        };
        let running = matches!(lock(&self.core).phase, Phase::Ready);
        let asked = running && {
            let msg = DaemonMsg::MobilePair {
                request,
                name: name.into(),
                code: code.into(),
            };
            let mut watchers = lock(&self.watchers);
            watchers.retain(|(_, tx)| tx.send(msg.clone()).is_ok());
            !watchers.is_empty()
        };
        if !asked {
            lock(&self.pending).remove(&request);
            return false;
        }
        let (by, yes) = answered
            .recv_timeout(self.tuning.ask_within)
            .unwrap_or(CANCELLED);
        lock(&self.pending).remove(&request);
        let done = DaemonMsg::MobilePairDone { request };
        lock(&self.watchers).retain(|(c, tx)| *c == by || tx.send(done.clone()).is_ok());
        yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fast() -> Tuning {
        Tuning {
            ask_within: Duration::from_millis(300),
            tick: Duration::from_millis(50),
            waits: tunnel::Waits::default(),
            store: Some(
                std::env::temp_dir().join(format!("jw-mobile-unit-{}.json", std::process::id())),
            ),
            hook: None,
            random: None,
            lan: Lan::Off,
            lan_every: Duration::from_millis(100),
        }
    }

    /// `fast()` with a store of its own, so a test can write one.
    fn with_store(name: &str) -> (Tuning, PathBuf) {
        let path =
            std::env::temp_dir().join(format!("jw-mobile-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        (
            Tuning {
                store: Some(path.clone()),
                ..fast()
            },
            path,
        )
    }

    fn service(tuning: Tuning) -> Mobile {
        Mobile::new(
            PathBuf::from("/nowhere/jw.sock"),
            Arc::new(|| serde_json::json!({})),
            tuning,
        )
    }

    /// A watcher: its `Tx`, and what arrives on it.
    fn watcher(m: &Mobile, client: u64) -> (Tx, mpsc::Receiver<DaemonMsg>) {
        let tx = Tx::new();
        let (out, got) = mpsc::channel();
        let rx = tx.clone();
        thread::spawn(move || {
            while let Some(msg) = rx.recv() {
                if out.send(msg).is_err() {
                    return;
                }
            }
        });
        m.watch(client, &tx);
        (tx, got)
    }

    fn ready(m: &Mobile) {
        lock(&m.0.core).phase = Phase::Ready;
    }

    fn next(got: &mpsc::Receiver<DaemonMsg>) -> DaemonMsg {
        got.recv_timeout(Duration::from_secs(2)).expect("a message")
    }

    fn quiet(got: &mpsc::Receiver<DaemonMsg>) {
        assert!(
            got.recv_timeout(Duration::from_millis(150)).is_err(),
            "no message was expected"
        );
    }

    fn pair_request(got: &mpsc::Receiver<DaemonMsg>) -> u64 {
        match next(got) {
            DaemonMsg::MobilePair { request, .. } => request,
            other => panic!("want MobilePair, got {other:?}"),
        }
    }

    #[test]
    fn a_watcher_gets_the_state_at_once_and_each_change_once() {
        let m = service(fast());
        let (_tx, got) = watcher(&m, 1);
        let DaemonMsg::MobileState(s) = next(&got) else {
            panic!("want MobileState")
        };
        assert_eq!(s.phase, "off");
        m.0.push();
        m.0.push();
        assert!(matches!(next(&got), DaemonMsg::MobileState(_)));
        quiet(&got);
        m.read_only(true);
        let DaemonMsg::MobileState(s) = next(&got) else {
            panic!("want MobileState")
        };
        assert!(s.read_only);
        quiet(&got);
        m.forget(1);
        m.read_only(false);
        quiet(&got);
    }

    #[test]
    fn nobody_watching_means_no() {
        let m = service(fast());
        ready(&m);
        assert!(!m.0.ask("Pixel", "482913"));
        assert!(lock(&m.0.pending).is_empty());
    }

    #[test]
    fn the_last_watcher_leaving_ends_a_question_that_is_waiting() {
        let m = service(Tuning {
            ask_within: Duration::from_secs(60),
            ..fast()
        });
        ready(&m);
        let (_a, a) = watcher(&m, 1);
        let _ = next(&a);
        let asker = {
            let m = m.clone();
            thread::spawn(move || m.0.ask("Pixel", "482913"))
        };
        let _ = pair_request(&a);
        let t = std::time::Instant::now();
        m.forget(1);
        assert!(!asker.join().unwrap());
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn only_a_watcher_can_answer_and_the_ids_are_not_a_count() {
        let m = service(Tuning {
            ask_within: Duration::from_secs(5),
            ..fast()
        });
        ready(&m);
        let (_a, a) = watcher(&m, 1);
        let _ = next(&a);
        let asker = {
            let m = m.clone();
            thread::spawn(move || m.0.ask("Pixel", "482913"))
        };
        let request = pair_request(&a);
        assert!(request > 1_000_000, "{request} is a counter");
        // Another client on the socket, that never watched, cannot answer.
        m.answer(77, request, true);
        quiet(&a);
        assert!(lock(&m.0.pending).contains_key(&request), "it was answered");
        m.answer(1, request, false);
        assert!(!asker.join().unwrap());
    }

    #[test]
    fn without_random_bytes_the_question_is_refused_not_asked_with_a_guessable_id() {
        let m = service(Tuning {
            random: Some(|| Err(std::io::Error::other("no entropy"))),
            ..fast()
        });
        ready(&m);
        let (_a, a) = watcher(&m, 1);
        let _ = next(&a);
        assert!(!m.0.ask("Pixel", "482913"));
        quiet(&a);
        assert!(lock(&m.0.pending).is_empty());
        // An id that is not a number is as unusable.
        let m = service(Tuning {
            random: Some(|| Ok("zz".into())),
            ..fast()
        });
        ready(&m);
        assert!(!m.0.ask("Pixel", "482913"));
    }

    #[test]
    fn a_second_question_is_refused_while_one_is_open() {
        let m = service(Tuning {
            ask_within: Duration::from_secs(5),
            ..fast()
        });
        ready(&m);
        let (_a, a) = watcher(&m, 1);
        let _ = next(&a);
        let first = {
            let m = m.clone();
            thread::spawn(move || m.0.ask("Pixel", "482913"))
        };
        let request = pair_request(&a);
        let t = std::time::Instant::now();
        assert!(!m.0.ask("Galaxy", "111222"), "a second question was asked");
        assert!(t.elapsed() < Duration::from_secs(1));
        quiet(&a);
        m.answer(1, request, true);
        assert!(first.join().unwrap());
    }

    #[test]
    fn a_stop_that_cannot_finish_is_still_over_in_time() {
        let stalled = Arc::new(AtomicU64::new(0));
        let seen = stalled.clone();
        let m = service(Tuning {
            hook: Some(Arc::new(move |stage| {
                if stage == "stop" {
                    seen.fetch_add(1, Ordering::SeqCst);
                    // A phone whose socket never lets go.
                    thread::sleep(Duration::from_secs(5));
                }
            })),
            ..fast()
        });
        ready(&m);
        let t = std::time::Instant::now();
        m.stop(Duration::from_millis(300));
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        assert_eq!(m.snapshot().phase, "off");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(stalled.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panic_while_starting_becomes_a_failure_and_the_next_start_works() {
        let (tuning, _) = with_store("panic");
        let m = service(Tuning {
            hook: Some(Arc::new(|stage| {
                if stage == "start" {
                    panic!("a bug in the start");
                }
            })),
            ..tuning
        });
        let (tx, got) = watcher(&m, 1);
        let _ = next(&got);
        m.start(false, &tx);
        let failed = loop {
            if let DaemonMsg::MobileState(s) = next(&got)
                && s.phase == "failed"
            {
                break s;
            }
        };
        assert!(failed.error.contains("unexpectedly"), "{}", failed.error);
        // Failed is not starting: a start is accepted again.
        m.start(false, &tx);
        assert!(matches!(
            lock(&m.0.core).phase,
            Phase::Starting(_) | Phase::Failed(_)
        ));
    }

    #[test]
    fn the_state_does_not_read_the_store_it_has_cached() {
        let (tuning, path) = with_store("cache");
        let m = service(tuning);
        let store = Store::open(path.clone());
        store.add("Pixel 8", &jw_web::auth::hash("secret")).unwrap();
        m.0.refresh_devices();
        assert_eq!(m.snapshot().devices.len(), 1);
        // The file goes; a snapshot, which is what runs under `last`, does
        // not look.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(m.snapshot().devices.len(), 1);
        // The next look at the file does.
        m.0.refresh_devices();
        assert!(m.snapshot().devices.is_empty());
        // A change is picked up by the stamp, and a revoke at once.
        let store = Store::open(path.clone());
        store.add("Tablet", &jw_web::auth::hash("other")).unwrap();
        m.0.refresh_devices();
        assert_eq!(m.snapshot().devices[0].name, "Tablet");
        m.revoke("Tablet").unwrap();
        assert!(m.snapshot().devices.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nothing_is_asked_while_mobile_is_off() {
        let m = service(fast());
        let (_tx, got) = watcher(&m, 1);
        let _ = next(&got);
        assert!(!m.0.ask("Pixel", "482913"));
        quiet(&got);
    }

    #[test]
    fn the_first_answer_wins_and_the_other_watcher_is_told_it_is_over() {
        let m = service(Tuning {
            ask_within: Duration::from_secs(5),
            ..fast()
        });
        ready(&m);
        let (_a, a) = watcher(&m, 1);
        let (_b, b) = watcher(&m, 2);
        let _ = (next(&a), next(&b));
        let asker = {
            let m = m.clone();
            thread::spawn(move || m.0.ask("Pixel", "482913"))
        };
        let (ra, rb) = (pair_request(&a), pair_request(&b));
        assert_eq!(ra, rb);
        m.answer(2, ra, true);
        // A late answer finds nothing.
        m.answer(1, ra, false);
        assert!(asker.join().unwrap());
        assert_eq!(next(&a), DaemonMsg::MobilePairDone { request: ra });
        // The one that answered is not told.
        quiet(&b);
    }

    #[test]
    fn a_question_nobody_answers_is_no_and_closes_everywhere() {
        let m = service(fast());
        ready(&m);
        let (_a, a) = watcher(&m, 1);
        let _ = next(&a);
        let t = std::time::Instant::now();
        assert!(!m.0.ask("Pixel", "482913"));
        assert!(t.elapsed() < Duration::from_secs(2));
        let request = pair_request(&a);
        assert_eq!(next(&a), DaemonMsg::MobilePairDone { request });
        assert!(lock(&m.0.pending).is_empty());
    }

    #[test]
    fn stopping_ends_a_question_that_is_waiting() {
        let m = service(Tuning {
            ask_within: Duration::from_secs(60),
            ..fast()
        });
        ready(&m);
        let (_a, a) = watcher(&m, 1);
        let _ = next(&a);
        let asker = {
            let m = m.clone();
            thread::spawn(move || m.0.ask("Pixel", "482913"))
        };
        let request = pair_request(&a);
        let t = std::time::Instant::now();
        m.stop(Duration::from_secs(2));
        assert!(!asker.join().unwrap());
        assert!(t.elapsed() < Duration::from_secs(2));
        // The state going to `off` may come before or after the end of the
        // question.
        let mut over = false;
        while !over {
            match next(&a) {
                DaemonMsg::MobilePairDone { request: r } => over = r == request,
                DaemonMsg::MobileState(s) => assert_eq!(s.phase, "off"),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(m.snapshot().phase, "off");
    }

    #[test]
    fn start_while_running_changes_nothing_and_answers_the_asker() {
        let m = service(fast());
        ready(&m);
        let from = Tx::new();
        let rx = from.clone();
        m.start(false, &from);
        let Some(DaemonMsg::MobileState(s)) = rx.recv() else {
            panic!("want MobileState")
        };
        assert_eq!(s.phase, "ready");
        assert_eq!(lock(&m.0.core).epoch, 0);
    }

    #[test]
    fn a_code_needs_a_ready_mobile() {
        let m = service(fast());
        assert!(m.code().is_err());
    }
}
