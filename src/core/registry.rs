//! The list of jw worktrees in ~/.local/state/jw/registry.json.
//! Port of internal/core/registry/registry.go: both binaries read and write
//! the same file until the cutover (REQ-1, REQ-2).

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// One worktree managed by jw. Field order and `omitempty` match the Go
/// struct, so a file Rust saves is byte for byte what Go would write.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub slot: u32,
    /// Empty when the tab is closed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tab: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pr: u64,
    /// An agent ran here before: resume, don't start fresh.
    #[serde(default, skip_serializing_if = "is_false")]
    pub opened: bool,
    /// The branch existed before jw: never jw's to delete.
    #[serde(default, skip_serializing_if = "is_false")]
    pub adopted: bool,
    /// The branch jw created, kept once the worktree switched to another one.
    #[serde(
        rename = "original_branch",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub original: String,
    /// RFC 3339 as Go writes it, kept as text so a round trip doesn't
    /// change its precision or offset.
    #[serde(default = "zero_time")]
    pub created: String,
}

/// The whole file on disk.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    /// Go writes `null` for an empty registry; both read either.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub entries: Vec<Entry>,
}

/// Honours XDG_STATE_HOME, falling back to ~/.local/state.
pub fn default_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("registry.json"))
}

/// jw's state directory: registry, session, socket fallback.
pub fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir).join("jw"));
    }
    Ok(super::config::home()?
        .join(".local")
        .join("state")
        .join("jw"))
}

impl Registry {
    /// A missing file is not an error: it is an empty registry.
    pub fn load(path: &Path) -> Result<Self> {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| path.display().to_string()),
        };
        serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))
    }

    /// Writes to a temp file and renames it, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let data = serde_json::to_vec_pretty(self)?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Looks up an entry of a project by name or by id prefix (4+ chars).
    pub fn find(&self, project: &str, key: &str) -> Result<&Entry> {
        let mut found = None;
        for e in self.entries.iter().filter(|e| e.project == project) {
            if e.name == key {
                return Ok(e);
            }
            if key.len() >= 4 && e.id.starts_with(key) {
                if found.is_some() {
                    bail!("id prefix {key:?} is ambiguous");
                }
                found = Some(e);
            }
        }
        found.ok_or_else(|| anyhow!("no worktree {key:?} in {project}"))
    }

    /// Whether `project` already has a worktree with exactly this name.
    pub fn has(&self, project: &str, name: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.project == project && e.name == name)
    }

    pub fn add(&mut self, e: Entry) {
        self.entries.push(e);
    }

    /// Deletes the entry with that id, freeing its slot.
    pub fn remove(&mut self, id: &str) {
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            self.entries.remove(i);
        }
    }

    /// The lowest slot not used by any project.
    pub fn next_slot(&self) -> u32 {
        (1..)
            .find(|s| !self.entries.iter().any(|e| e.slot == *s))
            .expect("slots are unbounded")
    }
}

/// A random RFC 4122 version 4 UUID.
pub fn new_id() -> Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

/// Now in UTC as Go's `time.Now().UTC()` marshals it:
/// `2026-10-08T22:01:02.123456789Z`, trailing zeros of the fraction dropped.
pub fn now_rfc3339() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    rfc3339(d.as_secs() as i64, d.subsec_nanos())
}

fn rfc3339(secs: i64, nanos: u32) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let frac = if nanos == 0 {
        String::new()
    } else {
        format!(".{nanos:09}").trim_end_matches('0').to_string()
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{frac}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Go's zero `time.Time`, which it writes for an entry without a date.
fn zero_time() -> String {
    "0001-01-01T00:00:00Z".into()
}

fn null_is_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Entry>, D::Error> {
    Ok(Option::<Vec<Entry>>::deserialize(d)?.unwrap_or_default())
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, slot: u32) -> Entry {
        Entry {
            id: id.into(),
            name: name.into(),
            project: "myapp".into(),
            slot,
            ..Entry::default()
        }
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let r = Registry::load(&dir.path().join("nope.json")).unwrap();
        assert!(r.entries.is_empty());
    }

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jw").join("registry.json");
        let mut r = Registry::default();
        r.add(entry(&new_id().unwrap(), "web", 1));
        r.save(&path).unwrap();
        assert_eq!(Registry::load(&path).unwrap(), r);
        assert!(!dir.path().join("jw/registry.json.tmp").exists());
    }

    /// The fixture was written by the Go binary (testdata/README.md): Rust
    /// reads it and saves exactly the same bytes, so Go reads Rust's file.
    #[test]
    fn round_trips_the_go_fixture_byte_for_byte() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/registry.go.json");
        let r = Registry::load(&fixture).unwrap();
        assert_eq!(r.entries.len(), 3);
        assert_eq!(r.entries[1].original, "feat/old-name");
        assert!(r.entries[2].adopted && r.entries[2].opened);

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("registry.json");
        r.save(&out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out).unwrap(),
            std::fs::read_to_string(fixture).unwrap()
        );
    }

    #[test]
    fn null_entries_is_empty() {
        let r: Registry = serde_json::from_str(r#"{"entries": null}"#).unwrap();
        assert!(r.entries.is_empty());
    }

    #[test]
    fn next_slot_fills_gaps() {
        let r = Registry {
            entries: vec![entry("a", "a", 1), entry("b", "b", 3)],
        };
        assert_eq!(r.next_slot(), 2);
    }

    #[test]
    fn remove_frees_slot() {
        let mut r = Registry {
            entries: vec![entry("a", "a", 1), entry("b", "b", 2)],
        };
        r.remove("a");
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.next_slot(), 1);
    }

    #[test]
    fn find_by_name_and_prefix() {
        let r = Registry {
            entries: vec![
                entry("0b1c9e2a-0000-4000-8000-000000000000", "web", 1),
                entry("7f3ad011-0000-4000-8000-000000000000", "api", 2),
            ],
        };
        assert_eq!(r.find("myapp", "api").unwrap().name, "api");
        assert_eq!(r.find("myapp", "0b1c").unwrap().name, "web");
        assert!(r.find("myapp", "0b1").is_err(), "prefix shorter than 4");
        assert!(r.find("other", "web").is_err(), "names are per project");
        assert!(r.has("myapp", "web") && !r.has("myapp", "we"));
    }

    #[test]
    fn timestamps_look_like_go() {
        assert_eq!(rfc3339(0, 0), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(1_787_270_399, 500_000_000),
            "2026-08-20T23:59:59.5Z"
        );
        assert_eq!(
            rfc3339(951_782_400, 123_456_789),
            "2000-02-29T00:00:00.123456789Z"
        );
        assert!(now_rfc3339().ends_with('Z'));
    }

    #[test]
    fn new_id_is_v4() {
        let id = new_id().unwrap();
        let b = id.as_bytes();
        assert_eq!(id.len(), 36);
        assert_eq!(b[14], b'4');
        assert!(matches!(b[19], b'8' | b'9' | b'a' | b'b'), "{id}");
    }
}
