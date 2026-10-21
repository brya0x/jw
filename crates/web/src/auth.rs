//! Pairing tokens, the paired devices and the limit on failed attempts.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `bytes` random bytes from the operating system, as lowercase hex.
pub fn random_hex(bytes: usize) -> io::Result<String> {
    let mut buf = vec![0u8; bytes];
    fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(hex_bytes(&buf))
}

pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(text: &str) -> String {
    sha256_bytes_hex(text.as_bytes())
}

pub fn sha256_bytes_hex(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}

/// Equality that takes the same time wherever the texts differ.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The one pairing token on offer: 128 random bits, spent the first time it is presented.
pub struct Pairing {
    ttl: Duration,
    token: Mutex<Option<(String, Instant)>>,
}

impl Pairing {
    pub fn new(ttl: Duration) -> Pairing {
        Pairing {
            ttl,
            token: Mutex::new(None),
        }
    }

    /// A new token; any earlier one stops working.
    pub fn issue(&self) -> io::Result<String> {
        let token = random_hex(16)?;
        *lock(&self.token) = Some((token.clone(), Instant::now()));
        Ok(token)
    }

    /// The token on offer, while it can still be spent.
    pub fn current(&self) -> Option<String> {
        lock(&self.token)
            .as_ref()
            .filter(|(_, at)| at.elapsed() <= self.ttl)
            .map(|(t, _)| t.clone())
    }

    /// Whether `presented` is the live token. A match spends it, whatever comes next.
    pub fn take(&self, presented: &str) -> bool {
        let mut slot = lock(&self.token);
        let live = slot
            .as_ref()
            .is_some_and(|(t, at)| same(t, presented) && at.elapsed() <= self.ttl);
        if live {
            *slot = None;
        }
        live
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// SHA-256 of the secret; the secret itself is never kept.
    pub hash: String,
    pub created: u64,
    pub last_seen: u64,
}

/// The paired devices, in a file (`devices.json`, mode 0600) that is read afresh each time,
/// so `jwn revoke` works with the bridge running or not.
pub struct Store {
    path: PathBuf,
    write: Mutex<()>,
}

impl Store {
    pub fn new(path: PathBuf) -> Store {
        Store {
            path,
            write: Mutex::new(()),
        }
    }

    pub fn list(&self) -> io::Result<Vec<Device>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", self.path.display()),
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, devices: &[Device]) -> io::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(serde_json::to_string_pretty(devices)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, &self.path)
    }

    /// A new device: its id and the secret, which is returned here and nowhere else.
    pub fn add(&self, name: &str) -> io::Result<(String, String)> {
        let _guard = lock(&self.write);
        let mut devices = self.list()?;
        let (id, secret) = (random_hex(8)?, random_hex(32)?);
        devices.push(Device {
            id: id.clone(),
            name: name.to_owned(),
            hash: sha256_hex(&secret),
            created: now(),
            last_seen: now(),
        });
        self.save(&devices)?;
        Ok((id, secret))
    }

    /// The device `id` when `secret` is its secret.
    pub fn find(&self, id: &str, secret: &str) -> Option<Device> {
        let hash = sha256_hex(secret);
        self.list()
            .ok()?
            .into_iter()
            .find(|d| d.id == id && same(&d.hash, &hash))
    }

    pub fn is_paired(&self, id: &str) -> bool {
        self.list().is_ok_and(|l| l.iter().any(|d| d.id == id))
    }

    pub fn touch(&self, id: &str) -> io::Result<()> {
        let _guard = lock(&self.write);
        let mut devices = self.list()?;
        if let Some(d) = devices.iter_mut().find(|d| d.id == id) {
            d.last_seen = now();
            self.save(&devices)?;
        }
        Ok(())
    }

    /// Remove the device whose id or name is `which`; false when there is none.
    pub fn revoke(&self, which: &str) -> io::Result<bool> {
        let _guard = lock(&self.write);
        let mut devices = self.list()?;
        let before = devices.len();
        devices.retain(|d| d.id != which && d.name != which);
        if devices.len() == before {
            return Ok(false);
        }
        self.save(&devices)?;
        Ok(true)
    }
}

const FAILS: usize = 5;
const WINDOW: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Tries {
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

/// Five failures from one address in a minute lock it out for the next minute.
#[derive(Default)]
pub struct Limiter {
    seen: Mutex<HashMap<String, Tries>>,
}

impl Limiter {
    pub fn allowed(&self, key: &str) -> bool {
        let mut seen = lock(&self.seen);
        let Some(tries) = seen.get_mut(key) else {
            return true;
        };
        match tries.locked_until {
            Some(t) if t > Instant::now() => false,
            _ => {
                tries.locked_until = None;
                true
            }
        }
    }

    pub fn failed(&self, key: &str) {
        let mut seen = lock(&self.seen);
        let tries = seen.entry(key.to_owned()).or_default();
        let now = Instant::now();
        tries.failures.push_back(now);
        while tries
            .failures
            .front()
            .is_some_and(|t| now.duration_since(*t) > WINDOW)
        {
            tries.failures.pop_front();
        }
        if tries.failures.len() >= FAILS {
            tries.locked_until = Some(now + WINDOW);
            tries.failures.clear();
        }
    }

    pub fn cleared(&self, key: &str) {
        lock(&self.seen).remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_spent_the_first_time_it_is_presented() {
        let p = Pairing::new(Duration::from_secs(120));
        let t = p.issue().unwrap();
        assert_eq!(t.len(), 32);
        assert!(
            !p.take("00000000000000000000000000000000"),
            "a wrong token spends nothing"
        );
        assert!(p.take(&t));
        assert!(!p.take(&t), "one use");
    }

    #[test]
    fn a_token_expires_and_a_new_one_replaces_the_old() {
        let p = Pairing::new(Duration::from_millis(30));
        let t = p.issue().unwrap();
        std::thread::sleep(Duration::from_millis(60));
        assert!(!p.take(&t), "expired");
        let a = p.issue().unwrap();
        let b = p.issue().unwrap();
        assert!(!p.take(&a));
        assert!(p.take(&b));
    }

    #[test]
    fn the_store_keeps_a_hash_and_revokes_by_id_or_name() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path().join("devices.json"));
        let (id, secret) = s.add("Pixel").unwrap();
        let text = fs::read_to_string(dir.path().join("devices.json")).unwrap();
        assert!(!text.contains(&secret), "the secret is not stored");
        assert!(text.contains(&sha256_hex(&secret)));
        assert_eq!(
            fs::metadata(dir.path().join("devices.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(s.find(&id, &secret).is_some());
        assert!(s.find(&id, &"0".repeat(64)).is_none());
        assert!(s.find("ffffffffffffffff", &secret).is_none());
        assert!(s.revoke("Pixel").unwrap());
        assert!(!s.is_paired(&id));
        assert!(!s.revoke("Pixel").unwrap());
    }

    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn five_failures_lock_an_address_out_and_success_forgets_them() {
        let l = Limiter::default();
        for _ in 0..4 {
            l.failed("a");
        }
        assert!(l.allowed("a"));
        l.failed("a");
        assert!(!l.allowed("a"));
        assert!(l.allowed("b"), "other addresses are not affected");
        l.cleared("a");
        assert!(l.allowed("a"));
    }

    #[test]
    fn the_comparison_looks_at_every_byte_and_the_length() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
        assert!(same("", ""));
    }
}
