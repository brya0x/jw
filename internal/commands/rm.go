package commands

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/registry"
)

type rmOptions struct {
	force      bool // go ahead even though local work would be lost (still asks)
	keepBranch bool // keep the local branch
}

func (a *App) runRm(args []string) error {
	name, args := splitName(args)
	var o rmOptions
	fs := flag.NewFlagSet("rm", flag.ExitOnError)
	fs.BoolVar(&o.force, "force", false, "remove even with uncommitted or unpushed work (still asks)")
	fs.BoolVar(&o.keepBranch, "keep-branch", false, "keep the local branch")
	fs.Parse(args)

	p, e, err := a.target(name)
	if err != nil {
		return err
	}
	return a.rm(p, e, o)
}

// rm removes a stream whatever its PR says: abandoned work, a PR closed
// unmerged, a worktree deleted by hand. It is jw done without the merged
// requirement, so it guards what done's checks would have: local work.
func (a *App) rm(p *project, e *registry.Entry, o rmOptions) error {
	// 0. Not from inside the stream's own tab: closing it would kill jw
	// before the worktree is gone.
	if e.Tab != "" {
		if b, err := a.backendFor(e); err == nil && b.InsideOwn(e) {
			return refuseFromOwnTab(e, "rm")
		}
	}

	_, statErr := os.Stat(e.Path)
	onDisk := statErr == nil

	// 1. What would be lost.
	var dirty, unpushed []string
	if onDisk {
		var err error
		if dirty, err = git.DirtyFiles(e.Path); err != nil {
			return err
		}
		if unpushed, err = git.Unpushed(e.Path); err != nil {
			return err
		}
	}
	keepBranch := o.keepBranch || e.Adopted

	a.printf("%s  (%s, slot %d)\n", e.Name, e.Branch, e.Slot)
	if !onDisk {
		a.printf("  worktree %s is already gone\n", e.Path)
	}
	if len(dirty) > 0 {
		a.printf("  %d uncommitted file(s) will be lost:\n", len(dirty))
		printSome(a, dirty)
	}
	if len(unpushed) > 0 {
		a.printf("  %d commit(s) exist only on this machine:\n", len(unpushed))
		printSome(a, unpushed)
	}

	// Work that only exists here needs --force even to be asked about.
	losesWork := len(dirty) > 0 || (len(unpushed) > 0 && !keepBranch)
	if losesWork && !o.force {
		return errors.New("refusing: that work would be lost (commit and push it, or pass --force)")
	}

	// 2. Ask. Nothing is removed without a yes.
	what := []string{}
	if onDisk {
		what = append(what, "worktree "+e.Path)
	}
	if !keepBranch && p.repo.BranchExists(e.Branch) {
		what = append(what, "local branch "+e.Branch)
	}
	question := fmt.Sprintf("remove %s?", e.Name)
	if len(what) > 0 {
		question = fmt.Sprintf("remove %s: delete %s?", e.Name, strings.Join(what, " and "))
	}
	ok, err := a.confirm(question)
	if err != nil {
		return err
	}
	if !ok {
		a.printf("kept\n")
		return nil
	}

	// 3. Remove, tab first so no process holds the directory.
	if e.Tab != "" {
		if b, err := a.backendFor(e); err == nil {
			if _, err := b.Close(e, true, nil); err != nil {
				return err
			}
		}
	}
	if onDisk {
		if err := p.repo.RemoveWorktree(e.Path); err != nil {
			return err
		}
	} else if err := p.repo.PruneWorktrees(); err != nil {
		return err
	}
	if !keepBranch && p.repo.BranchExists(e.Branch) {
		if err := p.repo.DeleteBranch(e.Branch); err != nil {
			return err
		}
	}
	// The branch jw first created, if the worktree moved off it: only if
	// git sees it merged.
	if !o.keepBranch && e.Original != "" && e.Original != e.Branch && p.repo.BranchExists(e.Original) {
		if err := p.repo.DeleteMergedBranch(e.Original); err != nil {
			a.warnf("note: kept branch %s (git says it isn't merged)\n", e.Original)
		}
	}

	removed := *e // Remove shifts reg.Entries; e would point elsewhere
	p.reg.Remove(removed.ID)
	if err := p.save(); err != nil {
		return errors.Join(fmt.Errorf("removed %s but could not update the registry", removed.Name), err)
	}

	note := ""
	if keepBranch {
		note = fmt.Sprintf(", branch %s kept", removed.Branch)
	}
	a.printf("removed %s%s, slot %d free (the remote branch is untouched)\n", removed.Name, note, removed.Slot)
	return nil
}

// printSome prints up to five lines, then how many more there are.
func printSome(a *App, lines []string) {
	for i, l := range lines {
		if i == 5 {
			a.printf("    … and %d more\n", len(lines)-5)
			return
		}
		a.printf("    %s\n", strings.TrimSpace(l))
	}
}
