package commands

import (
	"errors"
	"flag"
	"fmt"
	"strings"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/core/registry"
)

func (a *App) runClose(args []string) error {
	name, args := splitName(args)
	fs := flag.NewFlagSet("close", flag.ExitOnError)
	yes := fs.Bool("y", false, "close even if a dev server is running, without asking")
	fs.Parse(args)

	p, e, err := open(name)
	if err != nil {
		return err
	}
	return a.close(p, e, *yes)
}

// close kills the worktree's tab and keeps worktree, branch and slot.
func (a *App) close(p *project, e *registry.Entry, yes bool) error {
	if e.Tab == "" {
		a.printf("%s is already closed\n", e.Name)
		return nil
	}
	mux, err := a.NewMux()
	if err != nil {
		return err
	}
	closed, err := a.closeTab(mux, e, yes)
	if err != nil || !closed {
		return err
	}

	e.Tab = ""
	if err := p.save(); err != nil {
		return err
	}
	a.printf("closed %s — worktree kept, `jw open %s` brings it back\n", e.Name, e.Name)
	return nil
}

// closeTab closes e's tab, asking first if its dev pane is running
// something. It reports false when the user says no.
func (a *App) closeTab(mux connectors.Multiplexer, e *registry.Entry, yes bool) (bool, error) {
	tab, err := mux.GetTab(e.Tab)
	if errors.Is(err, connectors.ErrNotFound) {
		return true, nil // already gone
	}
	if err != nil {
		return false, err
	}

	if !yes {
		running, err := devProcesses(mux, tab)
		if err != nil {
			return false, err
		}
		if len(running) > 0 {
			a.printf("the dev pane of %s is running: %s\n", e.Name, strings.Join(running, "; "))
			ok, err := a.confirm("close it anyway?")
			if err != nil {
				return false, fmt.Errorf("%w (pass -y to close without asking)", err)
			}
			if !ok {
				a.printf("left open\n")
				return false, nil
			}
		}
	}
	return true, mux.CloseTab(tab.ID)
}

// devProcesses returns what runs in the tab's dev panes besides the shell.
func devProcesses(mux connectors.Multiplexer, tab connectors.Tab) ([]string, error) {
	panes, err := mux.PanesInTab(tab)
	if err != nil {
		return nil, err
	}
	var running []string
	for _, p := range panes {
		if p.Label != "dev" {
			continue
		}
		procs, err := mux.Foreground(p.ID)
		if err != nil {
			return nil, err
		}
		for _, proc := range procs {
			if !isShell(proc.Name) {
				running = append(running, proc.Cmdline)
			}
		}
	}
	return running, nil
}

func isShell(name string) bool {
	switch strings.TrimPrefix(name, "-") { // login shells show up as "-zsh"
	case "zsh", "bash", "sh", "fish", "dash", "nu", "ksh", "cmd.exe", "powershell.exe", "pwsh.exe":
		return true
	}
	return false
}
