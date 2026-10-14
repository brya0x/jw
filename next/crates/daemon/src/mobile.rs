//! The phone link as the daemon hosts it (REQ-75, REQ-80, REQ-81): the bridge, the tunnel,
//! and the pairing question that `jwn pair` answers.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use web::auth::{Pairing, Store};
use web::bridge::{Answer, Ask, Bridge, Config as BridgeConfig, Confirm, Endpoints, Lan};
use web::tls;
use web::tunnel::Tunnel;

/// How long the person has to answer, and how long a pairing token lives.
const ANSWER_WITHIN: Duration = Duration::from_secs(60);
const TOKEN_LIFE: Duration = Duration::from_secs(120);
/// How long the tunnel may take to come up (the name can resolve about a minute late).
const TUNNEL_WAIT: Duration = Duration::from_secs(90);
/// A `jwn pair` between two waits still counts as present for this long.
const PRESENT_GRACE: Duration = Duration::from_secs(5);

/// What the daemon is told about the phone link at start.
#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// The tunnel program (`cloudflared`); none means no tunnel.
    pub tunnel: Option<String>,
    /// Wait for the tunnel's host name to resolve before offering it.
    pub resolve: bool,
    /// Also listen with TLS on the private address.
    pub lan: bool,
    /// Listen there instead of on the address found for this machine (tests use loopback).
    pub lan_ip: Option<std::net::Ipv4Addr>,
}

impl LinkConfig {
    pub fn from_identity() -> LinkConfig {
        let override_ = std::env::var(identity::env_var("CLOUDFLARED")).ok();
        LinkConfig {
            resolve: override_.is_none(),
            tunnel: Some(override_.unwrap_or_else(|| identity::get().tunnel_program.clone())),
            lan: true,
            lan_ip: None,
        }
    }

    pub fn off() -> LinkConfig {
        LinkConfig {
            tunnel: None,
            resolve: false,
            lan: false,
            lan_ip: None,
        }
    }
}

#[derive(Default)]
struct AskState {
    pending: Option<Ask>,
    answer: Option<bool>,
    /// A `jwn pair` is waiting until then.
    present_until: Option<Instant>,
}

/// The one pairing question: the bridge puts it, `mobile_wait` hands it out and
/// `mobile_answer` settles it.
#[derive(Default)]
struct Asker {
    state: Mutex<AskState>,
    changed: Condvar,
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Asker {
    fn ask(&self, ask: &Ask) -> Answer {
        let mut s = guard(&self.state);
        if s.present_until.is_none_or(|t| t < Instant::now()) || s.pending.is_some() {
            return Answer::Timeout;
        }
        s.pending = Some(ask.clone());
        s.answer = None;
        self.changed.notify_all();
        let deadline = Instant::now() + ANSWER_WITHIN;
        while s.answer.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            s = self
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        s.pending = None;
        match s.answer.take() {
            Some(true) => Answer::Yes,
            Some(false) => Answer::No,
            None => Answer::Timeout,
        }
    }

    fn wait(&self, seconds: u32) -> Option<Ask> {
        let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
        let mut s = guard(&self.state);
        s.present_until = Some(deadline + PRESENT_GRACE);
        while s.pending.is_none() || s.answer.is_some() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            s = self
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        s.pending.clone()
    }

    fn answer(&self, yes: bool) -> bool {
        let mut s = guard(&self.state);
        if s.pending.is_none() || s.answer.is_some() {
            return false;
        }
        s.answer = Some(yes);
        self.changed.notify_all();
        true
    }
}

struct Running {
    bridge: Bridge,
    pairing: Arc<Pairing>,
    tunnel: Option<Tunnel>,
}

/// The phone link of one daemon.
pub struct Hub {
    socket: PathBuf,
    state_dir: PathBuf,
    config: LinkConfig,
    asker: Arc<Asker>,
    running: Mutex<Option<Running>>,
    /// Held while a tunnel is being brought up, so two callers never start two.
    tunnel_lock: Mutex<()>,
    /// A tunnel is being brought up in the background: what `pending` says.
    tunnel_busy: AtomicBool,
}

/// The paired devices: a file `jwn devices` reads with no daemon running.
pub fn devices_file(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join(&identity::get().pairing_store)
}

/// The name of this machine as a phone shows it: `hostname` without a `.local` tail, if it says one.
fn host_name() -> Option<String> {
    let out = std::process::Command::new("hostname").output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout);
    let name = name.trim().trim_end_matches(".local");
    (!name.is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control))
        .then(|| name.to_owned())
}

