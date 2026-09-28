package commands

import (
	"os"
	"os/exec"
	"strings"
	"testing"
)

func TestRmGuardsLocalWork(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	path, branch := e.Path, e.Branch

	// A commit that exists nowhere else.
	cmd := exec.Command("git", "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "only here")
	cmd.Dir = path
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}

	// Refused outright, without even asking.
	env.yes = true
	err := env.app.rm(p, e, rmOptions{})
	if err == nil || !strings.Contains(err.Error(), "would be lost") {
		t.Fatalf("want refusal, got %v", err)
	}
	if len(env.asked) != 0 {
		t.Fatal("must not ask before --force when work would be lost")
	}
	if !strings.Contains(env.out.String(), "only here") {
		t.Errorf("should show the commit that would be lost:\n%s", env.out)
	}

	// Keeping the branch keeps the commit: nothing is lost, so it asks.
	env.yes = false
	if err := env.app.rm(p, e, rmOptions{keepBranch: true}); err != nil {
		t.Fatal(err)
	}
	if len(env.asked) != 1 {
		t.Fatalf("asked %v", env.asked)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatal("a no must keep the worktree")
	}

	// --force asks, and a yes removes everything local.
	env.yes = true
	if err := env.app.rm(p, e, rmOptions{force: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("worktree still on disk")
	}
	if p.repo.BranchExists(branch) {
		t.Error("local branch still exists")
	}
	if p.reg.Has(p.name, "web") {
		t.Error("still registered")
	}
}

func TestRmRefusesUncommittedFiles(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	os.WriteFile(e.Path+"/draft.txt", []byte("x"), 0o644)

	env.yes = true
	// --keep-branch doesn't save an uncommitted file: still refused.
	err := env.app.rm(p, e, rmOptions{keepBranch: true})
	if err == nil || !strings.Contains(env.out.String(), "draft.txt") {
		t.Fatalf("want refusal listing draft.txt, got %v\n%s", err, env.out)
	}
}

func TestRmKeepsAdoptedBranchAndHandlesMissingWorktree(t *testing.T) {
	env := newTestEnv(t)
	cmd := exec.Command("git", "branch", "feat/theirs")
	cmd.Dir = env.repo
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	p, err := loadProject(env.repo)
	if err != nil {
		t.Fatal(err)
	}
	if err := env.app.newStream(p, newOptions{name: "theirs", branch: "feat/theirs"}); err != nil {
		t.Fatal(err)
	}
	e, _ := p.reg.Find(p.name, "theirs")
	if !e.Adopted {
		t.Fatal("a branch that existed before jw must be marked adopted")
	}

	// The worktree was deleted by hand.
	os.RemoveAll(e.Path)

	env.yes = true
	if err := env.app.rm(p, e, rmOptions{}); err != nil {
		t.Fatal(err)
	}
	if !p.repo.BranchExists("feat/theirs") {
		t.Fatal("rm deleted a branch jw did not create")
	}
	if p.reg.Has(p.name, "theirs") {
		t.Fatal("still registered")
	}
	// git forgot the worktree too, so the branch can be checked out again.
	if err := env.app.newStream(p, newOptions{name: "again", branch: "feat/theirs"}); err != nil {
		t.Fatalf("worktree not pruned: %v", err)
	}
}
