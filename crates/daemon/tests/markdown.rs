//! N88: a Markdown file of a workspace as blocks, and what is refused (REQ-465, REQ-466).

mod common;

use std::path::{Path, PathBuf};

use common::{Sandbox, text};
use contract::markdown::{MarkdownDoc, MdBlock};
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

fn read(c: &mut wire::Client, ws: &str, path: &str) -> Result<MarkdownDoc, (ErrorCode, String)> {
    match c
        .cmd(Cmd::ReadMarkdown {
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
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    dir
}

#[test]
fn a_file_of_the_workspace_comes_as_blocks_with_a_title_and_its_path_and_a_link_out_is_refused() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = app(&sb);
    std::fs::write(
        dir.join("docs/plan.md"),
        "# Plan **uno**\n\nTexto con `código` y [un enlace](docs/x.md).\n\n- [x] hecho\n- [ ] falta\n",
    )
    .unwrap();
    std::fs::write(dir.join("README.md"), "sin título\n").unwrap();
    std::fs::write(dir.join(".env"), "SECRETO=1\n").unwrap();
    std::fs::write(dir.join("notas.txt"), "no es markdown\n").unwrap();
    // a file outside the workspace that a link inside it leads to
    let outside = dir.parent().unwrap().join("fuera.md");
    std::fs::write(&outside, "# Fuera\n").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("enlace.md")).unwrap();
    let mut c = sb.client();
    let ws = open(&mut c, &dir);

    let doc = read(&mut c, &ws, "docs/plan.md").unwrap();
    assert_eq!(
        doc.title, "Plan uno",
        "the first heading, without its marks"
    );
    assert_eq!(doc.path, Some(text(&dir.join("docs/plan.md"))));
    assert!(
        matches!(&doc.blocks[0], MdBlock::Heading { level: 1, text } if text.len() == 2 && text[1].b)
    );
    let MdBlock::Para { text: runs } = &doc.blocks[1] else {
        panic!("{:?}", doc.blocks[1]);
    };
    assert!(
        runs[1].code && runs[3].href.as_deref() == Some("docs/x.md"),
        "{runs:?}"
    );
    let MdBlock::List { items, .. } = &doc.blocks[2] else {
        panic!("{:?}", doc.blocks[2]);
    };
    assert_eq!(
        items.iter().map(|i| i.task).collect::<Vec<_>>(),
        [Some(true), Some(false)]
    );

    // no heading: the name of the file; an absolute path inside the workspace is the same file
    assert_eq!(read(&mut c, &ws, "README.md").unwrap().title, "README.md");
    assert_eq!(
        read(&mut c, &ws, &text(&dir.join("README.md")))
            .unwrap()
            .title,
        "README.md"
    );

    // REQ-466
    let refused = |c: &mut wire::Client, path: &str| read(c, &ws, path).unwrap_err();
    assert_eq!(
        refused(&mut c, ".env").0,
        ErrorCode::BadRequest,
        "not Markdown"
    );
    assert!(
        refused(&mut c, "notas.txt")
            .1
            .contains("not a Markdown file")
    );
    assert!(
        refused(&mut c, "../fuera.md").1.contains("outside"),
        "up and out"
    );
    assert!(
        refused(&mut c, "enlace.md").1.contains("outside"),
        "a link out"
    );
    assert!(
        refused(&mut c, &text(&outside)).1.contains("outside"),
        "absolute and out"
    );
    assert_eq!(refused(&mut c, "nada.md").0, ErrorCode::NotFound);
    assert!(refused(&mut c, "docs").1.contains("not a Markdown file"));
    d.stop();
}

#[test]
fn a_file_that_is_not_text_or_weighs_too_much_is_refused_without_reading_it() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = app(&sb);
    std::fs::write(dir.join("binario.md"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
    std::fs::write(dir.join("grande.md"), "x".repeat(1024 * 1024 + 1)).unwrap();
    std::fs::write(dir.join("justo.md"), "x".repeat(1024 * 1024)).unwrap();
    let mut c = sb.client();
    let ws = open(&mut c, &dir);
    assert!(
        read(&mut c, &ws, "binario.md")
            .unwrap_err()
            .1
            .contains("not text")
    );
    assert!(
        read(&mut c, &ws, "grande.md")
            .unwrap_err()
            .1
            .contains("1024 KiB")
    );
    assert_eq!(read(&mut c, &ws, "justo.md").unwrap().blocks.len(), 1);
    assert_eq!(
        read(&mut c, "no-es-uno", "a.md").unwrap_err().0,
        ErrorCode::NotFound
    );
    d.stop();
}
