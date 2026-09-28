package commands

import (
	"os"
	"slices"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/connectors/git"
)

func TestOpenLayoutAndEnvOnEveryPane(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")

	if err := env.app.open(p, e, openOptions{noFocus: true}); err != nil {
		t.Fatal(err)
	}

	// Bottom split first (full width), then the top cut in two.
	root := "p3" // w1, t2, p3: the workspace's first pane
	want := []string{root + " down 0.7", root + " right 0.5"}
	if !slices.Equal(env.mux.splits, want) {
		t.Fatalf("splits %v, want %v", env.mux.splits, want)
	}
	// herdr doesn't pass env to splits, so every pane must get it.
	for id, penv := range env.mux.paneEnv {
		if !slices.Contains(penv, "JW_NAME=web") {
			t.Errorf("pane %s created without JW_* env: %v", id, penv)
		}
	}
	if !strings.HasPrefix(env.mux.ran[root], "nvim") {
		t.Errorf("editor pane ran %q", env.mux.ran[root])
	}
	if len(env.mux.agents) != 1 || !strings.HasPrefix(env.mux.agents[0], "web claude") {
		t.Errorf("agents %v", env.mux.agents)
	}
	if e.Tab == "" || !e.Opened {
		t.Errorf("entry not updated: tab=%q opened=%v", e.Tab, e.Opened)
	}
}

func TestReopenResumesAndIsIdempotent(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	env.app.open(p, e, openOptions{noFocus: true})

	// Live tab: focused, not rebuilt.
	env.app.open(p, e, openOptions{})
	if len(env.mux.agents) != 1 || len(env.mux.focused) != 1 {
		t.Fatalf("second open rebuilt the tab: agents %v", env.mux.agents)
	}

	// Tab closed by hand: rebuilt, and the agent resumes.
	env.mux.CloseTab(e.Tab)
	env.app.open(p, e, openOptions{noFocus: true})
	if got := env.mux.agents[1]; !strings.HasSuffix(got, "--continue") {
		t.Fatalf("reopen should resume, got %q", got)
	}
}

func TestCloseAsksWhenDevRuns(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	env.app.open(p, e, openOptions{noFocus: true})
	tab := e.Tab

	for id, pane := range env.mux.panes {
		if pane.Label == "dev" {
			env.mux.running[id] = []connectors.Process{{Name: "node", Cmdline: "vite --port 20100"}}
		}
	}

	env.yes = false
	if err := env.app.close(p, e, false); err != nil {
		t.Fatal(err)
	}
	if len(env.asked) != 1 || len(env.mux.closed) != 0 || e.Tab != tab {
		t.Fatalf("a no should leave the tab: asked %v closed %v", env.asked, env.mux.closed)
	}

	env.yes = true
	if err := env.app.close(p, e, false); err != nil {
		t.Fatal(err)
	}
	if len(env.mux.closed) != 1 || e.Tab != "" {
		t.Fatalf("a yes should close: closed %v tab %q", env.mux.closed, e.Tab)
	}
	if _, err := os.Stat(e.Path); err != nil {
		t.Fatal("close must keep the worktree")
	}
}

func TestDoneGuardsThenDeletes(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	path, branch := e.Path, e.Branch
	head, _ := git.Head(path)

	// No PR.
	if err := env.app.done(p, e); err == nil || !strings.Contains(err.Error(), "no pull request") {
		t.Fatalf("want no-PR refusal, got %v", err)
	}

	// Open PR.
	env.app.PRs = fakePRs{&connectors.PR{Number: 7, State: "OPEN", Branch: branch, HeadSHA: head}}
	if err := env.app.done(p, e); err == nil || !strings.Contains(err.Error(), "not merged") {
		t.Fatalf("want not-merged refusal, got %v", err)
	}

	// Merged, but the tree is dirty.
	env.app.PRs = fakePRs{&connectors.PR{Number: 7, State: "MERGED", Branch: branch, HeadSHA: head}}
	os.WriteFile(path+"/new.txt", []byte("x"), 0o644)
	if err := env.app.done(p, e); err == nil || !strings.Contains(err.Error(), "uncommitted") {
		t.Fatalf("want dirty refusal, got %v", err)
	}
	os.Remove(path + "/new.txt")

	// Merged and clean, but the user says no.
	env.yes = false
	if err := env.app.done(p, e); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatal("a no must keep the worktree")
	}

	// Yes: everything goes.
	env.yes = true
	if err := env.app.done(p, e); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("worktree still on disk")
	}
	if p.repo.BranchExists(branch) {
		t.Error("branch still exists")
	}
	if p.reg.Has(p.name, "web") {
		t.Error("entry still registered")
	}
	if !strings.Contains(env.out.String(), "done: web removed, slot 1 free") {
		t.Errorf("output: %s", env.out)
	}
}

func TestSetupRunsThroughShell(t *testing.T) {
	env := newTestEnv(t)
	p, err := loadProject(env.repo)
	if err != nil {
		t.Fatal(err)
	}
	p.cfg.Setup = []string{"pnpm install --frozen-lockfile", "echo {name} {slot}"}
	if err := env.app.newStream(p, newOptions{name: "api"}); err != nil {
		t.Fatal(err)
	}
	want := []string{"pnpm install --frozen-lockfile", "echo api 1"}
	if !slices.Equal(env.shell.ran, want) {
		t.Fatalf("shell ran %v, want %v", env.shell.ran, want)
	}
}
