package commands

import (
	"flag"
	"fmt"

	"github.com/brya0x/jw/internal/core/registry"
)

func (a *App) runClose(args []string) error {
	name, args := splitName(args)
	fs := flag.NewFlagSet("close", flag.ExitOnError)
	yes := fs.Bool("y", false, "close even if a dev server is running, without asking")
	fs.Parse(args)

	p, e, err := a.target(name)
	if err != nil {
		return err
	}
	return a.close(p, e, *yes)
}

// close tears the stream down and keeps worktree, branch and slot.
func (a *App) close(p *project, e *registry.Entry, yes bool) error {
	if e.Tab == "" {
		a.printf("%s is already closed\n", e.Name)
		return nil
	}
	b, err := a.backendFor(e)
	if err != nil {
		return err
	}
	closed, err := b.Close(e, yes, p.save)
	if err != nil || !closed {
		return err
	}
	a.printf("closed %s — worktree kept, `jw open %s` brings it back\n", e.Name, e.Name)
	return nil
}

// refuseFromOwnTab is the error done and rm give inside the stream's tab.
func refuseFromOwnTab(e *registry.Entry, cmd string) error {
	return fmt.Errorf("you're inside %s's own tab: %s closes it first, which would kill this shell half-way — run `jw %s %s` from another tab",
		e.Name, cmd, cmd, e.Name)
}
