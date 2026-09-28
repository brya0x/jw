package commands

import (
	"os"
	"os/exec"
	"slices"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/config"
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

func TestInitDetectsAndWritesALoadableConfig(t *testing.T) {
	env := newTestEnv(t)
	write := func(rel, content string) {
		t.Helper()
		path := env.repo + "/" + rel
		os.MkdirAll(path[:strings.LastIndex(path, "/")], 0o755)
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write(".gitignore", ".env*\nnode_modules/\n")
	write("pnpm-lock.yaml", "lockfileVersion: 9\n")
	write("apps/web/.env.local", "# web\nexport API_URL=\"http://localhost:8787/graphql\"\nTOKEN=x\n")
	write("apps/web/.env.example", "API_URL=http://localhost:8787\n") // template: skipped
	write("node_modules/pkg/.env", "X=1\n")                           // ignored dir: skipped

	repo, err := git.Open(env.repo)
	if err != nil {
		t.Fatal(err)
	}
	if err := env.app.init(repo, initOptions{}); err != nil {
		t.Fatal(err)
	}

	// Every other command now picks it up.
	p, err := loadProject(env.repo)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasSuffix(p.cfg.Source, "/jw/myapp.toml") {
		t.Fatalf("config not picked up, source %q", p.cfg.Source)
	}
	if !slices.Equal(p.cfg.Setup, []string{"pnpm install --frozen-lockfile"}) {
		t.Errorf("setup %v", p.cfg.Setup)
	}
	if len(p.cfg.Env) != 1 || p.cfg.Env[0].File != "apps/web/.env.local" {
		t.Errorf("env %+v", p.cfg.Env)
	}
	if !strings.Contains(env.out.String(), "API_URL → localhost:8787") {
		t.Errorf("output: %s", env.out)
	}

	// Never clobbers without --force.
	if err := env.app.init(repo, initOptions{}); err == nil || !strings.Contains(err.Error(), "already exists") {
		t.Fatalf("want already-exists refusal, got %v", err)
	}
	if err := env.app.init(repo, initOptions{force: true}); err != nil {
		t.Fatal(err)
	}

	// --repo writes a committable file without the personal match line.
	if err := env.app.init(repo, initOptions{repo: true}); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(env.repo + "/.jw.toml")
	if err != nil || strings.Contains(string(data), "\nmatch ") {
		t.Fatalf(".jw.toml: %v\n%s", err, data)
	}
}

func TestNewOnExistingBranch(t *testing.T) {
	env := newTestEnv(t)
	git := func(args ...string) string {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = env.repo
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
		return strings.TrimSpace(string(out))
	}
	commit := func(msg string) {
		git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", msg)
	}

	// A local branch with its own history, and one that only lives on origin.
	git("checkout", "-q", "-b", "feat/local")
	commit("local work")
	localHead := git("rev-parse", "HEAD")
	git("checkout", "-q", "-b", "feat/remote")
	commit("remote work")
	remoteHead := git("rev-parse", "HEAD")
	git("push", "-q", "origin", "feat/remote")
	git("checkout", "-q", "main")
	git("branch", "-q", "-D", "feat/remote")

	p, err := loadProject(env.repo)
	if err != nil {
		t.Fatal(err)
	}

	// Colliding without --branch stays an error, with the way out in it.
	// The template feat/{name} gives feat/local, which exists: refuse.
	err = env.app.newStream(p, newOptions{name: "local"})
	if err == nil {
		t.Fatal("an existing branch must not be reused without --branch")
	}
	if !strings.Contains(err.Error(), "pass --branch feat/local") {
		t.Fatalf("got %v", err)
	}

	// --branch with an existing local branch: checked out as is.
	if err := env.app.newStream(p, newOptions{name: "local", branch: "feat/local"}); err != nil {
		t.Fatal(err)
	}
	e, _ := p.reg.Find(p.name, "local")
	if head, _ := gitHead(e.Path); head != localHead {
		t.Fatalf("worktree at %s, want the branch's own %s", head, localHead)
	}

	// --branch with a branch only on origin: a tracking branch is made.
	if err := env.app.newStream(p, newOptions{name: "remote", branch: "feat/remote"}); err != nil {
		t.Fatal(err)
	}
	e, _ = p.reg.Find(p.name, "remote")
	if head, _ := gitHead(e.Path); head != remoteHead {
		t.Fatalf("worktree at %s, want origin's %s", head, remoteHead)
	}

	// --from makes no sense on a branch that already has history.
	err = env.app.newStream(p, newOptions{name: "other", branch: "feat/local", from: "origin/main"})
	if err == nil || !strings.Contains(err.Error(), "--from") {
		t.Fatalf("want --from refusal, got %v", err)
	}
}

func TestRollbackKeepsAnExistingBranch(t *testing.T) {
	env := newTestEnv(t)
	cmd := exec.Command("git", "branch", "feat/keep")
	cmd.Dir = env.repo
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}

	p, err := loadProject(env.repo)
	if err != nil {
		t.Fatal(err)
	}
	// An env "file" that is a directory in the main checkout can't be read,
	// so provision fails after the worktree already exists.
	p.cfg.Env = append(p.cfg.Env, config.EnvFile{File: "sub"})
	os.MkdirAll(env.repo+"/sub", 0o755)

	err = env.app.newStream(p, newOptions{name: "keep", branch: "feat/keep"})
	if err == nil || !strings.Contains(err.Error(), "rolled back") {
		t.Fatalf("want a rollback, got %v", err)
	}
	if _, err := os.Stat(p.cfg.Root + "/keep"); !os.IsNotExist(err) {
		t.Fatal("rollback should remove the worktree")
	}
	if !p.repo.BranchExists("feat/keep") {
		t.Fatal("rollback deleted a branch jw did not create")
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

func gitHead(dir string) (string, error) { return git.Head(dir) }
