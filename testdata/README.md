Files written by the Go jw, kept as fixtures: configs and registries in the wild still look
like this, so the Rust code must keep reading them.

- `config.go.toml`: loaded by `reads_the_go_fixture`.
- `registry.go.json`: the Rust test loads it and saves it again, and the bytes must match.
- `init-full.go.toml`, `init-bare.go.toml`: the drafts built in `src/init.rs`'s tests must
  render the same bytes.
