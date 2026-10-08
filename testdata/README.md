Fixtures shared by the Go and Rust code while both read the same files
(docs/specs/rust-tui.md, REQ-1 and REQ-2).

- `config.go.toml`: loaded by `TestSharedFixtureLoads` (Go) and `reads_the_go_fixture` (Rust).
- `registry.go.json`: written by `registry.Save` in Go. The Rust test loads it and saves it
  again, and the bytes must match. To regenerate it, build a `Registry` in a throwaway Go
  test in `internal/core/registry` and `Save` it here.
