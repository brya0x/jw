# jw

[![CI](https://github.com/brya0x/jw/actions/workflows/ci.yml/badge.svg)](https://github.com/brya0x/jw/actions/workflows/ci.yml)

**A terminal for working with coding agents in parallel.** A daemon owns every terminal and every
conversation with Claude; a terminal client and an Android app draw what it says. Each piece of work
gets its own git worktree with a shell, a Claude, its diff and its files, and everything keeps
running when you close the client.

This is the jw that was written as `jwn` (`next/`) and replaced the old one; it took its name in N158. Its contract is a set of
JSON documents (`contract/`), its spec is `docs/spec.md`, and `AGENTS.md` says how the code is laid
out.

```
cargo install --path crates/jw --locked   # the binary
jw                   # the terminal client; it starts the daemon if none runs
jw stop              # stop the daemon
jw claude            # a conversation with Claude in this pane
^␣ then a key        # the client's commands; wait a moment and every key shows
^␣ m                 # a QR for the phone app, through a Cloudflare tunnel
just ci              # fmt, clippy, tests and the Android build, as CI runs them
```

The Android app is in `android/` (`just android` builds and lints it).
