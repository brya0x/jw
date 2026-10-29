//! Copies the identity file that this build compiles in (REQ-1078): the file `IDENTITY_FILE` names
//! when it is set and not empty, else the root `identity.toml`. `lib.rs` includes the copy.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=IDENTITY_FILE");
    let file = match std::env::var("IDENTITY_FILE") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"))
            .join("../../identity.toml"),
    };
    println!("cargo:rerun-if-changed={}", file.display());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it")).join("identity.toml");
    std::fs::copy(&file, &out)
        .unwrap_or_else(|e| panic!("the identity file {}: {e}", file.display()));
}
