package main

import (
	"flag"
	"fmt"
	"os"
	"text/tabwriter"

	"github.com/brya0x/jw/internal/gh"
	"github.com/brya0x/jw/internal/git"
	"github.com/brya0x/jw/internal/herdr"
	"github.com/brya0x/jw/internal/registry"
)

func runLs(args []string) error {
	fs := flag.NewFlagSet("ls", flag.ExitOnError)
	all := fs.Bool("a", false, "show worktrees of every project")
	fs.Parse(args)

	path, err := registry.DefaultPath()
	if err != nil {
		return err
	}
	reg, err := registry.Load(path)
	if err != nil {
		return err
	}

	// Outside a git repo there is no current project, so show everything.
	project := ""
	if !*all {
		if cwd, err := os.Getwd(); err == nil {
			if repo, err := git.Open(cwd); err == nil {
				project = git.ProjectName(repo.Remote)
			}
		}
	}

	var entries []*registry.Entry
	for i := range reg.Entries {
		if project == "" || reg.Entries[i].Project == project {
			entries = append(entries, &reg.Entries[i])
		}
	}
	if len(entries) == 0 {
		fmt.Println("no worktrees yet — create one with `jw new <name>`")
		return nil
	}

	// Reconcile with reality: a tab closed by hand is forgotten here, so the
	// registry never keeps pointing at a dead tab.
	if h, err := herdr.New(); err == nil {
		changed := false
		for _, e := range entries {
			if e.Tab == "" {
				continue
			}
			if _, err := h.GetTab(e.Tab); herdr.IsNotFound(err) {
				e.Tab = ""
				changed = true
			}
		}
		if changed {
			if err := reg.Save(path); err != nil {
				return err
			}
		}
	}

	prs := prsByProject(entries)

	w := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
	if project == "" {
		fmt.Fprint(w, "PROJECT\t")
	}
	fmt.Fprintln(w, "NAME\tID\tBRANCH\tSLOT\tTAB\tPR\tSTATE")
	for _, e := range entries {
		tab := "closed"
		if e.Tab != "" {
			tab = "open"
		}

		prLabel := "-"
		pr, hasPR := prs[e.Project][e.Branch]
		if hasPR {
			prLabel = pr.Label()
		} else if prs[e.Project] == nil {
			prLabel = "?" // gh unavailable for this project
		}

		state := ""
		if _, err := os.Stat(e.Path); err != nil {
			state = "missing"
		} else if dirty, err := git.Dirty(e.Path); err == nil && dirty {
			state = "dirty"
		} else if hasPR && pr.State == "MERGED" {
			state = "ready for done"
		}

		if project == "" {
			fmt.Fprintf(w, "%s\t", e.Project)
		}
		fmt.Fprintf(w, "%s\t%s\t%s\t%d\t%s\t%s\t%s\n", e.Name, e.ID[:8], e.Branch, e.Slot, tab, prLabel, state)
	}
	return w.Flush()
}

// prsByProject asks gh once per project, from any of its worktrees that still
// exists. A project gh can't answer for is left out (shown as "?").
func prsByProject(entries []*registry.Entry) map[string]map[string]gh.PR {
	dirs := map[string]string{}
	for _, e := range entries {
		if _, seen := dirs[e.Project]; seen {
			continue
		}
		if _, err := os.Stat(e.Path); err == nil {
			dirs[e.Project] = e.Path
		}
	}

	out := map[string]map[string]gh.PR{}
	for project, dir := range dirs {
		if prs, err := gh.ByBranch(dir); err == nil {
			out[project] = prs
		}
	}
	return out
}
