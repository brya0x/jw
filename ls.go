package main

import (
	"flag"
	"fmt"
	"os"
	"text/tabwriter"

	"github.com/brya0x/jw/internal/git"
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

	var entries []registry.Entry
	for _, e := range reg.Entries {
		if project == "" || e.Project == project {
			entries = append(entries, e)
		}
	}

	if len(entries) == 0 {
		fmt.Println("no worktrees yet — create one with `jw new <name>`")
		return nil
	}

	w := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
	fmt.Fprintln(w, "PROJECT\tNAME\tID\tBRANCH\tSLOT\tTAB")
	for _, e := range entries {
		tab := "closed"
		if e.Tab != "" {
			tab = "open"
		}
		fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%d\t%s\n", e.Project, e.Name, e.ID[:8], e.Branch, e.Slot, tab)
	}
	return w.Flush()
}
