# Work order: the server track (Rust)

You are the **server agent**. You build the Rust half of `jw mobile`. Another agent, the **app agent**, works in a second pane of this same workspace on the Android app (`android/`). A third party, the **brain** (the main Claude session in this workspace), reads your progress, reviews each part and writes to you.

## Where you are

- Checkout: `/Users/bastacio/ws/jw-wt/mobile-app`, branch `feat/mobile-app`. **The app agent shares this checkout and its git index.** That is why the git rules below are strict.
- The spec is `docs/specs/mobile.md`. You own it: when behaviour changes, update it in the same commit, keep REQ, RAT, RISK and the parts table true, and add a Corrections line when something it said was wrong.
- Read first, in this order: `AGENTS.md`, then `crates/web/AGENTS.md`, `crates/daemon/AGENTS.md`, `crates/tui/AGENTS.md`, `crates/proto/AGENTS.md`, `crates/jw/AGENTS.md`, `crates/core/AGENTS.md`, then `docs/specs/mobile.md` (all of it), then `docs/specs/mobile-modal-prototype.html` (read its CSS and JS: the views, keys, copy and colours of the TUI modal are in there and are what the user approved).
- M1 and M2 are built and tested: `crates/web/src/bridge.rs`, `crates/web/src/auth.rs`, `crates/jw/src/mobile.rs`, `crates/jw/tests/web.rs`. Read them; D1 moves their hosting, it does not rewrite their logic.

## Your parts, in this order

One commit per part. Do not start a part before the previous one is committed.

1. **W1: palettes in `ready`** (REQ-17). `ready` gains `palettes` (the built-in themes plus user themes, each as the colour roles of `tui/src/theme.rs:19-37` and `dark`) and `settings` (which theme is the dark slot, which the light slot, from `settings.json`). RISK-19 is the catch: palettes live in the TUI crate and the daemon must not depend on it. The least invasive way is for the `jw` binary, which depends on everything, to hand a callback to whoever builds the bridge. Choose, record the choice in the spec (Interfaces and a Corrections line if it changes what the spec said), and test it.
2. **D1: the daemon hosts mobile** (REQ 1, 3, 6, 9, 33 to 40, 50). The tunnel code moves from `crates/jw/src/mobile.rs` to `crates/web/src/tunnel.rs`. New `crates/daemon/src/mobile.rs`: the service with phases off, starting, ready, failed, its own threads and locks (REQ-40). New messages `Mobile*` in `proto.rs`; a watcher list on `Daemon` (the daemon has no all-client broadcast today); the stop hook in `save_on_signal` between `save()` and `exit(0)`, with a 2 s limit. `JW_CLOUDFLARED` replaces the program and skips the DNS wait, for tests. **Bump `PROTOCOL` once**: run `git fetch origin` and look at `origin/main:crates/proto/src/proto.rs`, then take the first number free past it. Fix the stale line numbers in `crates/daemon/AGENTS.md`, add the new dependency edge to `AGENTS.md`, and keep the rule that the TUI never depends on the daemon.
3. **D2: `jw mobile` as a client** (REQ 30, 41). `jw mobile` watches, starts mobile if off, prints the steps, the QR and the link, and answers pair questions in the terminal. Ctrl-C leaves mobile running. `jw mobile stop`. `jw mobile devices` and `revoke` keep working with mobile off. The QR drawing moves to `crates/core/src/qr.rs`, shared with the TUI. Without a daemon, start one as the TUI does.
4. **T1: the TUI** (REQ 42 to 49). `Modal::Mobile` with the views of the prototype, `^␣ m` (row in `settings::ACTIONS`, group `go`, key `m`; mind that the array has a fixed length and tests may count it), the status-bar indicator, the pair question that opens over the screen (held back while another modal or picker is open), the QR through `core::qr`, the too-small fallback. Mirror the prototype's copy and layout. `MobileWatch` right after `Hello`.

## Done means, for every part

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` all pass. Run them yourself and read the output; a green line is not proof if a command was cut short.
- Tests exist for what the part adds (the spec's Tests section says which), and each new test fails when the behaviour is broken.
- The spec, the crate `AGENTS.md` files and the README are true after the commit.
- **Version: do not bump it.** The brain does that when the PR is prepared.
- Append one line to `.mobile-work/server/STATUS.md`: date, part, what it does, the commit hash. Then read `.mobile-work/server/INBOX.md`: the brain may have written to you.

## Git, because the index is shared

- Stage and commit **only your own paths**: `git add crates scripts AGENTS.md docs/specs/mobile.md` (list what you changed) and then `git commit -m "<message>" -- crates scripts AGENTS.md docs/specs/mobile.md`. The `--` with paths makes the commit take only those paths even if the app agent has things staged.
- **Never** `git add -A`, `git add .`, `git commit -a`, `git stash`, `git checkout`, `git reset`, `git merge`, `git rebase`, `git push` or `gh`. Never touch `android/`.
- If `git` reports `index.lock`, wait a few seconds and retry. Do not delete the lock.
- Commit messages: conventional (`feat(mobile): D1, ...`), no `Co-Authored-By` unless the brain says so.

## Rules from this repo and from the user

- **Never `pkill jw` and never touch the user's real daemon.** It is the process `/Users/bastacio/.cargo/bin/jw daemon`, and your own pane belongs to it. Tests and hand checks use their own socket directory and kill **only the pid they started**.
- A unix socket path must stay under about 104 characters on macOS: for hand checks `cd` to a directory with a short path and use a **relative** `JW_SOCKET` (for example `jw.sock`), with `XDG_CONFIG_HOME` and `XDG_STATE_HOME` in a temp directory.
- **Tests must not be able to start a tunnel.** Any test that expects `jw mobile` or the daemon to fail before a tunnel runs with a `PATH` that has no `cloudflared`. (A run without that once put a real tunnel on the internet from a test.) The real tunnel is checked by hand, with isolated state, once per part at most, and you stop what you started.
- Never bind a key with Alt (AeroSpace owns Alt).
- `cargo` and the app agent: the app agent does not use `target/`. You may use it freely.
- Anything printed from a phone-controlled string on the PC must be stripped of control characters first. No tokens or terminal bytes in logs (REQ-27).

## How you work

- Searching is not your job. Send every "where does X live" to the `scout` subagent (it runs on Haiku), one focused question each, several in parallel when you can. Read inline only the file you are about to edit.
- Call the advisor before you start a part and before you say it is done.
- Write what you decide and why in the commit message and the spec, not in chat.
- If something is the user's decision, or you are stuck after two real attempts, write `BLOCKED: <what and why>` as a line in `.mobile-work/server/STATUS.md` and stop until `INBOX.md` answers. Do not ask the user directly: the brain relays.
- When all four parts are committed, append `ALL DONE` with a short summary to `STATUS.md`.
