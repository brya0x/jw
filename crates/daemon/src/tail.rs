//! The last lines a shell or a monitor printed (REQ-607). The engine writes what each background
//! task prints to `<tmp>/claude-<uid>/<project>/<session>/tasks/<task>.output`; the daemon reads
//! the end of that file and keeps it as plain text. Nothing here takes a path from a client.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

/// Lines kept for a task.
pub const LINES: usize = 6;
/// A line is cut here before it travels.
const CHARS: usize = 200;
/// What is read from the end of the file.
const READ: u64 = 16 * 1024;
/// How a Bash result says where the output goes.
const SENTENCE: &str = "Output is being written to: ";
const EXT: &str = ".output";

/// The path a `run_in_background` Bash result names, when it does.
pub fn named_in(text: &str) -> Option<PathBuf> {
    let rest = &text[text.find(SENTENCE)? + SENTENCE.len()..];
    let end = rest.find(EXT)? + EXT.len();
    Some(PathBuf::from(&rest[..end]))
}

/// The file of `task` beside `known`, when `known` is a file of the same `session`: another
/// session keeps its files in another directory.
pub fn sibling(known: &Path, session: &str, task: &str) -> Option<PathBuf> {
    let tasks = known.parent()?;
    (tasks.parent()?.file_name()? == session).then(|| tasks.join(format!("{task}{EXT}")))
}

/// Where the engine keeps the file of `task` of `session`, looked for in the temporary directories
/// it writes to, when nothing told its place (a monitor's result does not).
pub fn discover(session: &str, task: &str) -> Option<PathBuf> {
    discover_in(
        &[Path::new("/private/tmp"), Path::new("/tmp")],
        session,
        task,
    )
}

fn discover_in(bases: &[&Path], session: &str, task: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\']) || task.contains(['/', '\\']) {
        return None;
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    for base in bases {
        // On macOS `/tmp` is `/private/tmp`: one directory, scanned once.
        let Ok(real) = base.canonicalize() else {
            continue;
        };
        if seen.contains(&real) {
            continue;
        }
        seen.push(real);
        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        for e in entries.flatten() {
            if !e.file_name().to_string_lossy().starts_with("claude-") {
                continue;
            }
            let Ok(projects) = std::fs::read_dir(e.path()) else {
                continue;
            };
            for p in projects.flatten() {
                let file = p
                    .path()
                    .join(session)
                    .join("tasks")
                    .join(format!("{task}{EXT}"));
                if file.is_file() {
                    return Some(file);
                }
            }
        }
    }
    None
}

/// The file only if it is what the engine writes for this task of this session: a regular file,
/// not a link, named `<task>.output` in a `tasks` directory inside the directory of `session`.
fn checked(path: &Path, session: &str, task: &str) -> Option<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    if std::fs::symlink_metadata(path)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let real = path.canonicalize().ok()?;
    let name = real.file_name()?.to_str()?;
    let tasks = real.parent()?;
    let own = tasks.parent()?.file_name()?.to_str()?;
    (name == format!("{task}{EXT}")
        && tasks.file_name()? == "tasks"
        && own == session
        && real.is_file())
    .then_some(real)
}

/// The last lines of the file of a task, or `None` when it is not there or is not what it must be.
/// `ended` says the task is over, so the engine's closing line is not one of them.
pub fn read(path: &Path, session: &str, task: &str, ended: bool) -> Option<Vec<String>> {
    let real = checked(path, session, task)?;
    let mut file = File::open(real).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(READ);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::with_capacity(READ as usize);
    file.take(READ).read_to_end(&mut bytes).ok()?;
    if from > 0 {
        // The window starts in the middle of a line, or of a progress update, or of a character:
        // what comes before the first line end goes. With no line end, only the bytes of a character
        // cut in two go.
        let cut = match bytes.iter().position(|b| matches!(b, b'\n' | b'\r')) {
            Some(i) => i + 1,
            None => bytes.iter().take_while(|b| **b & 0xC0 == 0x80).count(),
        };
        bytes.drain(..cut);
    }
    Some(lines_of(&bytes, ended))
}