/// `<scheme>://<ipv4>:<port>/<token>?fp=<hex>[&t=<tunnel>]`, or `<scheme>://<tunnel>/<token>`.
pub fn link_uri(
    token: &str,
    lan: Option<(&SocketAddr, &str)>,
    tunnel: Option<&str>,
) -> Option<String> {
    let scheme = &identity::get().name;
    match (lan, tunnel) {
        (Some((addr, fp)), t) => {
            let extra = t.map(|t| format!("&t={t}")).unwrap_or_default();
            Some(format!("{scheme}://{addr}/{token}?fp={fp}{extra}"))
        }
        (None, Some(t)) => Some(format!("{scheme}://{t}/{token}")),
        (None, None) => None,
    }
}

impl Hub {
    pub fn new(socket: PathBuf, state_dir: PathBuf, config: LinkConfig) -> Hub {
        Hub {
            socket,
            state_dir,
            config,
            asker: Arc::default(),
            running: Mutex::new(None),
            tunnel_lock: Mutex::new(()),
            tunnel_busy: AtomicBool::new(false),
        }
    }

    /// Turn the link on if it is off, and make a new pairing token. With a private address the
    /// answer is at once, over that address, and the tunnel opens in the background (the
    /// connected phones are told its host when it is up); without one, the tunnel is the only
    /// way in and this waits for it, up to 90 s, without holding the lock that `stop` needs.
    pub fn start(self: &Arc<Self>) -> Result<Value, String> {
        self.ensure_bridge()?;
        let lan = guard(&self.running)
            .as_ref()
            .is_some_and(|r| r.bridge.lan.is_some());
        if lan {
            self.open_tunnel_in_background();
        } else {
            self.ensure_tunnel();
        }
        let token = {
            let slot = guard(&self.running);
            let run = slot.as_ref().ok_or("the phone link was turned off")?;
            run.pairing
                .issue()
                .map_err(|e| format!("no random numbers: {e}"))?
        };
        self.link_with(&token)
    }

    /// The link as it is now, with the pairing token on offer (a new one only when it is gone
    /// or spent): what a client asks for while the tunnel opens.
    pub fn link(&self) -> Result<Value, String> {
        let token = {
            let slot = guard(&self.running);
            let run = slot.as_ref().ok_or("the phone link is off")?;
            match run.pairing.current() {
                Some(t) => t,
                None => run
                    .pairing
                    .issue()
                    .map_err(|e| format!("no random numbers: {e}"))?,
            }
        };
        self.link_with(&token)
    }

    fn link_with(&self, token: &str) -> Result<Value, String> {
        let slot = guard(&self.running);
        let run = slot.as_ref().ok_or("the phone link was turned off")?;
        let lan = run.bridge.lan.as_ref();
        let tunnel = run.tunnel.as_ref().map(|t| t.host.as_str());
        let uri = link_uri(token, lan.map(|(a, fp)| (a, fp.as_str())), tunnel)
            .ok_or("there is no way to reach this PC: install cloudflared or join a network")?;
        let pending = self.config.tunnel.is_some()
            && run.tunnel.is_none()
            && self.tunnel_busy.load(Ordering::SeqCst);
        let mut out = json!({"uri": uri, "local": run.bridge.addr.to_string(), "pending": pending});
        if let Some((addr, _)) = lan {
            out["lan"] = json!(addr.to_string());
        }
        if let Some(host) = tunnel {
            out["tunnel"] = json!(host);
        }
        Ok(out)
    }

