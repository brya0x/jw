//! `jwn bench`: measure the real wire. Starts a private daemon (own socket dir, `/bin/sh`
//! panes), fills it with 25 workspaces of 2 panes each, and reports document sizes,
//! patch sizes, idle cost and latencies as a Markdown table.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use contract::Cmd;
use daemon::{Config, Daemon};
use wire::Client;

const WORKSPACES: usize = 25;
const PANES_PER_WORKSPACE: usize = 2;
const WAIT: Duration = Duration::from_secs(10);

fn pct(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn latency_row(name: &str, mut samples: Vec<Duration>) {
    samples.sort();
    println!(
        "| {name} | {} | {} | {} |",
        ms(pct(&samples, 0.5)),
        ms(pct(&samples, 0.95)),
        ms(*samples.last().unwrap())
    );
}

fn quiet(c: &mut Client, how_long: Duration) -> Result<(), String> {
    c.wait_until(how_long, |_| false)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn run() -> Result<(), String> {
    let dir = PathBuf::from(format!("/tmp/jwn-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let config = Config {
        socket: dir.join("s.sock"),
        state_dir: dir.join("state"),
        shell: Some("/bin/sh".into()),
        tcp: None,
        env: Vec::new(),
        blocks: true,
        link: daemon::LinkConfig::off(),
        hook_exe: None,
        gh: None,
        github_every: std::time::Duration::from_secs(60),
        conversation: false,
        projects: dir.join("projects"),
    };
    let socket = config.socket.clone();
    let daemon =
        Daemon::start(config).map_err(|e| format!("cannot start the bench daemon: {e}"))?;
    let result = measure(&dir, &socket);
    daemon.stop();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn measure(dir: &std::path::Path, socket: &std::path::Path) -> Result<(), String> {
    let e = |x: wire::ClientError| x.to_string();
    let mut admin = Client::connect(socket, "bench-admin").map_err(e)?;

    let mut workspaces = Vec::new();
    let mut panes = Vec::new();
    for i in 0..WORKSPACES {
        let path = dir.join(format!("ws{i:02}"));
        std::fs::create_dir_all(&path).map_err(|x| x.to_string())?;
        let r = admin
            .cmd(Cmd::OpenFolder {
                path: path.to_string_lossy().into(),
            })
            .map_err(e)?
            .map_err(|x| x.message)?;
        let ws = r.unwrap()["workspace"].as_str().unwrap().to_owned();
        for _ in 0..PANES_PER_WORKSPACE {
            let r = admin
                .cmd(Cmd::NewPane {
                    workspace: ws.clone(),
                    program: None,
                    beside: None,
                    dir: None,
                })
                .map_err(e)?
                .map_err(|x| x.message)?;
            panes.push(r.unwrap()["pane"].as_str().unwrap().to_owned());
        }
        workspaces.push(ws);
    }
    std::thread::sleep(Duration::from_millis(500)); // prompts settle

    println!("# jwn wire measurements\n");
    println!(
        "{WORKSPACES} workspaces x {PANES_PER_WORKSPACE} panes, real `/bin/sh` shells, Unix socket, release={}\n",
        !cfg!(debug_assertions)
    );

    // Snapshots: bytes to open each level cold.
    println!("## Snapshots (bytes on the wire, newline included)\n");
    println!("| What | Bytes |\n|---|---|");
    let size_of = |node: &str| -> Result<u64, String> {
        let mut c = Client::connect(socket, "bench-snap").map_err(e)?;
        let before = c.bytes_in;
        c.open(node).map_err(e)?;
        c.wait_until(WAIT, |c| c.nodes.contains_key(node))
            .map_err(e)?;
        Ok(c.bytes_in - before)
    };
    let root = admin.root.clone();
    let session = size_of(&root)?;
    let workspace = size_of(&workspaces[0])?;
    let pane = size_of(&panes[0])?;
    println!("| level 0: session (25 workspace summaries) | {session} |");
    println!("| level 1: one workspace (2 panes) | {workspace} |");
    println!("| level 2: one pane (24x80, idle prompt) | {pane} |");
    let everything = session + workspaces.len() as u64 * workspace + panes.len() as u64 * pane;
    println!("| everything opened (1 + 25 + 50 nodes) | {everything} |\n");

    // A watcher that has every node open.
    let mut w = Client::connect(socket, "bench-watch").map_err(e)?;
    let all: Vec<String> = std::iter::once(root.clone())
        .chain(workspaces.iter().cloned())
        .chain(panes.iter().cloned())
        .collect();
    for n in &all {
        w.open(n).map_err(e)?;
    }
    w.wait_until(WAIT, |c| all.iter().all(|n| c.nodes.contains_key(n)))
        .map_err(e)?;
    quiet(&mut w, Duration::from_millis(300))?;

    // Patches.
    println!("## Patches (a watcher with all 76 nodes open)\n");
    println!("| Change | Patches | Bytes |\n|---|---|---|");
    let mut change =
        |label: &str, act: &mut dyn FnMut() -> Result<(), String>| -> Result<(), String> {
            let (b0, mut n) = (w.bytes_in, 0usize);
            act()?;
            let end = Instant::now() + Duration::from_millis(400);
            while Instant::now() < end {
                if w.poll(Duration::from_millis(50)).map_err(e)?.is_some() {
                    n += 1;
                }
            }
            println!("| {label} | {n} | {} |", w.bytes_in - b0);
            Ok(())
        };
    change("rename the session", &mut || {
        admin
            .cmd(Cmd::RenameSession {
                name: "bench".into(),
            })
            .map_err(e)?
            .map(|_| ())
            .map_err(|x| x.message)
    })?;
    change(
        "type `echo hi` in one pane (echo, output, new prompt)",
        &mut || {
            admin
                .cmd(Cmd::Write {
                    pane: panes[0].clone(),
                    data: "echo hi\r".into(),
                })
                .map_err(e)?
                .map(|_| ())
                .map_err(|x| x.message)
        },
    )?;
    change(
        "a command finishes with exit 3 (attention bubbles pane, workspace, session)",
        &mut || {
            let data = "printf '\\033]133;D;3\\007'\r".to_owned();
            admin
                .cmd(Cmd::Write {
                    pane: panes[1].clone(),
                    data,
                })
                .map_err(e)?
                .map(|_| ())
                .map_err(|x| x.message)
        },
    )?;
    change(
        "open one more folder (session and its summary)",
        &mut || {
            let path = dir.join("extra");
            std::fs::create_dir_all(&path).map_err(|x| x.to_string())?;
            admin
                .cmd(Cmd::OpenFolder {
                    path: path.to_string_lossy().into(),
                })
                .map_err(e)?
                .map(|_| ())
                .map_err(|x| x.message)
        },
    )?;
    println!();

    // Idle.
    println!("## Idle\n");
    let b0 = w.bytes_in;
    quiet(&mut w, Duration::from_secs(5))?;
    let idle = w.bytes_in - b0;
    println!(
        "- A watcher with 76 nodes open received **{idle} bytes in 5 s idle** (push: nothing is sent when nothing changes)."
    );
    // Reconnect check: a phone that cached everything asks "anything new?" per node.
    let mut r = Client::connect(socket, "bench-resume").map_err(e)?;
    r.nodes = w.nodes.clone();
    let b0 = r.bytes_in;
    r.open(&root).map_err(e)?;
    quiet(&mut r, Duration::from_millis(200))?;
    println!(
        "- Reconnect check of the session node with a current cache: **{} bytes** (empty patch).",
        r.bytes_in - b0
    );
    let ask = serde_json::to_vec(&contract::ClientMsg::Open {
        node: root.clone(),
        epoch: Some(r.epoch.clone()),
        since: Some(0),
    })
    .map_err(|x| x.to_string())?
    .len() as u64
        + 1;
    let per_check = ask + (r.bytes_in - b0);
    println!(
        "- Polling the session node every 10 s for 10 min: {} bytes as full snapshots ({session} x 60), {} bytes as resume checks ({per_check} x 60), 0 bytes pushed.\n",
        session * 60,
        per_check * 60
    );

    // Latency.
    println!("## Latency (Unix socket, same machine)\n");
    println!("| What | p50 | p95 | max |\n|---|---|---|---|");
    let mut reply = Vec::new();
    let mut seen = Vec::new();
    for i in 0..200 {
        let name = format!("n{i}");
        let t = Instant::now();
        admin
            .cmd(Cmd::RenameSession { name: name.clone() })
            .map_err(e)?
            .map_err(|x| x.message)?;
        reply.push(t.elapsed());
        w.wait_until(WAIT, |c| c.nodes[&root]["name"] == name.as_str())
            .map_err(e)?;
        seen.push(t.elapsed());
    }
    latency_row("command to reply (rename_session)", reply);
    latency_row("command to the change visible on a second client", seen);
    let mut cold = Vec::new();
    for _ in 0..50 {
        let t = Instant::now();
        let mut c = Client::connect(socket, "bench-cold").map_err(e)?;
        c.open(&root).map_err(e)?;
        c.wait_until(WAIT, |c| c.nodes.contains_key(&root))
            .map_err(e)?;
        cold.push(t.elapsed());
    }
    latency_row("connect + hello + open session snapshot", cold);
    let mut typed = Vec::new();
    for i in 0..30 {
        let mark = format!("m{i}x");
        let t = Instant::now();
        admin
            .cmd(Cmd::Write {
                pane: panes[2].clone(),
                data: format!("echo {mark}\r"),
            })
            .map_err(e)?
            .map_err(|x| x.message)?;
        w.wait_until(WAIT, |c| {
            c.nodes[&panes[2]]["screen"]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|l| l.as_str().unwrap().contains(&mark))
                .count()
                >= 2
        })
        .map_err(e)?;
        typed.push(t.elapsed());
    }
    latency_row("keystrokes to the output visible on a second client", typed);
    Ok(())
}
