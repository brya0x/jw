package main

import (
	"flag"
	"fmt"
	"os"
	"text/tabwriter"

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

	// TODO: derive the current project from the cwd's git remote; until
	// then `ls` behaves as `ls -a`.
	_ = all

	if len(reg.Entries) == 0 {
		fmt.Println("no worktrees yet — create one with `jw new <name>`")
		return nil
	}

	w := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
	fmt.Fprintln(w, "PROJECT\tNAME\tID\tBRANCH\tSLOT\tTAB")
	for _, e := range reg.Entries {
		tab := "closed"
		if e.Tab != "" {
			tab = "open"
		}
		fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%d\t%s\n", e.Project, e.Name, e.ID[:8], e.Branch, e.Slot, tab)
	}
	return w.Flush()
}
