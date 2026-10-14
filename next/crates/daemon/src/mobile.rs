//! The phone link as the daemon hosts it (REQ-75, REQ-80, REQ-81): the bridge, the tunnel,
//! and the pairing question that `jwn pair` answers.

use std::net::SocketAddr;
use std::path::PathBuf;
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
}

impl LinkConfig {
    pub fn from_identity() -> LinkConfig {
        let override_ = std::env::var(identity::env_var("CLOUDFLARED")).ok();
        LinkConfig {
            resolve: override_.is_none(),
            tunnel: Some(override_.unwrap_or_else(|| identity::get().tunnel_program.clone())),
            lan: true,
        }
    }

    pub fn off() -> LinkConfig {
        LinkConfig {
            tunnel: None,
            resolve: false,
            lan: false,
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
}

/// The paired devices: a file `jwn devices` reads with no daemon running.
pub fn devices_file(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join(&identity::get().pairing_store)
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
        }
    }

    /// Turn the link on if it is off, and make a new pairing token.
    pub fn start(&self) -> Result<Value, String> {
        let mut slot = guard(&self.running);
        if slot.is_none() {
            *slot = Some(self.bring_up()?);
        }
        let run = slot.as_ref().expect("just set");
        let token = run
            .pairing
            .issue()
            .map_err(|e| format!("no random numbers: {e}"))?;
        let lan = run.bridge.lan.as_ref();
        let tunnel = run.tunnel.as_ref().map(|t| t.host.as_str());
        let uri = link_uri(&token, lan.map(|(a, fp)| (a, fp.as_str())), tunnel)
            .ok_or("there is no way to reach this PC: install cloudflared or join a network")?;
        let mut out = json!({"uri": uri, "local": run.bridge.addr.to_string()});
        if let Some((addr, _)) = lan {
            out["lan"] = json!(addr.to_string());
        }
        if let Some(host) = tunnel {
            out["tunnel"] = json!(host);
        }
        Ok(out)
    }

    fn bring_up(&self) -> Result<Running, String> {
        let store = Arc::new(Store::new(devices_file(&self.state_dir)));
        let pairing = Arc::new(Pairing::new(TOKEN_LIFE));
        let asker = self.asker.clone();
        let confirm: Arc<Confirm> = Arc::new(move |ask| asker.ask(ask));
        let lan_ip = if self.config.lan { tls::lan_ip() } else { None };
        let bridge = Bridge::start(BridgeConfig {
            bind: "127.0.0.1:0".parse().expect("a literal address"),
            lan: lan_ip.map(|ip| Lan {
                ip,
                port: 0,
                cert: self.state_dir.join("link-cert.pem"),
            }),
            daemon_socket: self.socket.clone(),
            store,
            pairing: pairing.clone(),
            confirm,
        })
        .map_err(|e| format!("cannot start the phone link: {e}"))?;
        let pidfile = self.state_dir.join("tunnel.pid");
        let tunnel = self.config.tunnel.as_deref().and_then(|program| {
            web::tunnel::reap(&pidfile, program);
            match Tunnel::up(
                program,
                bridge.addr.port(),
                self.config.resolve,
                TUNNEL_WAIT,
                Some(&pidfile),
            ) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("no tunnel: {e}");
                    None
                }
            }
        });
        bridge.set_endpoints(Endpoints {
            lan: bridge.lan.as_ref().map(|(a, _)| a.to_string()),
            tunnel: tunnel.as_ref().map(|t| t.host.clone()),
        });
        Ok(Running {
            bridge,
            pairing,
            tunnel,
        })
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
