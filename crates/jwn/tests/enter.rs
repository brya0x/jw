//! REQ-186: `jwn` with no daemon starts one in the background, and it outlives the client.

mod common;

use std::time::{Duration, Instant};

use common::Env;

fn answers(env: &Env) -> bool {
    wire::Conn::connect(&env.dir.join("s.sock")).is_ok()
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn entering_with_no_daemon_starts_one_that_outlives_the_client() {
    let env = Env::new("enter");
    assert!(!answers(&env), "no daemon to begin with");
    // There is no terminal here, so the client itself gives up once it has its daemon.
    let out = env.jwn().output().unwrap();
    assert!(!out.status.success(), "the client needs a terminal");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("starting the daemon"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(answers(&env), "the daemon started for it is still there");
    assert!(env.dir.join("state").join("daemon.log").exists());

    // A second entry finds it and starts nothing.
    let again = env.jwn().output().unwrap();
    assert!(
        !String::from_utf8_lossy(&again.stderr).contains("starting the daemon"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );

    assert!(env.jwn().arg("stop").status().unwrap().success());
    wait_for("the daemon to end", || !answers(&env));
}
