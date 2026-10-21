//! `^V` in a conversation: the picture the machine's clipboard holds, made small before it goes
//! to the daemon (spec REQ-407). The terminal cannot be asked for it (OSC 52 only writes), so the
//! machine's own tools are: `osascript` and `sips` on macOS, `wl-paste` or `xclip` and
//! ImageMagick elsewhere. Nothing is read from a machine that is not the one running the client
//! (over ssh there is no clipboard to read). `JWN_CLIPBOARD_IMAGE` names a file to take instead,
//! which is what the tests and a person without these tools use.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use contract::conversation::Image;

use super::shell::base64;

/// What the long side of a picture is made at most, in pixels.
const LONG_SIDE: &str = "1568";
/// What a picture weighs at most once it is small: the daemon refuses more than 5 MiB.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// The picture on the clipboard, a JPEG of at most `LONG_SIDE` pixels (or the file as it is when
/// the machine has nothing to make it small with and it is already small enough).
pub fn image() -> Result<Image, String> {
    image_from(std::env::var_os(identity::env_var("CLIPBOARD_IMAGE")).map(PathBuf::from))
}

/// The picture of `file` when there is one, else the clipboard's.
fn image_from(file: Option<PathBuf>) -> Result<Image, String> {
    let work = Work::new()?;
    let source = work.file("in");
    match file {
        Some(path) => {
            std::fs::copy(&path, &source).map_err(|e| format!("no se pudo leer la imagen: {e}"))?;
        }
        None => from_clipboard(&source)?,
    }
    let out = work.file("out");
    let bytes = if shrink(&source, &out) {
        std::fs::read(&out)
    } else {
        std::fs::read(&source)
    }
    .map_err(|e| format!("no se pudo leer la imagen: {e}"))?;
    if bytes.is_empty() {
        return Err("no hay una imagen en el portapapeles".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err("la imagen pesa demasiado: no hay con qué reducirla".into());
    }
    let media_type = if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else {
        return Err("el portapapeles no tiene una imagen que se pueda enviar".into());
    };
    Ok(Image {
        media_type: media_type.into(),
        data: base64(&bytes),
    })
}

/// A folder of this call's own, gone when it ends.
struct Work(PathBuf);

impl Work {
    fn new() -> Result<Work, String> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "{}-clip-{}-{}",
            identity::get().name,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("no se pudo leer el portapapeles: {e}"))?;
        Ok(Work(dir))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const NONE: &str = "no hay una imagen en el portapapeles";

fn from_clipboard(to: &Path) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        // The path is inside an AppleScript string.
        let path = to
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let script = format!(
            "try\nset d to the clipboard as «class PNGf»\nset f to open for access POSIX file \"{}\" with write permission\nset eof f to 0\nwrite d to f\nclose access f\nreturn \"ok\"\non error\nreturn \"none\"\nend try",
            path
        );
        let out = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("no se pudo leer el portapapeles: {e}"))?;
        return if String::from_utf8_lossy(&out.stdout).trim() == "ok" {
            Ok(())
        } else {
            Err(NONE.into())
        };
    }
    let tries: [(&str, &[&str]); 2] = [
        ("wl-paste", &["--type", "image/png"]),
        (
            "xclip",
            &["-selection", "clipboard", "-t", "image/png", "-o"],
        ),
    ];
    for (program, args) in tries {
        let Ok(out) = Command::new(program)
            .args(args)
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };
        if out.status.success() && !out.stdout.is_empty() {
            return std::fs::write(to, out.stdout).map_err(|e| e.to_string());
        }
    }
    Err(NONE.into())
}

/// Make `from` a JPEG of at most `LONG_SIDE` pixels at `to`; false when the machine cannot.
fn shrink(from: &Path, to: &Path) -> bool {
    let long = format!("{LONG_SIDE}x{LONG_SIDE}>");
    let tries: Vec<(&str, Vec<String>)> = if cfg!(target_os = "macos") {
        vec![(
            "sips",
            vec![
                "-Z".into(),
                LONG_SIDE.into(),
                "-s".into(),
                "format".into(),
                "jpeg".into(),
                from.display().to_string(),
                "--out".into(),
                to.display().to_string(),
            ],
        )]
    } else {
        ["magick", "convert"]
            .into_iter()
            .map(|p| {
                (
                    p,
                    vec![
                        from.display().to_string(),
                        "-resize".into(),
                        long.clone(),
                        "-quality".into(),
                        "85".into(),
                        format!("jpeg:{}", to.display()),
                    ],
                )
            })
            .collect()
    };
    tries.into_iter().any(|(program, args)| {
        Command::new(program)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && to.exists()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn scratch(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{}-clip-test-{}-{name}",
            identity::get().name,
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("picture");
        std::fs::write(&file, bytes).unwrap();
        file
    }

    #[test]
    fn a_picture_comes_out_as_a_jpeg_or_a_png_with_its_data_in_base64() {
        let file = scratch("ok", PNG);
        let image = image_from(Some(file.clone())).unwrap();
        assert!(
            ["image/jpeg", "image/png"].contains(&image.media_type.as_str()),
            "{}",
            image.media_type
        );
        assert!(image.data.len() > 8 && !image.data.contains('\n'));
        if cfg!(target_os = "macos") {
            assert_eq!(image.media_type, "image/jpeg", "sips makes it a JPEG");
        }
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn what_is_not_a_picture_or_is_not_there_is_said() {
        let file = scratch("bad", b"hola");
        let err = image_from(Some(file.clone())).unwrap_err();
        assert!(err.contains("no tiene una imagen"), "{err}");
        assert!(
            image_from(Some(file.with_file_name("nada")))
                .unwrap_err()
                .contains("no se pudo leer"),
        );
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn the_folder_of_a_call_is_removed_when_it_ends() {
        let work = Work::new().unwrap();
        let dir = work.0.clone();
        std::fs::write(work.file("in"), b"x").unwrap();
        assert!(dir.exists());
        drop(work);
        assert!(!dir.exists());
    }
}
