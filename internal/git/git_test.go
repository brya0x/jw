package git

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// newTestRepo builds origin (bare) + a clone with one commit, on a default
// branch deliberately not called main.
func newTestRepo(t *testing.T) string {
	t.Helper()
	// Ignore the developer's global git config (signing, hooks, templates).
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")

	dir := t.TempDir()
	origin := filepath.Join(dir, "myapp.git")
	work := filepath.Join(dir, "myapp")

	mustGit(t, dir, "init", "-q", "--bare", "-b", "trunk", origin)
	mustGit(t, dir, "init", "-q", "-b", "trunk", work)
	mustGit(t, work, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init")
	mustGit(t, work, "remote", "add", "origin", origin)
	mustGit(t, work, "push", "-q", "-u", "origin", "trunk")
	return work
}

func mustGit(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %s: %v\n%s", strings.Join(args, " "), err, out)
	}
}

// realpath resolves macOS's /var → /private/var so paths compare equal.
func realpath(t *testing.T, p string) string {
	t.Helper()
	r, err := filepath.EvalSymlinks(p)
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func TestDefaultBranchIsNotAssumed(t *testing.T) {
	repo, err := Open(newTestRepo(t))
	if err != nil {
		t.Fatal(err)
	}
	got, err := repo.DefaultBranch()
	if err != nil {
		t.Fatal(err)
	}
	if got != "trunk" {
		t.Fatalf("want trunk, got %s", got)
	}
}

func TestOpenFromWorktreeFindsMainCheckout(t *testing.T) {
	work := newTestRepo(t)
	repo, err := Open(work)
	if err != nil {
		t.Fatal(err)
	}

	wt := filepath.Join(filepath.Dir(work), "myapp-wt", "feature")
	if err := repo.AddWorktree(wt, "feat/feature", "origin/trunk"); err != nil {
		t.Fatal(err)
	}
	if !repo.BranchExists("feat/feature") {
		t.Fatal("branch was not created")
	}

	fromWt, err := Open(wt)
	if err != nil {
		t.Fatal(err)
	}
	if realpath(t, fromWt.Root) != realpath(t, work) {
		t.Fatalf("root from worktree = %s, want %s", fromWt.Root, work)
	}

	if err := repo.RemoveWorktree(wt); err != nil {
		t.Fatal(err)
	}
	if err := repo.DeleteBranch("feat/feature"); err != nil {
		t.Fatal(err)
	}
	if repo.BranchExists("feat/feature") {
		t.Fatal("branch still exists")
	}
}

func TestExcludeIsIdempotent(t *testing.T) {
	repo, err := Open(newTestRepo(t))
	if err != nil {
		t.Fatal(err)
	}
	for range 2 {
		if err := repo.Exclude(".jw.env"); err != nil {
			t.Fatal(err)
		}
	}
	data, _ := os.ReadFile(filepath.Join(repo.CommonDir, "info", "exclude"))
	if n := strings.Count(string(data), ".jw.env"); n != 1 {
		t.Fatalf("want 1 line, got %d:\n%s", n, data)
	}
}

func TestProjectName(t *testing.T) {
	cases := map[string]string{
		"git@github.com:acme/myapp.git":  "myapp",
		"https://github.com/acme/myapp":  "myapp",
		"https://github.com/acme/myapp/": "myapp",
		"/tmp/origins/myapp.git":         "myapp",
	}
	for in, want := range cases {
		if got := ProjectName(in); got != want {
			t.Errorf("ProjectName(%q) = %q, want %q", in, got, want)
		}
	}
}
