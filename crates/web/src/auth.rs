//! What lets a phone in: a pairing token, the devices that paired, and the
//! limit on failed attempts (mobile spec, REQ-2, 3, 5, 6).
//!
//! Secrets are 128 or 256 random bits, so a plain SHA-256 is enough to store
//! them: there is nothing to guess and no reason to be slow.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `bytes` random bytes from the system, as lowercase hex.
pub fn random_hex(bytes: usize) -> io::Result<String> {
    let mut buf = vec![0u8; bytes];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(hex(&buf))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the store keeps of a secret.
pub fn hash(secret: &str) -> String {
    hex(&Sha256::digest(secret.as_bytes()))
}

/// Equal without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The one-time token a QR carries (REQ-2).
pub struct Pairing {
    token: String,
    born: Instant,
    used: bool,
}

/// Why a pairing token was not taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not the token the bridge handed out.
    Unknown,
    /// It was used, or it is older than its lifetime.
    Expired,
}

impl Pairing {
    pub fn new(now: Instant) -> io::Result<Self> {
        Ok(Self {
            token: random_hex(16)?,
            born: now,
            used: false,
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// How long the token still has, or `None` when it was used or is too old.
    pub fn left(&self, ttl: Duration, now: Instant) -> Option<Duration> {
        if self.used {
            return None;
        }
        ttl.checked_sub(now.duration_since(self.born))
            .filter(|d| !d.is_zero())
    }

    /// Spends the token. It is spent the moment a right one arrives, before
    /// anyone is asked anything, so only one request can ever be pending.
    pub fn take(&mut self, presented: &str, ttl: Duration, now: Instant) -> Result<(), Refused> {
        if !same(&self.token, presented) {
            return Err(Refused::Unknown);
        }
        if self.used || now.duration_since(self.born) > ttl {
            return Err(Refused::Expired);
        }
        self.used = true;
        Ok(())
    }
}

/// A phone that paired. Only the hash of its token is kept.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub hash: String,
    pub created: u64,
    pub last_seen: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Devices {
    devices: Vec<Device>,
}

/// `mobile.json`, written by the bridge (a device pairs, one is seen) and by
/// `jw mobile revoke` in another process, so every change takes a lock on a
/// file next to it and reads the file again under it.
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn open(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn devices(&self) -> io::Result<Vec<Device>> {
        Ok(self.read()?.devices)
    }

    pub fn find(&self, token_hash: &str) -> io::Result<Option<Device>> {
        Ok(self
            .read()?
            .devices
            .into_iter()
            .find(|d| same(&d.hash, token_hash)))
    }

    pub fn add(&self, name: &str, token_hash: &str) -> io::Result<Device> {
        let id = random_hex(4)?;
        let device = Device {
            id,
            name: name.to_string(),
            hash: token_hash.to_string(),
            created: now_secs(),
            last_seen: now_secs(),
        };
        self.update(|ds| ds.push(device.clone()))?;
        Ok(device)
    }

    /// Removes the device with this id, or the one with this name. `None`
    /// when there is none; an error when a name is shared.
    pub fn revoke(&self, id_or_name: &str) -> io::Result<Option<Device>> {
        self.update(|ds| {
            let at = match ds.iter().position(|d| d.id == id_or_name) {
                Some(i) => i,
                None => {
                    let named: Vec<usize> = (0..ds.len())
                        .filter(|&i| ds[i].name == id_or_name)
                        .collect();
                    match named.as_slice() {
                        [] => return Ok(None),
                        [i] => *i,
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                format!(
                                    "{} devices are called {id_or_name:?}; use an id",
                                    named.len()
                                ),
                            ));
                        }
                    }
                }
            };
            Ok(Some(ds.remove(at)))
        })?
    }

    pub fn touch(&self, id: &str) -> io::Result<()> {
        self.update(|ds| {
            if let Some(d) = ds.iter_mut().find(|d| d.id == id) {
                d.last_seen = now_secs();
            }
        })
    }

    /// A file next to the store with `suffix` after its whole name.
    fn beside(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    }

    fn read(&self) -> io::Result<Devices> {
        match fs::read(&self.path) {
            Ok(data) => Ok(serde_json::from_slice(&data)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Devices::default()),
            Err(e) => Err(e),
        }
    }

    /// Read, change, write, all under the lock. The file is written whole
    /// beside itself with mode 0600 and renamed over, so a reader never sees
    /// half of it.
    fn update<T>(&self, change: impl FnOnce(&mut Vec<Device>) -> T) -> io::Result<T> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(self.beside(".lock"))?;
        // SAFETY: `lock` is an open file; the lock goes when it is dropped.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut file = self.read()?;
        let out = change(&mut file.devices);
        let tmp = self.beside(&format!(".tmp.{}", std::process::id()));
        write_private(&tmp, &serde_json::to_vec_pretty(&file)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(out)
    }
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let _ = fs::remove_file(path);
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Failed attempts per address (REQ-5): `max` inside `window` locks the
/// address out for `lockout`.
pub struct Limiter {
    max: usize,
    window: Duration,
    lockout: Duration,
    fails: HashMap<String, VecDeque<Instant>>,
    locked: HashMap<String, Instant>,
}

/// More addresses than this are kept only while they are locked or recent.
const TRACKED: usize = 4096;

impl Limiter {
    pub fn new(max: usize, window: Duration, lockout: Duration) -> Self {
        Self {
            max,
            window,
            lockout,
            fails: HashMap::new(),
            locked: HashMap::new(),
        }
    }

