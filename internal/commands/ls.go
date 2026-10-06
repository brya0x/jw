package commands

import (
	"flag"
	"fmt"
	"os"
	"text/tabwriter"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/registry"
)

// lsRow is one worktree as jw ls shows it, reconciled with the multiplexer,
// git and the forge.
type lsRow struct {
	Entry registry.Entry
	Tab   string // "open" or "closed"
	PR    string // "#12 merged", "-" (none) or "?" (PR state unavailable)
	PRURL string
	State string // "missing", "dirty", "ready for done" or ""

	pr      *connectors.PR // nil: no PR, or PR state unavailable (see PR)
	unknown bool           // PR state couldn't be read
}

func (a *App) runLs(args []string) error {
	fs := flag.NewFlagSet("ls", flag.ExitOnError)
	all := fs.Bool("a", false, "show worktrees of every project")
	interactive := fs.Bool("i", false, "interactive: pick a worktree and open, close or finish it")
	asJSON := fs.Bool("json", false, "print the streams as JSON (for scripts and agents)")
	fs.Parse(args)

	dir, err := os.Getwd()
	if err != nil {
		return err
	}
	project := currentProject(dir)

	if *interactive {
		if !a.Shell.IsTerminal(os.Stdout) || !a.Shell.IsTerminal(a.In) {
			return fmt.Errorf("jw ls -i needs a terminal: %w", ErrNeedsHuman)
		}
		return a.lsInteractive(project, *all)
	}

	if *all {
		project = ""
	}
	rows, err := a.loadRows(project)
	if err != nil {
		return err
	}
	if *asJSON {
		return a.writeJSON(a.streamsJSON(rows))
	}
	if len(rows) == 0 {
		a.printf("no worktrees yet — create one with `jw new <name>`\n")
		return nil
	}

	w := tabwriter.NewWriter(a.Out, 0, 0, 2, ' ', 0)
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

// currentProject is the project of the repo dir is in, or "" outside one.
func currentProject(dir string) string {
	if repo, err := git.Open(dir); err == nil {
		return git.ProjectName(repo.Remote)
	}
	return ""
}

// loadRows reads the registry entries of project ("" for every project) and
// reconciles each one with the multiplexer, git and the forge.
func (a *App) loadRows(project string) ([]lsRow, error) {
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

	// Entries follow the branch their worktree is on now (see follow).
	followed := false
	for _, e := range entries {
		if _, err := os.Stat(e.Path); err != nil {
			continue
		}
		if cur, err := git.CurrentBranch(e.Path); err == nil && cur != "" && cur != e.Branch {
			if e.Original == "" && !e.Adopted {
				e.Original = e.Branch
			}
			e.Branch = cur
			followed = true
		}
	}
	if followed {
		if err := reg.Save(path); err != nil {
			return nil, err
		}
	}

	// What was closed outside jw is forgotten here, so the registry never
	// keeps pointing at a dead tab.
	changed := false
	for _, e := range entries {
		if e.Tab == "" {
			continue
		}
		if b, err := a.backendFor(e); err == nil && b.Prune(e) {
			changed = true
		}
	}
	if changed {
		if err := reg.Save(path); err != nil {
			return nil, err
		}
	}

	prs := a.prsByProject(entries)
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
			r.pr = &pr
		case prs[e.Project] == nil:
			r.PR, r.unknown = "?", true
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

// prsByProject asks once per project, from any of its worktrees that still
// exists. A project the forge can't answer for is left out (shown as "?").
func (a *App) prsByProject(entries []*registry.Entry) map[string]map[string]connectors.PR {
	dirs := map[string]string{}
	for _, e := range entries {
		if _, seen := dirs[e.Project]; seen {
			continue
		}
		if _, err := os.Stat(e.Path); err == nil {
			dirs[e.Project] = e.Path
		}
	}

	out := map[string]map[string]connectors.PR{}
	for project, dir := range dirs {
		if prs, err := a.PRs.ByBranch(dir); err == nil {
			out[project] = prs
		}
	}
	return out
}