/// Plain lines out of what a program printed: no escape sequences or other control characters,
/// a carriage return keeps the last state of its line, none longer than `CHARS`, the blank ones at
/// the ends gone, and at most `LINES` of them, the newest.
fn lines_of(bytes: &[u8], ended: bool) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<String> = text.split('\n').map(line).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    // What the engine adds when the process ends; a running program could print the same words.
    if ended
        && lines
            .last()
            .is_some_and(|l| l.starts_with("[exited with code ") && l.ends_with(']'))
    {
        lines.pop();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
    }
    let first = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    lines.drain(..first);
    let start = lines.len().saturating_sub(LINES);
    lines.split_off(start)
}

fn line(raw: &str) -> String {
    let state = raw
        .rsplit('\r')
        .map(plain)
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default();
    let state = state.trim_end();
    if state.chars().count() > CHARS {
        let mut cut: String = state.chars().take(CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        state.to_owned()
    }
}

fn is_intermediate(c: char) -> bool {
    ('\x20'..='\x2f').contains(&c)
}

/// `s` without the escape sequences (colours, cursor moves, titles, charsets) and the control
/// characters.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for n in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut last = '\0';
                    for n in chars.by_ref() {
                        if n == '\x07' || (last == '\x1b' && n == '\\') {
                            break;
                        }
                        last = n;
                    }
                }
                // Charset selection (`ESC ( B`): intermediate bytes, then the final one.
                Some(c) if is_intermediate(c) => {
                    for n in chars.by_ref() {
                        if !is_intermediate(n) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory like the engine's, with `task` written in it.
    fn engine_dir(name: &str, session: &str, task: &str, body: &[u8]) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("jwn-tail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root
            .join("claude-501")
            .join("-proj")
            .join(session)
            .join("tasks");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{task}.output"));
        std::fs::write(&file, body).unwrap();
        (root, file)
    }

    #[test]
    fn the_path_is_taken_from_the_sentence_of_a_real_result() {
        let real = "Command running in background with ID: b3n42ka0g. Output is being written to: /private/tmp/claude-501/-private-tmp-cap-n135/dda07519/tasks/b3n42ka0g.output. You will be notified when it completes. To check interim output, use Read on that file path.";
        let path = named_in(real).unwrap();
        assert_eq!(
            path,
            Path::new(
                "/private/tmp/claude-501/-private-tmp-cap-n135/dda07519/tasks/b3n42ka0g.output"
            )
        );
        assert_eq!(
            sibling(&path, "dda07519", "bs4dhpkq2").unwrap(),
            Path::new(
                "/private/tmp/claude-501/-private-tmp-cap-n135/dda07519/tasks/bs4dhpkq2.output"
            )
        );
        assert!(sibling(&path, "another", "bs4dhpkq2").is_none());
        assert!(named_in("Monitor started (task bs4dhpkq2, expires in 1m)").is_none());
    }

    #[test]
    fn escapes_carriage_returns_and_long_lines_become_plain_lines() {
        let mut long = "x".repeat(300);
        long.push('\n');
        let text = format!(
            "\x1b[32m   Compiling\x1b[0m jwn\n10%\r50%\r100% done\n\x1b]0;title\x07tab\there\n{long}"
        );
        let lines = lines_of(text.as_bytes(), false);
        assert_eq!(lines[0], "   Compiling jwn");
        assert_eq!(lines[1], "100% done");
        assert_eq!(lines[2], "tab here");
        assert_eq!(lines[3].chars().count(), CHARS);
        assert!(lines[3].ends_with('…'));
    }

    #[test]
    fn only_the_last_lines_stay_and_the_blank_ones_at_the_ends_go() {
        let text: String = (1..=10).map(|i| format!("l{i}\n")).collect();
        let lines = lines_of(format!("\n\n{text}\n\n").as_bytes(), false);
        assert_eq!(lines, ["l5", "l6", "l7", "l8", "l9", "l10"]);
    }

    #[test]
    fn the_closing_line_of_the_engine_goes_only_when_the_task_ended() {
        let text = b"line 1\nline 2\n\n[exited with code 1]\n";
        assert_eq!(lines_of(text, true), ["line 1", "line 2"]);
        assert_eq!(
            lines_of(text, false).last().unwrap(),
            "[exited with code 1]"
        );
    }

    #[test]
    fn what_a_running_shell_wrote_is_read_from_the_file_of_the_engine() {
        let (root, file) = engine_dir("read", "sess1", "bg1", b"one\ntwo\nthree\n");
        assert_eq!(
            read(&file, "sess1", "bg1", false).unwrap(),
            ["one", "two", "three"]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_that_is_not_the_one_of_this_task_is_not_read() {
        let (root, file) = engine_dir("safe", "sess1", "bg1", b"secret\n");
        // Another session, another task, a path with `..`, a relative path, a link.
        assert!(read(&file, "sess2", "bg1", false).is_none());
        assert!(read(&file, "sess1", "bg2", false).is_none());
        let dotted = file
            .parent()
            .unwrap()
            .join("..")
            .join("tasks")
            .join("bg1.output");
        assert!(read(&dotted, "sess1", "bg1", false).is_none());
        assert!(read(Path::new("tasks/bg1.output"), "sess1", "bg1", false).is_none());
        let link = file.parent().unwrap().join("bg3.output");
        std::os::unix::fs::symlink("/etc/hosts", &link).unwrap();
        assert!(read(&link, "sess1", "bg3", false).is_none());
        // A directory with the right name.
        let dir = file.parent().unwrap().join("bg4.output");
        std::fs::create_dir(&dir).unwrap();
        assert!(read(&dir, "sess1", "bg4", false).is_none());
        // The same name outside a `tasks` directory.
        let loose = root.join("bg1.output");
        std::fs::write(&loose, b"x").unwrap();
        assert!(read(&loose, "sess1", "bg1", false).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_progress_stream_with_no_line_end_keeps_its_last_state() {
        let body: String = (0..2000).map(|i| format!("progress {i}\r")).collect();
        assert!(body.len() > READ as usize);
        let (root, file) = engine_dir("cr", "sess1", "bg1", body.as_bytes());
        assert_eq!(
            read(&file, "sess1", "bg1", false).unwrap(),
            ["progress 1999"]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_character_cut_at_the_start_of_the_window_is_dropped_whole() {
        // '€' is three bytes and 7000 of them are 21000 bytes: the window starts on the third byte
        // of a character, with no line end anywhere.
        let body: String = "€".repeat(7000);
        let (root, file) = engine_dir("utf8", "sess1", "bg1", body.as_bytes());
        let lines = read(&file, "sess1", "bg1", false).unwrap();
        assert_eq!(lines, ["€".repeat(CHARS - 1) + "…"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_charset_selection_leaves_no_letter_behind() {
        assert_eq!(lines_of(b"done\x1b(B\x1b[m\n", false), ["done"]);
        assert_eq!(lines_of(b"a\x1b7b\n", false), ["ab"]);
    }

    #[test]
    fn a_big_file_is_read_from_its_end_and_its_cut_first_line_is_dropped() {
        let mut body = Vec::new();
        for i in 0..5000 {
            body.extend_from_slice(format!("line number {i}\n").as_bytes());
        }
        let (root, file) = engine_dir("big", "sess1", "bg1", &body);
        let lines = read(&file, "sess1", "bg1", false).unwrap();
        assert_eq!(lines.last().unwrap(), "line number 4999");
        assert_eq!(lines.len(), LINES);
        assert!(lines.iter().all(|l| l.starts_with("line number ")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_task_the_result_did_not_place_is_found_by_its_session_and_id() {
        let (root, file) = engine_dir("find", "sess9", "mon1", b"a\n");
        assert_eq!(
            discover_in(&[root.as_path()], "sess9", "mon1").unwrap(),
            file
        );
        assert!(discover_in(&[root.as_path()], "sess9", "mon2").is_none());
        assert!(discover_in(&[root.as_path()], "../x", "mon1").is_none());
        assert!(discover_in(&[root.as_path()], "", "mon1").is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
