package commands

import (
	"errors"
	"fmt"
	"os"

	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/registry"
)

func (a *App) runDone(args []string) error {
	name, _ := splitName(args)
	p, e, err := open(name)
	if err != nil {
		return err
	}
	return a.done(p, e)
}

// done removes a finished stream: once its PR is merged and nothing local
// would be lost, it asks, then deletes tab, worktree, branch and entry.
func (a *App) done(p *project, e *registry.Entry) error {
	// 0. Not from inside the stream's own tab: closing it would kill jw
	// before the worktree is gone.
	if e.Tab != "" {
		if mux, err := a.NewMux(); err == nil && insideOwnTab(mux, e) {
			return refuseFromOwnTab(e, "done")
		}
	}

	_, statErr := os.Stat(e.Path)
	onDisk := statErr == nil

	// 1. Nothing uncommitted.
	if onDisk {
		dirty, err := git.Dirty(e.Path)
		if err != nil {
			return err
		}
		if dirty {
			return fmt.Errorf("%s has uncommitted changes — commit or discard them first", e.Name)
		}
	}

	// 2. The PR is merged.
	pr, err := a.PRs.ForBranch(p.repo.Root, e.Branch)
	if err != nil {
		return err
	}
	if pr == nil {
		return fmt.Errorf("no pull request for %s", e.Branch)
	}
	if pr.State != "MERGED" {
		return fmt.Errorf("PR #%d is %s, not merged yet (%s)", pr.Number, pr.Status(), pr.URL)
	}

	// 3. Nothing unpushed: the local HEAD must be part of what was merged.
	// Comparing against the PR's head (not an upstream branch) keeps this
	// working after the forge deletes the remote branch on merge.
	if onDisk {
		head, err := git.Head(e.Path)
		if err != nil {
			return err
		}
		if head != pr.HeadSHA {
			if !p.repo.HasCommit(pr.HeadSHA) {
				_ = p.repo.FetchCommit(pr.HeadSHA)
			}
			if !p.repo.IsAncestor(head, pr.HeadSHA) {
				return fmt.Errorf("%s has commits that are not in PR #%d — push them or open another PR", e.Name, pr.Number)
			}
		}
	}

	// 4. Ask. Nothing is deleted without a yes.
	a.printf("%s: PR #%d merged — %s\n", e.Name, pr.Number, pr.URL)
	if !onDisk {
		a.printf("  (worktree %s is already gone)\n", e.Path)
	}
	ok, err := a.confirm(fmt.Sprintf("delete worktree %s and branch %s?", e.Path, e.Branch))
	if err != nil {
		return err
	}
	if !ok {
		a.printf("kept\n")
		return nil
	}

	// 5. Delete, tab first so no process holds the directory.
	if e.Tab != "" {
		if mux, err := a.NewMux(); err == nil {
			if _, err := a.closeTab(mux, e, true, nil); err != nil {
				return err
			}
		}
	}
	if onDisk {
		if err := p.repo.RemoveWorktree(e.Path); err != nil {
			return err
		}
	}
	if p.repo.BranchExists(e.Branch) {
		if err := p.repo.DeleteBranch(e.Branch); err != nil {
			return err
		}
	}

	// e points into reg.Entries, and Remove shifts that slice: after it, e
	// would point at whatever entry moved into its place. Copy first.
	removed := *e
	p.reg.Remove(removed.ID)
	if err := p.save(); err != nil {
		return errors.Join(fmt.Errorf("deleted %s but could not update the registry", removed.Name), err)
	}
	a.printf("done: %s removed, slot %d free\n", removed.Name, removed.Slot)
	return nil
}
