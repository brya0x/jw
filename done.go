package main

import (
	"errors"
	"fmt"
	"os"

	"github.com/brya0x/jw/internal/gh"
	"github.com/brya0x/jw/internal/git"
	"github.com/brya0x/jw/internal/herdr"
)

// runDone is `jw done [name]`: once the PR is merged and nothing local would
// be lost, ask, then remove tab, worktree, branch and registry entry.
func runDone(args []string) error {
	name, _ := splitName(args)

	p, err := openProject()
	if err != nil {
		return err
	}
	e, err := p.entry(nameArgs(name))
	if err != nil {
		return err
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
	pr, err := gh.ForBranch(p.repo.Root, e.Branch)
	if err != nil {
		return err
	}
	if pr == nil {
		return fmt.Errorf("no pull request for %s", e.Branch)
	}
	if pr.State != "MERGED" {
		return fmt.Errorf("PR #%d is %s, not merged yet (%s)", pr.Number, pr.Status(), pr.URL)
	}

	// 3. Nothing unpushed: the local HEAD must be part of what GitHub
	// merged. Comparing against the PR's head (not an upstream branch) keeps
	// this working after GitHub deletes the remote branch on merge.
	if onDisk {
		head, err := git.Head(e.Path)
		if err != nil {
			return err
		}
		if head != pr.HeadRefOid {
			if !p.repo.HasCommit(pr.HeadRefOid) {
				_ = p.repo.FetchCommit(pr.HeadRefOid)
			}
			if !p.repo.IsAncestor(head, pr.HeadRefOid) {
				return fmt.Errorf("%s has commits that are not in PR #%d — push them or open another PR", e.Name, pr.Number)
			}
		}
	}

	// 4. Ask. Nothing is deleted without a yes.
	fmt.Printf("%s: PR #%d merged — %s\n", e.Name, pr.Number, pr.URL)
	if !onDisk {
		fmt.Printf("  (worktree %s is already gone)\n", e.Path)
	}
	ok, err := confirm(fmt.Sprintf("delete worktree %s and branch %s?", e.Path, e.Branch))
	if err != nil {
		return err
	}
	if !ok {
		fmt.Println("kept")
		return nil
	}

	// 5. Delete, tab first so no process holds the directory.
	if e.Tab != "" {
		if h, err := herdr.New(); err == nil {
			if _, err := closeTab(h, e, true); err != nil {
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
	if err := p.reg.Save(p.regPath); err != nil {
		return errors.Join(fmt.Errorf("deleted %s but could not update the registry", removed.Name), err)
	}
	fmt.Printf("done: %s removed, slot %d free\n", removed.Name, removed.Slot)
	return nil
}
