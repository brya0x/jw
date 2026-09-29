package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

// setup gives each test its own repo dir and an empty personal config dir.
func setup(t *testing.T) (repo, personal string) {
	t.Helper()
	dir := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	return filepath.Join(dir, "myapp"), filepath.Join(dir, "config", "jw")
}

func TestDefaults(t *testing.T) {
	repo, _ := setup(t)
	c, err := Load(repo, "git@github.com:acme/myapp.git", "myapp")
	if err != nil {
		t.Fatal(err)
	}
	if c.Source != "" || c.Root != repo+"-wt" || c.Branch != "feat/{name}" || c.Workspace != "myapp" {
		t.Fatalf("bad defaults: %+v", c)
	}
}

func TestRepoFileWinsOverPersonal(t *testing.T) {
	repo, personal := setup(t)
	writeFile(t, filepath.Join(repo, ".jw.toml"), `branch = "repo/{name}"`)
	writeFile(t, filepath.Join(personal, "myapp.toml"), `branch = "personal/{name}"`)

	c, err := Load(repo, "https://github.com/acme/myapp", "myapp")
	if err != nil {
		t.Fatal(err)
	}
	if c.Branch != "repo/{name}" {
		t.Fatalf("want repo file, got %s from %s", c.Branch, c.Source)
	}
}

func TestPersonalMatchedByRemote(t *testing.T) {
	repo, personal := setup(t)
	// The file name doesn't matter when `match` is set; ssh vs https doesn't either.
	writeFile(t, filepath.Join(personal, "work.toml"), `
match = "github.com/acme/myapp"
root  = "worktrees"
`)
	c, err := Load(repo, "git@github.com:acme/myapp.git", "myapp")
	if err != nil {
		t.Fatal(err)
	}
	if c.Root != filepath.Join(repo, "worktrees") {
		t.Fatalf("relative root should resolve against the repo, got %s", c.Root)
	}
}

func TestUnknownKeyFails(t *testing.T) {
	repo, _ := setup(t)
	writeFile(t, filepath.Join(repo, ".jw.toml"), `setpu = ["pnpm install"]`)
	_, err := Load(repo, "x", "myapp")
	if err == nil || !strings.Contains(err.Error(), "setpu") {
		t.Fatalf("want unknown key error, got %v", err)
	}
}

func TestUnknownPlaceholderFails(t *testing.T) {
	repo, _ := setup(t)
	writeFile(t, filepath.Join(repo, ".jw.toml"), `
[ports]
web = 0

[dev]
web = ["vite --port {port.wbe}"]
`)
	_, err := Load(repo, "x", "myapp")
	if err == nil || !strings.Contains(err.Error(), "{port.wbe}") {
		t.Fatalf("want placeholder error, got %v", err)
	}
}

func TestOffsetOutOfBlockFails(t *testing.T) {
	repo, _ := setup(t)
	writeFile(t, filepath.Join(repo, ".jw.toml"), "[ports]\nweb = 100\n")
	if _, err := Load(repo, "x", "myapp"); err == nil {
		t.Fatal("offset 100 must not fit in a 100-port block")
	}
}

func TestExpand(t *testing.T) {
	v := Vars{Name: "web", Base: "development", Slot: 3, Ports: map[string]int{"api": 20302}}
	got, err := Expand("{name} {base} {slot} http://localhost:{port.api}", v)
	if err != nil {
		t.Fatal(err)
	}
	if want := "web development 3 http://localhost:20302"; got != want {
		t.Fatalf("got %q want %q", got, want)
	}
}

func TestSameRepo(t *testing.T) {
	for _, u := range []string{
		"git@github.com:Acme/MyApp.git",
		"https://github.com/acme/myapp",
		"ssh://git@github.com/acme/myapp.git",
	} {
		if !sameRepo("github.com/acme/myapp", u) {
			t.Errorf("%s should match", u)
		}
	}
	if sameRepo("github.com/acme/myapp", "github.com/acme/myapp-api") {
		t.Error("prefix must not match")
	}
}

func TestEnvName(t *testing.T) {
	if got := EnvName("console-web"); got != "JW_PORT_CONSOLE_WEB" {
		t.Fatal(got)
	}
}

func TestSyncMode(t *testing.T) {
	repo, _ := setup(t)
	c, err := Load(repo, "x", "myapp")
	if err != nil || c.Sync != "rebase" {
		t.Fatalf("default sync should be rebase, got %q (%v)", c.Sync, err)
	}
	writeFile(t, filepath.Join(repo, ".jw.toml"), `sync = "squash"`)
	if _, err := Load(repo, "x", "myapp"); err == nil || !strings.Contains(err.Error(), "rebase") {
		t.Fatalf("want an invalid-mode error, got %v", err)
	}
}
