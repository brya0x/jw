//! N137: a text file of a workspace as lines, and what is refused (REQ-610 to REQ-614, REQ-620).

mod common;

use std::path::{Path, PathBuf};

use common::{Sandbox, text};
use contract::text::TextDoc;
use contract::{Cmd, ErrorCode};

fn open(c: &mut wire::Client, dir: &Path) -> String {
    c.cmd(Cmd::OpenFolder { path: text(dir) })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn read(c: &mut wire::Client, ws: &str, path: &str) -> Result<TextDoc, (ErrorCode, String)> {
    match c
        .cmd(Cmd::ReadFile {
            workspace: ws.into(),
            path: path.into(),
        })
        .unwrap()
    {
        Ok(v) => Ok(serde_json::from_value(v.unwrap()).unwrap()),
        Err(e) => Err((e.code, e.message)),
    }
}

fn app(sb: &Sandbox) -> PathBuf {
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("app");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn plain(doc: &TextDoc) -> Vec<String> {
    doc.lines
        .iter()
        .map(|l| l.iter().map(|r| r.t.as_str()).collect())
        .collect()
}

#[test]
fn a_text_file_comes_as_lines_and_what_leaves_the_workspace_or_is_not_text_is_refused() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = app(&sb);
    std::fs::write(
        dir.join("src/main.rs"),
        "fn main() {\r\n    println!(\"hola\");\r\n\r\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("notas.txt"), "sin salto final").unwrap();
    std::fs::write(dir.join("vacio.txt"), "").unwrap();
    std::fs::write(dir.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
    std::fs::write(dir.join("latin1.txt"), b"caf\xe9\n").unwrap();
    let outside = dir.parent().unwrap().join("fuera.txt");
    std::fs::write(&outside, "fuera\n").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("enlace.txt")).unwrap();
    let mut c = sb.client();
    let ws = open(&mut c, &dir);

    let doc = read(&mut c, &ws, "src/main.rs").unwrap();
    assert_eq!(doc.title, "main.rs");
    assert_eq!(doc.path, text(&dir.join("src/main.rs")));
    assert_eq!(
        doc.size,
        std::fs::metadata(dir.join("src/main.rs")).unwrap().len(),
        "the weight is the file's"
    );
    assert!(!doc.truncated);
    assert_eq!(
        plain(&doc),
        ["fn main() {", "    println!(\"hola\");", "", "}"],
        "CRLF loses its \\r and a line end ends a line, not starts another"
    );
    assert!(doc.lines[2].is_empty(), "an empty line has no runs");
    assert_eq!(
        read(&mut c, &ws, &text(&dir.join("src/main.rs")))
            .unwrap()
            .title,
        "main.rs",
        "an absolute path inside the workspace is the same file"
    );
    assert_eq!(
        plain(&read(&mut c, &ws, "notas.txt").unwrap()),
        ["sin salto final"]
    );
    assert!(read(&mut c, &ws, "vacio.txt").unwrap().lines.is_empty());

    // refused: not text, outside, a link out, a folder, missing
    assert_eq!(
        read(&mut c, &ws, "logo.png").unwrap_err().0,
        ErrorCode::Binary
    );
    assert_eq!(
        read(&mut c, &ws, "latin1.txt").unwrap_err().0,
        ErrorCode::Binary
    );
    assert_eq!(
        read(&mut c, &ws, "../fuera.txt").unwrap_err().0,
        ErrorCode::BadRequest
    );
    assert_eq!(
        read(&mut c, &ws, &text(&outside)).unwrap_err().0,
        ErrorCode::BadRequest
    );
    assert_eq!(
        read(&mut c, &ws, "enlace.txt").unwrap_err().0,
        ErrorCode::BadRequest,
        "a link that leaves the workspace is not followed"
    );
    assert_eq!(
        read(&mut c, &ws, "src").unwrap_err().0,
        ErrorCode::BadRequest
    );
    assert_eq!(
        read(&mut c, &ws, "no-hay.txt").unwrap_err().0,
        ErrorCode::NotFound
    );
    d.stop();
}

#[test]
fn a_big_file_is_cut_at_a_line_end_a_long_line_is_cut_and_nothing_is_written() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = app(&sb);
    let line = "x".repeat(99);
    let big = format!("{line}\n").repeat(3 * 1024 * 1024 / 100);
    std::fs::write(dir.join("app.log"), &big).unwrap();
    std::fs::write(dir.join("una-linea.js"), "é".repeat(3 * 1024 * 1024 / 2)).unwrap();
    std::fs::write(
        dir.join("ancha.txt"),
        format!("{}\nfin\n", "a".repeat(5000)),
    )
    .unwrap();
    let before = std::fs::read(dir.join("ancha.txt")).unwrap();
    let mut c = sb.client();
    let ws = open(&mut c, &dir);

    let doc = read(&mut c, &ws, "app.log").unwrap();
    assert!(doc.truncated);
    assert_eq!(doc.size, big.len() as u64);
    assert!(
        plain(&doc).iter().all(|l| l == &line),
        "every line is whole: it was cut at a line end"
    );
    assert!(doc.lines.len() * 100 <= 1024 * 1024);

    let one = read(&mut c, &ws, "una-linea.js").unwrap();
    assert!(one.truncated);
    assert_eq!(one.lines.len(), 1);
    assert_eq!(
        one.lines[0][0].t.chars().count(),
        2001,
        "a line keeps 2000 characters and an ellipsis; the cut fell on a whole character"
    );

    let wide = read(&mut c, &ws, "ancha.txt").unwrap();
    let first = &plain(&wide)[0];
    assert!(first.ends_with('…') && first.chars().count() == 2001);
    assert_eq!(plain(&wide)[1], "fin");
    assert_eq!(std::fs::read(dir.join("ancha.txt")).unwrap(), before);
    d.stop();
}

#[test]
fn a_file_of_a_known_language_comes_coloured_its_head_only_when_it_is_big_and_plain_when_unknown() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = app(&sb);
    std::fs::write(
        dir.join("src/lib.rs"),
        "// uno\nfn dos() { let t = \"tres\"; }\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.zzz"), "fn dos() { let t = \"tres\"; }\n").unwrap();
    let big = "let valor = 12345; // x\n".repeat(200 * 1024 / 23);
    std::fs::write(dir.join("src/grande.rs"), &big).unwrap();
    let mut c = sb.client();
    let ws = open(&mut c, &dir);

    let doc = read(&mut c, &ws, "src/lib.rs").unwrap();
    let classes: Vec<(&str, &str)> = doc
        .lines
        .iter()
        .flatten()
        .filter_map(|r| r.k.as_deref().map(|k| (r.t.as_str(), k)))
        .collect();
    assert!(classes.contains(&("// uno", "cm")), "{classes:?}");
    assert!(classes.contains(&("fn", "kw")), "{classes:?}");
    assert!(
        classes
            .iter()
            .any(|(t, k)| t.contains("tres") && *k == "str")
    );
    assert_eq!(plain(&doc), ["// uno", "fn dos() { let t = \"tres\"; }"]);

    let unknown = read(&mut c, &ws, "src/lib.zzz").unwrap();
    assert!(unknown.lines.iter().flatten().all(|r| r.k.is_none()));

    let started = std::time::Instant::now();
    let doc = read(&mut c, &ws, "src/grande.rs").unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(
        doc.lines[0].iter().any(|r| r.k.is_some()),
        "the head is coloured"
    );
    assert!(
        doc.lines.last().unwrap().iter().all(|r| r.k.is_none()),
        "the tail past 64 KiB is plain"
    );
    d.stop();
}
