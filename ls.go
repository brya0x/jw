package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"text/tabwriter"

	"golang.org/x/term"

	"github.com/brya0x/jw/internal/gh"
	"github.com/brya0x/jw/internal/git"
	"github.com/brya0x/jw/internal/herdr"
	"github.com/brya0x/jw/internal/registry"
)

// lsRow is one worktree as jw ls shows it, reconciled with herdr, git and gh.
type lsRow struct {
	Entry registry.Entry
	Tab   string // "open" or "closed"
	PR    string // "#12 merged", "-" (none) or "?" (gh unavailable)
	PRURL string
	State string // "missing", "dirty", "ready for done" or ""
}

func runLs(args []string) error {
	fs := flag.NewFlagSet("ls", flag.ExitOnError)
	all := fs.Bool("a", false, "show worktrees of every project")
	interactive := fs.Bool("i", false, "interactive: pick a worktree and open, close or finish it")
	fs.Parse(args)

	if *interactive {
		if !term.IsTerminal(int(os.Stdout.Fd())) || !term.IsTerminal(int(os.Stdin.Fd())) {
			return errors.New("jw ls -i needs a terminal")
		}
		return runLsInteractive(currentProject(), *all)
	}

	project := ""
	if !*all {
		project = currentProject()
	}
	rows, err := loadRows(project)
	if err != nil {
		return err
	}
	if len(rows) == 0 {
		fmt.Println("no worktrees yet — create one with `jw new <name>`")
		return nil
	}

	w := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
	showProject := project == ""
	if showProject {
		fmt.Fprint(w, "PROJECT\t")
	}
	fmt.Fprintln(w, "NAME\tID\tBRANCH\tSLOT\tTAB\tPR\tSTATE")
	for _, r := range rows {
		if showProject {
			fmt.Fprintf(w, "%s\t", r.Entry.Project)
		}
		fmt.Fprintf(w, "%s\t%s\t%s\t%d\t%s\t%s\t%s\n",
			r.Entry.Name, r.Entry.ID[:8], r.Entry.Branch, r.Entry.Slot, r.Tab, r.PR, r.State)
	}
	return w.Flush()
}

// currentProject is the project of the repo the cwd is in, or "" outside one.
func currentProject() string {
	if cwd, err := os.Getwd(); err == nil {
		if repo, err := git.Open(cwd); err == nil {
			return git.ProjectName(repo.Remote)
		}
	}
	return ""
}

// loadRows reads the registry entries of project ("" for every project) and
// reconciles each one with herdr, git and gh.
func loadRows(project string) ([]lsRow, error) {
	path, err := registry.DefaultPath()
	if err != nil {
		return nil, err
	}
	reg, err := registry.Load(path)
	if err != nil {
		return nil, err
	}

	var entries []*registry.Entry
	for i := range reg.Entries {
		if project == "" || reg.Entries[i].Project == project {
			entries = append(entries, &reg.Entries[i])
		}
	}

	// A tab closed by hand is forgotten here, so the registry never keeps
	// pointing at a dead tab.
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
				return nil, err
			}
		}
	}

	prs := prsByProject(entries)
	rows := make([]lsRow, 0, len(entries))
	for _, e := range entries {
		r := lsRow{Entry: *e, Tab: "closed", PR: "-"}
		if e.Tab != "" {
			r.Tab = "open"
		}

		pr, hasPR := prs[e.Project][e.Branch]
		switch {
		case hasPR:
			r.PR, r.PRURL = pr.Label(), pr.URL
		case prs[e.Project] == nil:
			r.PR = "?"
		}

		if _, err := os.Stat(e.Path); err != nil {
			r.State = "missing"
		} else if dirty, err := git.Dirty(e.Path); err == nil && dirty {
			r.State = "dirty"
		} else if hasPR && pr.State == "MERGED" {
			r.State = "ready for done"
		}
		rows = append(rows, r)
	}
	return rows, nil
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