    /// Bring the tunnel up on a thread of its own, unless there is one or one is coming. It
    /// stays up until the link is turned off, whoever asked for it.
    fn open_tunnel_in_background(self: &Arc<Self>) {
        if self.config.tunnel.is_none()
            || guard(&self.running)
                .as_ref()
                .is_none_or(|r| r.tunnel.is_some())
            || self.tunnel_busy.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let hub = self.clone();
        let spawned = std::thread::Builder::new()
            .name("mobile-tunnel".into())
            .spawn(move || {
                hub.ensure_tunnel();
                hub.tunnel_busy.store(false, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.tunnel_busy.store(false, Ordering::SeqCst);
        }
    }

    /// A daemon that starts with phones already paired opens the link again, on the same network
    /// port, so those phones find this PC without anyone running `jwn pair` (REQ-86). The tunnel
    /// comes up in the background.
    pub fn resume(self: &Arc<Self>) {
        let paired = Store::new(devices_file(&self.state_dir))
            .list()
            .is_ok_and(|l| !l.is_empty());
        if !paired {
            return;
        }
        let hub = self.clone();
        let _ = std::thread::Builder::new()
            .name("mobile-resume".into())
            .spawn(move || match hub.ensure_bridge() {
                Ok(()) => hub.open_tunnel_in_background(),
                Err(e) => eprintln!("the phone link did not come back: {e}"),
            });
    }

    /// The bridge and the private-address listener, started if they are not running.
    fn ensure_bridge(&self) -> Result<(), String> {
        let mut slot = guard(&self.running);
        if slot.is_some() {
            return Ok(());
        }
        let store = Arc::new(Store::new(devices_file(&self.state_dir)));
        let pairing = Arc::new(Pairing::new(TOKEN_LIFE));
        let asker = self.asker.clone();
        let confirm: Arc<Confirm> = Arc::new(move |ask| asker.ask(ask));
        let lan_ip = self
            .config
            .lan_ip
            .or_else(|| if self.config.lan { tls::lan_ip() } else { None });
        let port_file = self.state_dir.join("link-port");
        let config = |port: u16| BridgeConfig {
            bind: "127.0.0.1:0".parse().expect("a literal address"),
            lan: lan_ip.map(|ip| Lan {
                ip,
                port,
                cert: self.state_dir.join("link-cert.pem"),
            }),
            daemon_socket: self.socket.clone(),
            store: store.clone(),
            pairing: pairing.clone(),
            confirm: confirm.clone(),
            host: host_name(),
        };
        let saved = std::fs::read_to_string(&port_file)
            .ok()
            .and_then(|t| t.trim().parse::<u16>().ok())
            .unwrap_or(0);
        // The port a phone remembers; if something else holds it now, any port will do.
        let bridge = Bridge::start(config(saved))
            .or_else(|_| Bridge::start(config(0)))
            .map_err(|e| format!("cannot start the phone link: {e}"))?;
        if let Some((addr, _)) = &bridge.lan {
            let _ = std::fs::write(&port_file, addr.port().to_string());
        }
        bridge.set_endpoints(Endpoints {
            lan: bridge.lan.as_ref().map(|(a, _)| a.to_string()),
            tunnel: None,
        });
        *slot = Some(Running {
            bridge,
            pairing,
            tunnel: None,
        });
        Ok(())
    }

    /// The tunnel to the running bridge, started if it is not. It is brought up without the
    /// `running` lock (it takes up to a minute and a half) and attached only if the link is
    /// still the one it was made for.
    fn ensure_tunnel(&self) {
        let Some(program) = self.config.tunnel.as_deref() else {
            return;
        };
        let _one_at_a_time = guard(&self.tunnel_lock);
        let port = match &*guard(&self.running) {
            Some(run) if run.tunnel.is_none() => run.bridge.addr.port(),
            _ => return,
        };
        let pidfile = self.state_dir.join("tunnel.pid");
        web::tunnel::reap(&pidfile, program);
        let tunnel = match Tunnel::up(
            program,
            port,
            self.config.resolve,
            TUNNEL_WAIT,
            Some(&pidfile),
        ) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("no tunnel: {e}");
                return;
            }
        };
        let mut slot = guard(&self.running);
        match slot.as_mut() {
            Some(run) if run.bridge.addr.port() == port => {
                run.bridge.set_endpoints(Endpoints {
                    lan: run.bridge.lan.as_ref().map(|(a, _)| a.to_string()),
                    tunnel: Some(tunnel.host.clone()),
                });
                run.tunnel = Some(tunnel);
            }
            // The link was turned off meanwhile: the tunnel ends with this value.
            _ => {}
        }
    }

    pub fn wait(&self, seconds: u32) -> Option<Ask> {
        self.asker.wait(seconds)
    }

    pub fn answer(&self, yes: bool) -> bool {
        self.asker.answer(yes)
    }

    /// Turn the link off.
    pub fn stop(&self) {
        let run = guard(&self.running).take();
        if let Some(run) = run {
            run.bridge.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_carries_what_a_phone_needs_to_reach_the_pc() {
        let addr: SocketAddr = "192.168.1.20:7443".parse().unwrap();
        let fp = "ab".repeat(32);
        let scheme = &identity::get().name;
        assert_eq!(
            link_uri("t0k", Some((&addr, &fp)), None).unwrap(),
            format!("{scheme}://192.168.1.20:7443/t0k?fp={fp}")
        );
        assert_eq!(
            link_uri("t0k", Some((&addr, &fp)), Some("a-b.trycloudflare.com")).unwrap(),
            format!("{scheme}://192.168.1.20:7443/t0k?fp={fp}&t=a-b.trycloudflare.com")
        );
        assert_eq!(
            link_uri("t0k", None, Some("a-b.trycloudflare.com")).unwrap(),
            format!("{scheme}://a-b.trycloudflare.com/t0k")
        );
        assert!(link_uri("t0k", None, None).is_none());
    }

    #[test]
    fn the_question_waits_for_a_person_and_is_refused_when_nobody_is_there() {
        let asker = Arc::new(Asker::default());
        let ask = Ask {
            name: "Pixel".into(),
            code: "123456".into(),
        };
        assert_eq!(asker.ask(&ask), Answer::Timeout, "no jwn pair is waiting");
        let a = asker.clone();
        let person = std::thread::spawn(move || {
            let got = a.wait(5).expect("the question arrives");
            assert!(a.answer(true));
            got
        });
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(asker.ask(&ask), Answer::Yes);
        assert_eq!(person.join().unwrap(), ask);
        assert!(!asker.answer(true), "nothing left to answer");
    }
}
