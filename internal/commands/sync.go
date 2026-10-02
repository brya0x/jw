package commands

import (
	"flag"
	"fmt"
	"os"

	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/registry"
)

type syncOptions struct {
	merge bool // merge the base in instead of the config's default (rebase)
}

func (a *App) runSync(args []string) error {
	name, args := splitName(args)
	var o syncOptions
	fs := flag.NewFlagSet("sync", flag.ExitOnError)
	fs.BoolVar(&o.merge, "merge", false, "merge the base branch in instead of rebasing onto it")
	fs.Parse(args)

	p, e, err := a.target(name)
	if err != nil {
		return err
	}
	return a.sync(p, e, o)
}

// sync brings a stream's branch up to date with the project's base branch.
// It never pushes: publishing (and force-pushing after a rebase) stays a
// human decision.
func (a *App) sync(p *project, e *registry.Entry, o syncOptions) error {
	if _, err := os.Stat(e.Path); err != nil {
		return fmt.Errorf("%s: worktree %s is missing", e.Name, e.Path)
	}

	// Refuse what git would refuse, but say how to get out of it.
	if op := git.Operation(e.Path); op != "" {
		return fmt.Errorf("%s has a %s in progress: finish it (resolve, git add, git %s --continue) or abort it (git %s --abort)",
			e.Name, op, op, op)
	}
	dirty, err := git.DirtyFiles(e.Path)
	if err != nil {
		return err
	}
	if len(dirty) > 0 {
		return fmt.Errorf("%s has uncommitted changes — commit them first", e.Name)
	}

	mode := p.cfg.Sync
	if o.merge {
		mode = "merge"
	}
	baseBranch, err := p.repo.DefaultBranch()
	if err != nil {
		return err
	}
	base := "origin/" + baseBranch

	a.printf("fetching origin…\n")
	if err := p.repo.Fetch(); err != nil {
		return err
	}
	behind, ahead, err := git.Divergence(e.Path, base)
	if err != nil {
		return err
	}
	a.printf("%s: %d behind, %d ahead of %s\n", e.Name, behind, ahead, base)
	if behind == 0 {
		a.printf("already up to date\n")
		return nil
	}

	// A rebase rewrites the stream's commits; if they are already on origin,
	// publishing afterwards needs a force push. Know that before starting.
	published := p.repo.RemoteBranchExists(e.Branch)

	if mode == "merge" {
		a.printf("merging %s into %s…\n", base, e.Branch)
		err = git.Merge(e.Path, base)
	} else {
		a.printf("rebasing %s onto %s…\n", e.Branch, base)
		err = git.Rebase(e.Path, base)
	}
	if err != nil {
		return a.syncStopped(p, e, mode, err)
	}

	switch {
	case mode == "merge":
		a.printf("done: merged %d commit(s) from %s. Publish with `git push`.\n", behind, base)
	case published && ahead > 0:
		a.printf("done: %d commit(s) replayed onto %s. %s was already pushed: publish with `git push --force-with-lease`.\n",
			ahead, base, e.Branch)
	default:
		a.printf("done: %d commit(s) replayed onto %s.\n", ahead, base)
	}
	return nil
}

// syncStopped explains a rebase or merge that stopped on conflicts, and
// leaves it in place for the stream's agent (or you) to resolve.
func (a *App) syncStopped(p *project, e *registry.Entry, mode string, cause error) error {
	files, err := git.Conflicts(e.Path)
	if err != nil || len(files) == 0 {
		return fmt.Errorf("%s failed: %w", mode, cause)
	}
	a.printf("stopped: %d file(s) in conflict:\n", len(files))
	for _, f := range files {
		a.printf("  %s\n", f)
	}
	a.printf("resolve them, `git add` each one, then `git %s --continue` — or `git %s --abort` to undo.\n", mode, mode)
	if name, err := p.agentName(e); err == nil && e.Tab != "" {
		a.printf("the stream's agent can do it: herdr agent prompt %s \"resolve the %s conflicts\"\n", shellArg(name), mode)
	}
	return fmt.Errorf("%s stopped at conflicts", mode)
}
