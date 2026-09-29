package commands

import (
	"os"
	"os/exec"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors/git"
)

// syncEnv is a stream "web" with one commit of its own, while main moved on
// by one commit on origin. conflict makes both touch the same file.
func syncEnv(t *testing.T, conflict bool) (*testEnv, *project, func(dir string, args ...string) string) {
	t.Helper()
	env := newTestEnv(t)
	// rebase and merge write commits: give them an identity.
	for _, k := range []string{"GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"} {
		t.Setenv(k, "t")
	}
	for _, k := range []string{"GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"} {
		t.Setenv(k, "t@t")
	}
	run := func(dir string, args ...string) string {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
		return strings.TrimSpace(string(out))
	}
	change := func(dir, content, msg string) {
		os.WriteFile(dir+"/shared.txt", []byte(content), 0o644)
		run(dir, "add", "shared.txt")
		run(dir, "commit", "-q", "-m", msg)
	}

	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	if conflict {
		change(e.Path, "stream\n", "stream work")
		change(env.repo, "main\n", "main work")
	} else {
		os.WriteFile(e.Path+"/stream.txt", []byte("s\n"), 0o644)
		run(e.Path, "add", "stream.txt")
		run(e.Path, "commit", "-q", "-m", "stream work")
		change(env.repo, "main\n", "main work")
	}
	run(env.repo, "push", "-q", "origin", "main")
	return env, p, run
}

func TestSyncRebasesByDefault(t *testing.T) {
	env, p, run := syncEnv(t, false)
	e, _ := p.reg.Find(p.name, "web")

	if err := env.app.sync(p, e, syncOptions{}); err != nil {
		t.Fatalf("%v\n%s", err, env.out)
	}
	if behind, ahead, _ := git.Divergence(e.Path, "origin/main"); behind != 0 || ahead != 1 {
		t.Fatalf("after rebase: %d behind, %d ahead", behind, ahead)
	}
	// Linear: no merge commit.
	if parents := run(e.Path, "log", "-1", "--format=%P"); strings.Contains(parents, " ") {
		t.Fatal("rebase must not create a merge commit")
	}
	if !strings.Contains(env.out.String(), "1 behind, 1 ahead") {
		t.Errorf("output: %s", env.out)
	}

	// Running it again is a no-op.
	env.out.Reset()
	if err := env.app.sync(p, e, syncOptions{}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(env.out.String(), "already up to date") {
		t.Errorf("output: %s", env.out)
	}
}

func TestSyncWarnsToForcePushAPublishedBranch(t *testing.T) {
	env, p, run := syncEnv(t, false)
	e, _ := p.reg.Find(p.name, "web")
	run(e.Path, "push", "-q", "-u", "origin", e.Branch)
	run(env.repo, "fetch", "-q", "origin")

	if err := env.app.sync(p, e, syncOptions{}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(env.out.String(), "git push --force-with-lease") {
		t.Fatalf("a rebased, already pushed branch needs the warning:\n%s", env.out)
	}
}

func TestSyncMerge(t *testing.T) {
	env, p, run := syncEnv(t, false)
	e, _ := p.reg.Find(p.name, "web")

	if err := env.app.sync(p, e, syncOptions{merge: true}); err != nil {
		t.Fatalf("%v\n%s", err, env.out)
	}
	if parents := run(e.Path, "log", "-1", "--format=%P"); !strings.Contains(parents, " ") {
		t.Fatal("--merge must create a merge commit")
	}
	if strings.Contains(env.out.String(), "force") {
		t.Errorf("a merge never needs a force push:\n%s", env.out)
	}
}

func TestSyncStopsAtConflictsAndGuardsTheNextRun(t *testing.T) {
	env, p, _ := syncEnv(t, true)
	e, _ := p.reg.Find(p.name, "web")

	err := env.app.sync(p, e, syncOptions{})
	if err == nil || !strings.Contains(err.Error(), "stopped at conflicts") {
		t.Fatalf("want a stop, got %v", err)
	}
	if !strings.Contains(env.out.String(), "shared.txt") || !strings.Contains(env.out.String(), "git rebase --abort") {
		t.Errorf("should list the file and the way out:\n%s", env.out)
	}
	if op := git.Operation(e.Path); op != "rebase" {
		t.Fatalf("the rebase should be left in progress for resolving, got %q", op)
	}

	// A second sync refuses instead of stacking another operation on top.
	err = env.app.sync(p, e, syncOptions{})
	if err == nil || !strings.Contains(err.Error(), "rebase in progress") {
		t.Fatalf("want in-progress refusal, got %v", err)
	}
}

func TestSyncRefusesADirtyTree(t *testing.T) {
	env, p, _ := syncEnv(t, false)
	e, _ := p.reg.Find(p.name, "web")
	os.WriteFile(e.Path+"/draft.txt", []byte("x"), 0o644)

	if err := env.app.sync(p, e, syncOptions{}); err == nil || !strings.Contains(err.Error(), "uncommitted") {
		t.Fatalf("want dirty refusal, got %v", err)
	}
}
