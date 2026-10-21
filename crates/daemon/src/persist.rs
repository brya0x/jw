//! `session.json`: the model, written whole and atomically on every structural change.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::Model;

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    #[serde(flatten)]
    model: Model,
}

pub fn to_text(model: &Model) -> String {
    let file = File {
        v: VERSION,
        model: model.clone(),
    };
    serde_json::to_string_pretty(&file).expect("the model serialises")
}

/// Write via a temporary file and a rename, so a crash never leaves half a session.
pub fn save_text(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

/// The saved model, or `None` when there is none. A file that cannot be read is moved to
/// `session.json.bad` (never overwritten silently) and reported on stderr.
pub fn load(path: &Path) -> Option<Model> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_str::<File>(&text) {
        Ok(file) if file.v == VERSION => Some(file.model),
        Ok(file) => {
            quarantine(
                path,
                &format!("it is version {}, this daemon reads {VERSION}", file.v),
            );
            None
        }
        Err(e) => {
            quarantine(path, &e.to_string());
            None
        }
    }
}

fn quarantine(path: &Path, why: &str) {
    let bad = path.with_extension("json.bad");
    eprintln!(
        "{} is unusable ({why}); moved to {}",
        path.display(),
        bad.display()
    );
    if let Err(e) = fs::rename(path, &bad) {
        eprintln!("cannot move it aside: {e}");
    }
}
