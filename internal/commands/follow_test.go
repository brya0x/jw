package commands

import (
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/connectors/git"
)

func gitIn(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-c", "user.name=t", "-c", "user.email=t@t"}, args...)...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

// The drivecentric case: the stream was created on feat/web, someone switched
// the worktree to fix/web, committed, pushed, and that PR got merged.
func TestDoneFollowsTheBranchTheWorktreeIsOn(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	gitIn(t, e.Path, "checkout", "-q", "-b", "fix/web")
	gitIn(t, e.Path, "commit", "-q", "--allow-empty", "-m", "the real work")
	gitIn(t, e.Path, "push", "-q", "-u", "origin", "fix/web")
	head, _ := git.Head(e.Path)
	env.app.PRs = fakePRs{&connectors.PR{Number: 1223, State: "MERGED", Branch: "fix/web", HeadSHA: head}}
	env.yes = true
	t.Chdir(e.Path)

	if code := env.app.Run([]string{"done"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, env.out)
	}
	if !strings.Contains(env.out.String(), "branch is now fix/web (was feat/web)") {
		t.Errorf("should say it followed the branch:\n%s", env.out)
	}
	// Both are gone: the merged one, and the empty one jw first created.
	for _, b := range []string{"fix/web", "feat/web"} {
		if p.repo.BranchExists(b) {
			t.Errorf("branch %s still exists", b)
		}
	}
}

func TestLsShowsTheBranchTheWorktreeIsOn(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	gitIn(t, e.Path, "checkout", "-q", "-b", "fix/web")

	rows, err := env.app.loadRows(p.name)
	if err != nil {
		t.Fatal(err)
	}
	if rows[0].Entry.Branch != "fix/web" || rows[0].Entry.Original != "feat/web" {
		t.Fatalf("row %+v", rows[0].Entry)
	}
}

func TestDoneSpotsAReusedBranchName(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	gitIn(t, e.Path, "commit", "-q", "--allow-empty", "-m", "new work")
	// The only PR with this head name was merged long before the stream.
	env.app.PRs = fakePRs{&connectors.PR{Number: 524, State: "MERGED", Branch: e.Branch,
		HeadSHA: "0000000000000000000000000000000000000000", Merged: e.Created.Add(-250 * 24 * time.Hour)}}

	err := env.app.done(p, e)
	if err == nil || !strings.Contains(err.Error(), "branch name was used before") {
		t.Fatalf("want the reused-name explanation, got %v", err)
	}
}

func TestDetachedHeadKeepsTheRegisteredBranch(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	gitIn(t, e.Path, "checkout", "-q", "--detach")

	if err := env.app.follow(p, e); err != nil {
		t.Fatal(err)
	}
	if e.Branch != "feat/web" || e.Original != "" {
		t.Fatalf("entry changed on a detached HEAD: %+v", e)
	}
}
