package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDraftRendersAValidConfig(t *testing.T) {
	d := Draft{
		Project: "myapp",
		Match:   "git@github.com:acme/myapp.git",
		Root:    "~/code/myapp-wt",
		Setup:   []string{"pnpm install --frozen-lockfile"},
		Env: []DraftEnv{
			{File: "apps/web/.env.local", Localhost: []LocalhostRef{
				{Key: "API_URL", Value: "http://localhost:{port.SERVICE}", Port: "8787"},
			}},
			{File: "apps/api/.env.local"},
		},
	}
	text, err := d.Render()
	if err != nil {
		t.Fatal(err)
	}

	// Whatever init writes must load cleanly: no unknown keys, no bad
	// placeholders (the {port.SERVICE} hint is inside a comment).
	path := filepath.Join(t.TempDir(), "myapp.toml")
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
	c, err := decodeFile(path)
	if err != nil {
		t.Fatalf("%v\n%s", err, text)
	}
	if _, err := c.withDefaults("/repo", "myapp"); err != nil {
		t.Fatalf("%v\n%s", err, text)
	}

	if c.Match != d.Match || c.Workspace != "myapp" || len(c.Setup) != 1 || len(c.Env) != 2 {
		t.Fatalf("round trip lost data: %+v\n%s", c, text)
	}
	if len(c.Env[0].Set) != 0 {
		t.Fatal("the localhost suggestion must stay commented out")
	}
	if !strings.Contains(text, `# set = { API_URL = "http://localhost:{port.SERVICE}" }`) {
		t.Fatalf("missing the set suggestion:\n%s", text)
	}
}

func TestDraftWithoutMatchOrEnv(t *testing.T) {
	text, err := Draft{Project: "myapp", Root: "../myapp-wt"}.Render()
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(text, "\nmatch ") {
		t.Fatalf("a repo config has no match line:\n%s", text)
	}
	path := filepath.Join(t.TempDir(), ".jw.toml")
	os.WriteFile(path, []byte(text), 0o644)
	if _, err := decodeFile(path); err != nil {
		t.Fatalf("%v\n%s", err, text)
	}
}