    /// Whether `key` is locked out right now.
    pub fn locked(&mut self, key: &str, now: Instant) -> bool {
        match self.locked.get(key) {
            Some(&until) if now < until => true,
            Some(_) => {
                self.locked.remove(key);
                false
            }
            None => false,
        }
    }

    pub fn failed(&mut self, key: &str, now: Instant) {
        if self.fails.len() >= TRACKED {
            let window = self.window;
            self.fails
                .retain(|_, v| v.back().is_some_and(|&t| now.duration_since(t) < window));
            self.locked.retain(|_, &mut until| now < until);
        }
        let recent = self.fails.entry(key.to_string()).or_default();
        while recent
            .front()
            .is_some_and(|&t| now.duration_since(t) >= self.window)
        {
            recent.pop_front();
        }
        recent.push_back(now);
        if recent.len() >= self.max {
            recent.clear();
            self.locked.insert(key.to_string(), now + self.lockout);
        }
    }

    /// A good attempt forgets the bad ones before it.
    pub fn succeeded(&mut self, key: &str) {
        self.fails.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn secrets_are_random_hex_of_the_right_size() {
        let a = random_hex(16).unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, random_hex(16).unwrap());
        assert_eq!(hash("a").len(), 64);
        assert_ne!(hash("a"), hash("b"));
    }

    #[test]
    fn a_pairing_token_works_once() {
        let t0 = Instant::now();
        let mut p = Pairing::new(t0).unwrap();
        let token = p.token().to_string();
        assert_eq!(p.take("nope", secs(120), t0), Err(Refused::Unknown));
        assert_eq!(p.take(&token, secs(120), t0), Ok(()));
        assert_eq!(p.take(&token, secs(120), t0), Err(Refused::Expired));
    }

    #[test]
    fn a_pairing_token_expires() {
        let t0 = Instant::now();
        let mut p = Pairing::new(t0).unwrap();
        let token = p.token().to_string();
        assert_eq!(
            p.take(&token, secs(120), t0 + secs(121)),
            Err(Refused::Expired)
        );
        let mut p = Pairing::new(t0).unwrap();
        let token = p.token().to_string();
        assert_eq!(p.take(&token, secs(120), t0 + secs(119)), Ok(()));
    }

    #[test]
    fn the_store_keeps_hashes_and_finds_by_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("mobile.json"));
        assert!(store.devices().unwrap().is_empty());
        let secret = random_hex(32).unwrap();
        let d = store.add("Pixel 8", &hash(&secret)).unwrap();
        assert_eq!(store.find(&hash(&secret)).unwrap(), Some(d));
        assert_eq!(store.find(&hash("other")).unwrap(), None);
        let text = fs::read_to_string(dir.path().join("mobile.json")).unwrap();
        assert!(!text.contains(&secret), "the secret itself was written");
    }

    #[test]
    fn the_store_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mobile.json");
        Store::open(path.clone()).add("a", &hash("x")).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn revoke_by_id_by_name_and_when_a_name_is_shared() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("mobile.json"));
        let a = store.add("Pixel 8", &hash("a")).unwrap();
        store.add("Tablet", &hash("b")).unwrap();
        assert_eq!(store.revoke(&a.id).unwrap(), Some(a));
        assert_eq!(store.revoke("Tablet").unwrap().unwrap().name, "Tablet");
        assert_eq!(store.revoke("Tablet").unwrap(), None);
        store.add("Same", &hash("c")).unwrap();
        store.add("Same", &hash("d")).unwrap();
        assert!(store.revoke("Same").is_err());
        assert_eq!(store.devices().unwrap().len(), 2);
    }

    #[test]
    fn two_writers_do_not_lose_each_others_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mobile.json");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let store = Store::open(path.clone());
                std::thread::spawn(move || {
                    for j in 0..10 {
                        store
                            .add(&format!("d{i}-{j}"), &hash(&format!("{i}-{j}")))
                            .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(Store::open(path).devices().unwrap().len(), 80);
    }

    #[test]
    fn five_failures_in_a_minute_lock_an_address_for_five() {
        let t0 = Instant::now();
        let mut l = Limiter::new(5, secs(60), secs(300));
        for i in 0..4 {
            l.failed("1.2.3.4", t0 + secs(i));
            assert!(!l.locked("1.2.3.4", t0 + secs(i)));
        }
        l.failed("1.2.3.4", t0 + secs(4));
        assert!(l.locked("1.2.3.4", t0 + secs(5)));
        assert!(
            !l.locked("5.6.7.8", t0 + secs(5)),
            "another address is free"
        );
        assert!(l.locked("1.2.3.4", t0 + secs(300)));
        assert!(!l.locked("1.2.3.4", t0 + secs(305)));
    }

    #[test]
    fn slow_failures_and_a_success_do_not_lock() {
        let t0 = Instant::now();
        let mut l = Limiter::new(3, secs(60), secs(300));
        for i in 0..10 {
            l.failed("a", t0 + secs(i * 61));
            assert!(!l.locked("a", t0 + secs(i * 61)));
        }
        l.failed("b", t0);
        l.failed("b", t0);
        l.succeeded("b");
        l.failed("b", t0);
        assert!(!l.locked("b", t0));
    }
}
