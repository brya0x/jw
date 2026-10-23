//! N166a: the diff and the viewer are leaves of a workspace's layout (REQ-709 to REQ-717).

mod common;

use std::path::PathBuf;

use common::{Sandbox, WAIT};
use contract::{Cmd, ErrorCode, SplitDir, ViewKind};
use serde_json::Value;
use wire::Client;

struct World {
    sb: Sandbox,
    ws: String,
}

/// A workspace with two panes, `a | b`, opened on the client.
fn world() -> (World, daemon::Daemon, Client, String, String) {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir: PathBuf = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = sb.client();
    let ws = c
        .cmd(Cmd::OpenFolder {
            path: dir.to_string_lossy().into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&ws).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&ws)).unwrap());
    let a = new_pane(&mut c, &ws, None);
    let b = new_pane(&mut c, &ws, Some(a.clone()));
    assert!(
        c.wait_until(WAIT, |c| leaf_ids(&c.nodes[&ws]["layout"]).len() == 2)
            .unwrap()
    );
    (World { sb, ws }, d, c, a, b)
}

fn new_pane(c: &mut Client, ws: &str, beside: Option<String>) -> String {
    c.cmd(Cmd::NewPane {
        workspace: ws.into(),
        program: None,
        beside,
        dir: Some(SplitDir::Row),
    })
    .unwrap()
    .unwrap()
    .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn open_view(
    c: &mut Client,
    ws: &str,
    kind: ViewKind,
    path: Option<&str>,
    beside: Option<&str>,
    dir: Option<SplitDir>,
) -> Result<String, ErrorCode> {
    let r = c
        .cmd(Cmd::OpenView {
            workspace: ws.into(),
            kind,
            path: path.map(str::to_owned),
            beside: beside.map(str::to_owned),
            dir,
        })
        .unwrap();
    match r {
        Ok(v) => Ok(v.unwrap()["view"].as_str().unwrap().to_owned()),
        Err(e) => Err(e.code),
    }
}

fn close_view(c: &mut Client, view: &str, path: Option<&str>) -> Result<(), ErrorCode> {
    c.cmd(Cmd::CloseView {
        view: view.into(),
        path: path.map(str::to_owned),
    })
    .unwrap()
    .map(|_| ())
    .map_err(|e| e.code)
}

fn leaf_ids(layout: &Value) -> Vec<String> {
    match layout["t"].as_str() {
        Some("pane") => vec![layout["pane"].as_str().unwrap().to_owned()],
        Some(_) => {
            let mut v = leaf_ids(&layout["a"]);
            v.extend(leaf_ids(&layout["b"]));
            v
        }
        None => vec![],
    }
}

fn layout_of(c: &Client, ws: &str) -> Value {
    c.nodes[ws]["layout"].clone()
}

fn views_of(c: &Client, ws: &str) -> Vec<Value> {
    c.nodes[ws]
        .get("views")
        .map(|v| v.as_array().unwrap().clone())
        .unwrap_or_default()
}

fn wait_layout(c: &mut Client, ws: &str, f: impl Fn(&Value) -> bool) {
    assert!(
        c.wait_until(WAIT, |c| f(&c.nodes[ws])).unwrap(),
        "the workspace never became what the test waits for: {}",
        c.nodes[ws]
    );
}

/// REQ-709, REQ-710, REQ-712: the diff is one, at the right edge with half; it is a leaf.
#[test]
fn the_diff_is_one_view_at_the_edge_and_a_leaf_of_the_layout() {
    let (w, d, mut c, a, b) = world();
    assert!(
        c.nodes[&w.ws].get("views").is_none(),
        "absent when there are none"
    );
    let v = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    assert_eq!(
        leaf_ids(&layout_of(&c, &w.ws)),
        [a.clone(), b.clone(), v.clone()]
    );
    let root = layout_of(&c, &w.ws);
    assert_eq!(root["dir"], "row");
    assert_eq!(root["ratio"], 500);
    assert_eq!(root["b"]["pane"], v.as_str());
    let views = views_of(&c, &w.ws);
    assert_eq!(views.len(), 1);
    assert_eq!(views[0]["kind"], "diff");
    assert_eq!(views[0]["title"], "diff");
    // A second open finds it.
    let again = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    assert_eq!(again, v);
    assert_eq!(views_of(&c, &w.ws).len(), 1);
    // The diff takes no path; a viewer needs one; a stranger is not beside anything.
    assert_eq!(
        open_view(&mut c, &w.ws, ViewKind::Diff, Some("x"), None, None),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        open_view(&mut c, &w.ws, ViewKind::Viewer, None, None, None),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        open_view(
            &mut c,
            &w.ws,
            ViewKind::Viewer,
            Some("README.md"),
            Some("nowhere"),
            None
        ),
        Err(ErrorCode::BadRequest)
    );
    d.stop();
}

/// REQ-711, REQ-713: a viewer has up to 8 tabs, the oldest goes, a tab closes, the last closes it.
#[test]
fn a_viewer_keeps_eight_tabs_and_closes_with_its_last() {
    let (w, d, mut c, a, _b) = world();
    let v = open_view(
        &mut c,
        &w.ws,
        ViewKind::Viewer,
        Some("docs/spec.md"),
        Some(&a),
        Some(SplitDir::Col),
    )
    .unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    // Beside `a`, below it.
    let root = layout_of(&c, &w.ws);
    assert!(leaf_ids(&root).contains(&v), "the viewer is a leaf: {root}");
    assert_eq!(views_of(&c, &w.ws)[0]["title"], "spec.md");
    for n in 1..=9 {
        let again = open_view(
            &mut c,
            &w.ws,
            ViewKind::Viewer,
            Some(&format!("f{n}.txt")),
            None,
            None,
        )
        .unwrap();
        assert_eq!(again, v, "a viewer is one");
    }
    wait_layout(&mut c, &w.ws, |n| {
        n["views"][0]["files"]
            .as_array()
            .is_some_and(|f| f.len() == 8 && f[7] == "f9.txt" && f[0] == "f2.txt")
    });
    assert_eq!(views_of(&c, &w.ws)[0]["title"], "f9.txt");
    // A file that is open is not a second tab.
    open_view(&mut c, &w.ws, ViewKind::Viewer, Some("f9.txt"), None, None).unwrap();
    close_view(&mut c, &v, Some("f2.txt")).unwrap();
    wait_layout(&mut c, &w.ws, |n| {
        n["views"][0]["files"]
            .as_array()
            .is_some_and(|f| f.len() == 7)
    });
    assert_eq!(
        close_view(&mut c, &v, Some("never-open.txt")),
        Err(ErrorCode::BadRequest)
    );
    for n in 3..=9 {
        close_view(&mut c, &v, Some(&format!("f{n}.txt"))).unwrap();
    }
    wait_layout(&mut c, &w.ws, |n| n.get("views").is_none());
    assert!(!leaf_ids(&layout_of(&c, &w.ws)).contains(&v));
    assert_eq!(close_view(&mut c, &v, None), Err(ErrorCode::NotFound));
    d.stop();
}

/// REQ-714, REQ-715: closed and opened again, a view is where it was, with its share; also after
/// it was swapped to the left and made narrower.
#[test]
fn a_view_comes_back_where_it_was_closed() {
    let (w, d, mut c, a, b) = world();
    let before = layout_of(&c, &w.ws);
    let v = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    let at_edge = layout_of(&c, &w.ws);
    let first = v.clone();
    close_view(&mut c, &v, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n.get("views").is_none());
    assert_eq!(layout_of(&c, &w.ws), before);
    let v = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    let shape = |l: &Value, id: &str| l.to_string().replace(id, "V");
    assert_eq!(
        shape(&layout_of(&c, &w.ws), &v),
        shape(&at_edge, &first),
        "the same tree as the first time"
    );
    // Swap the diff to the left and make it narrower, close it, open it: the same.
    c.cmd(Cmd::SwapPanes {
        a: v.clone(),
        b: a.clone(),
    })
    .unwrap()
    .unwrap();
    wait_layout(&mut c, &w.ws, |n| leaf_ids(&n["layout"])[0] == v.as_str());
    c.cmd(Cmd::GrowPane {
        pane: v.clone(),
        dir: SplitDir::Row,
        by: -150,
    })
    .unwrap()
    .unwrap();
    wait_layout(&mut c, &w.ws, |n| n["layout"]["a"]["ratio"] == 350);
    let moved = layout_of(&c, &w.ws);
    let order = leaf_ids(&moved);
    assert_eq!(order, [v.clone(), b.clone(), a.clone()]);
    close_view(&mut c, &v, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n.get("views").is_none());
    let v2 = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    let again = layout_of(&c, &w.ws);
    assert_eq!(again["a"]["ratio"], 350, "its share is kept: {again}");
    assert_eq!(leaf_ids(&again)[0], v2.as_str(), "and its side: {again}");
    // When what it sat beside is gone, it goes by the rule: the right edge.
    close_view(&mut c, &v2, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n.get("views").is_none());
    c.cmd(Cmd::ClosePane { pane: b.clone() }).unwrap().unwrap();
    wait_layout(&mut c, &w.ws, |n| leaf_ids(&n["layout"]).len() == 1);
    let v3 = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    let last = layout_of(&c, &w.ws);
    assert_eq!(leaf_ids(&last), [a.clone(), v3.clone()]);
    assert_eq!(last["ratio"], 500);
    d.stop();
}

/// RISK of REQ-709: a pane made with no `beside` goes beside the last pane, not into the room of
/// a view; a swap, a grow and a new pane do not drop a view's leaf.
#[test]
fn a_new_pane_a_swap_and_a_grow_keep_the_leaf_of_a_view() {
    let (w, d, mut c, a, b) = world();
    let v = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    // The phone's `+`: no `beside`.
    let p = c
        .cmd(Cmd::NewPane {
            workspace: w.ws.clone(),
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_layout(&mut c, &w.ws, |n| leaf_ids(&n["layout"]).len() == 4);
    let root = layout_of(&c, &w.ws);
    assert_eq!(
        root["b"]["pane"],
        v.as_str(),
        "the diff is still alone at the edge: {root}"
    );
    assert!(leaf_ids(&root["a"]).contains(&p));
    // Swap a view with a pane, then a pane with a pane: nobody is lost.
    c.cmd(Cmd::SwapPanes {
        a: v.clone(),
        b: a.clone(),
    })
    .unwrap()
    .unwrap();
    c.cmd(Cmd::SwapPanes {
        a: b.clone(),
        b: p.clone(),
    })
    .unwrap()
    .unwrap();
    c.cmd(Cmd::GrowPane {
        pane: v.clone(),
        dir: SplitDir::Row,
        by: 50,
    })
    .unwrap()
    .unwrap();
    wait_layout(&mut c, &w.ws, |n| {
        let mut ids = leaf_ids(&n["layout"]);
        ids.sort();
        let mut want = vec![a.clone(), b.clone(), p.clone(), v.clone()];
        want.sort();
        ids == want && n["layout"]["b"]["pane"] != v.as_str()
    });
    assert_eq!(views_of(&c, &w.ws).len(), 1);
    // A view and a pane that is not in this workspace are not swapped.
    assert_eq!(
        c.cmd(Cmd::SwapPanes {
            a: v.clone(),
            b: "nowhere".into()
        })
        .unwrap()
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    d.stop();
}

/// REQ-717: views and their leaves are not saved; a restart gives the room to the panes.
#[test]
fn a_restart_drops_the_views_and_the_session_file_never_names_one() {
    let (w, d, mut c, a, b) = world();
    let v = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    let f = open_view(
        &mut c,
        &w.ws,
        ViewKind::Viewer,
        Some("README.md"),
        Some(&a),
        None,
    )
    .unwrap();
    wait_layout(&mut c, &w.ws, |n| {
        n["views"].as_array().is_some_and(|v| v.len() == 2)
    });
    // The session is saved on every change; give it the moment it takes.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let saved = std::fs::read_to_string(w.sb.config().session_file()).unwrap();
    assert!(!saved.contains(&v) && !saved.contains(&f), "saved: {saved}");
    assert!(saved.contains(&a) && saved.contains(&b));
    let before = leaf_ids(&layout_of(&c, &w.ws));
    assert_eq!(before.len(), 4);
    drop(c);
    d.stop();
    let d = w.sb.start();
    let mut c = w.sb.client();
    c.open(&w.ws).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&w.ws)).unwrap());
    assert!(views_of(&c, &w.ws).is_empty());
    let mut after = leaf_ids(&layout_of(&c, &w.ws));
    after.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(after, want);
    d.stop();
}

/// REQ-716: a workspace that has no panes has no views.
#[test]
fn closing_the_last_pane_takes_the_views_with_it() {
    let (w, d, mut c, a, b) = world();
    let v = open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None).unwrap();
    wait_layout(&mut c, &w.ws, |n| n["views"].as_array().is_some());
    c.cmd(Cmd::ClosePane { pane: a }).unwrap().unwrap();
    c.cmd(Cmd::ClosePane { pane: b }).unwrap().unwrap();
    wait_layout(&mut c, &w.ws, |n| {
        n.get("views").is_none() && n.get("layout").is_none()
    });
    assert_eq!(
        open_view(&mut c, &w.ws, ViewKind::Diff, None, None, None),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(close_view(&mut c, &v, None), Err(ErrorCode::NotFound));
    d.stop();
}
