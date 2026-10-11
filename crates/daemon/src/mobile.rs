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
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use jw_core::core::config;
use jw_web::auth::Store;
use jw_web::bridge::{Bridge, Config, Look, Timings};
use jw_web::tunnel::{self, Step, Tunnel};

use crate::outbox::Tx;
use crate::proto::{DaemonMsg, MobileDevice, MobileState};

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
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            ask_within: Duration::from_secs(60),
            tick: Duration::from_secs(1),
            waits: tunnel::Waits::default(),
            store: None,
        }
    }
}

/// Who answered a pair question (a client id) and what; `CANCELLED` when
/// nobody did because mobile ended.
type Answer = (u64, bool);
const CANCELLED: Answer = (u64::MAX, false);

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
    bridge: Option<Bridge>,
    tunnel: Option<Tunnel>,
    host: String,
    token: String,
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
    next_request: AtomicU64,
    /// What was sent last, with the countdown left out, so nothing is sent
    /// twice.
    last: Mutex<Option<MobileState>>,
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
            }),
            watchers: Mutex::default(),
            pending: Mutex::default(),
            next_request: AtomicU64::new(1),
            last: Mutex::default(),
        }))
    }

    /// Makes `client` a watcher and tells it where mobile stands (REQ-35).
    pub fn watch(&self, client: u64, tx: &Tx) {
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
            if let Err(why) = inner.bring_up(epoch, read_only) {
                inner.fail(epoch, why);
            }
        });
        self.0.push();
    }

    /// Stops mobile: `cloudflared` is killed and every phone and question
    /// closed before this returns; the threads are joined on another one, and
    /// waited for up to `wait` (REQ-9).
    pub fn stop(&self, wait: Duration) {
        let (bridge, tunnel) = {
            let mut core = lock(&self.0.core);
            core.epoch += 1;
            core.phase = Phase::Off;
            core.host.clear();
            core.token.clear();
            (core.bridge.take(), core.tunnel.take())
        };
        self.0.wind_down(&bridge, &tunnel);
        self.0.push();
        if bridge.is_none() && tunnel.is_none() {
            return;
        }
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            drop(bridge);
            drop(tunnel);
            let _ = done.send(());
        });
        let _ = finished.recv_timeout(wait);
    }

    /// A new pairing code, which ends the one before (REQ-38).
    pub fn code(&self) -> Result<()> {
        {
            let mut core = lock(&self.0.core);
            let Some(bridge) = core
                .bridge
                .as_ref()
                .filter(|_| matches!(core.phase, Phase::Ready))
            else {
                bail!("mobile is not ready");
            };
            let token = bridge.new_pairing()?;
            core.token = token;
        }
        self.0.push();
        Ok(())
    }

    /// Applies at once to the phones connected and to new ones (REQ-37).
    pub fn read_only(&self, on: bool) {
        {
            let mut core = lock(&self.0.core);
            core.read_only = on;
            if let Some(bridge) = &core.bridge {
                bridge.set_read_only(on);
            }
        }
        self.0.push();
    }

    /// The first answer to a pair question wins (REQ-36); later ones find
    /// nothing pending.
    pub fn answer(&self, client: u64, request: u64, yes: bool) {
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

    fn current(&self) -> u64 {
        lock(&self.core).epoch
    }

    /// Where mobile stands now. The pairing token is in `payload` (RISK-16).
    fn snapshot(&self) -> MobileState {
        let (phase, step, error, host, token, read_only, connected, left) = {
            let core = lock(&self.core);
            let (phase, step, error) = match &core.phase {
                Phase::Off => ("off", String::new(), String::new()),
                Phase::Starting(s) => ("starting", s.clone(), String::new()),
                Phase::Ready => ("ready", String::new(), String::new()),
                Phase::Failed(e) => ("failed", String::new(), e.clone()),
            };
            let (connected, left) = core
                .bridge
                .as_ref()
                .map_or((Vec::new(), None), |b| (b.connected(), b.pairing_left()));
            (
                phase,
                step,
                error,
                core.host.clone(),
                core.token.clone(),
                core.read_only,
                connected,
                left,
            )
        };
        let devices = self
            .store()
            .and_then(|s| s.devices().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|d| MobileDevice {
                connected: connected.contains(&d.id),
                id: d.id,
                name: d.name,
                paired: d.created,
                last_seen: d.last_seen,
            })
            .collect();
        let live = phase == "ready" && left.is_some();
        MobileState {
            phase: phase.into(),
            step,
            payload: if live {
                format!("jw://{host}/{token}")
            } else {
                String::new()
            },
            host,
            expires_in: left.map_or(0, |d| d.as_secs_f64().ceil() as u64),
            read_only,
            devices,
            error,
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

    /// The part of ending mobile that has to be quick: kill the tunnel, say
    /// no to every question, and close every phone's socket. Nothing here
    /// waits for a thread.
    fn wind_down(&self, bridge: &Option<Bridge>, tunnel: &Option<Tunnel>) {
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
    fn fail(&self, epoch: u64, why: String) {
        let (bridge, tunnel) = {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return;
            }
            core.epoch += 1;
            core.phase = Phase::Failed(why);
            core.host.clear();
            core.token.clear();
            (core.bridge.take(), core.tunnel.take())
        };
        self.wind_down(&bridge, &tunnel);
        self.push();
        thread::spawn(move || {
            drop(bridge);
            drop(tunnel);
        });
    }

    fn step(&self, epoch: u64, text: &str) {
        {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return;
            }
            core.phase = Phase::Starting(text.into());
        }
        self.push();
    }

    /// The start thread: bridge, tunnel, then the first code.
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
                timings: Timings::default(),
            },
        )
        .map_err(|e| format!("starting the bridge: {e}"))?;
        let port = bridge.addr().port();
        {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return Ok(());
            }
            core.bridge = Some(bridge);
        }
        self.ticker(epoch);

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
        let (_tunnel, host) = tunnel::up(port, self.tuning.waits, &cancel, &mut on_step)
            .map_err(|e| e.to_string())?;

        {
            let mut core = lock(&self.core);
            if core.epoch != epoch {
                return Ok(());
            }
            let Some(bridge) = core.bridge.as_ref() else {
                return Err("the bridge is gone".into());
            };
            let token = bridge.new_pairing().map_err(|e| e.to_string())?;
            core.token = token;
            core.host = host;
            core.phase = Phase::Ready;
        }
        self.push();
        Ok(())
    }

    /// Looks at the state every `tick` while this start lasts: phones come
    /// and go, the code runs out, a device is revoked from a terminal, and
    /// `cloudflared` may die.
    fn ticker(self: &Arc<Self>, epoch: u64) {
        let me = Arc::clone(self);
        thread::spawn(move || {
            loop {
                thread::sleep(me.tuning.tick);
                {
                    let core = lock(&me.core);
                    if core.epoch != epoch {
                        return;
                    }
                    let lost = matches!(core.phase, Phase::Ready)
                        && core.tunnel.as_ref().is_some_and(|t| t.exited());
                    if lost {
                        drop(core);
                        me.fail(epoch, "cloudflared stopped; start mobile again".into());
                        return;
                    }
                }
                me.push();
            }
        });
    }

    /// Puts "pair this phone?" to every watcher and waits for the first
    /// answer (REQ-36). With nobody watching, or nobody answering in time, or
    /// mobile not running, it is no.
    fn ask(&self, name: &str, code: &str) -> bool {
        let (answer, answered) = mpsc::channel();
        let request = self.next_request.fetch_add(1, Ordering::Relaxed);
        // Registered before the phase is looked at, so a stop either sees
        // this question and cancels it, or has already left `ready`.
        lock(&self.pending).insert(request, answer);
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

    fn fast() -> Tuning {
        Tuning {
            ask_within: Duration::from_millis(300),
            tick: Duration::from_millis(50),
            waits: tunnel::Waits::default(),
            store: Some(
                std::env::temp_dir().join(format!("jw-mobile-unit-{}.json", std::process::id())),
            ),
        }
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
