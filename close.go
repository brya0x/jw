package main

import (
	"bufio"
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"

	"golang.org/x/term"

	"github.com/brya0x/jw/internal/herdr"
	"github.com/brya0x/jw/internal/registry"
)

// runClose is `jw close [name]`: kill the tab, keep the worktree, branch and slot.
func runClose(args []string) error {
	name, args := splitName(args)
	fs := flag.NewFlagSet("close", flag.ExitOnError)
	yes := fs.Bool("y", false, "close even if a dev server is running, without asking")
	fs.Parse(args)

	p, err := openProject()
	if err != nil {
		return err
	}
	e, err := p.entry(nameArgs(name))
	if err != nil {
		return err
	}
	if e.Tab == "" {
		fmt.Printf("%s is already closed\n", e.Name)
		return nil
	}

	h, err := herdr.New()
	if err != nil {
		return err
	}
	closed, err := closeTab(h, e, *yes)
	if err != nil || !closed {
		return err
	}

	e.Tab = ""
	if err := p.reg.Save(p.regPath); err != nil {
		return err
	}
	fmt.Printf("closed %s — worktree kept, `jw open %s` brings it back\n", e.Name, e.Name)
	return nil
}

// closeTab closes e's tab, asking first if its dev pane is running
// something. It reports false when the user says no.
func closeTab(h *herdr.Client, e *registry.Entry, yes bool) (bool, error) {
	tab, err := h.GetTab(e.Tab)
	if herdr.IsNotFound(err) {
		return true, nil // already gone
	}
	if err != nil {
		return false, err
	}

	if !yes {
		running, err := devProcesses(h, tab)
		if err != nil {
			return false, err
		}
		if len(running) > 0 {
			fmt.Printf("the dev pane of %s is running: %s\n", e.Name, strings.Join(running, "; "))
			ok, err := confirm("close it anyway?")
			if err != nil {
				return false, fmt.Errorf("%w (pass -y to close without asking)", err)
			}
			if !ok {
				fmt.Println("left open")
				return false, nil
			}
		}
	}
	return true, h.CloseTab(tab.ID)
}

// devProcesses returns what runs in the tab's dev pane besides the shell.
func devProcesses(h *herdr.Client, tab herdr.Tab) ([]string, error) {
	panes, err := h.PanesInTab(tab)
	if err != nil {
		return nil, err
	}
	var running []string
	for _, p := range panes {
		if p.Label != "dev" {
			continue
		}
		procs, err := h.Foreground(p.ID)
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
	case "zsh", "bash", "sh", "fish", "dash", "nu", "ksh":
		return true
	}
	return false
}

// confirm asks a yes/no question on the terminal; anything but y/yes is no.
// Without a terminal there is nobody to answer, so it refuses.
func confirm(question string) (bool, error) {
	// Not os.ModeCharDevice: /dev/null is a character device too.
	if !term.IsTerminal(int(os.Stdin.Fd())) {
		return false, errors.New("no terminal to confirm on")
	}
	fmt.Printf("%s [y/N] ", question)
	line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
	answer := strings.ToLower(strings.TrimSpace(line))
	return answer == "y" || answer == "yes", nil
}
