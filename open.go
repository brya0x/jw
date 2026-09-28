package main

import (
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/brya0x/jw/internal/config"
	"github.com/brya0x/jw/internal/herdr"
	"github.com/brya0x/jw/internal/registry"
)

// agentSpec is one agent to start: its herdr name, kind and arguments.
type agentSpec struct {
	name string
	kind string
	args []string
}

func runOpen(args []string) error {
	name, args := splitName(args)
	fs := flag.NewFlagSet("open", flag.ExitOnError)
	agent := fs.String("agent", "", "claude, codex or both (default from config)")
	noFocus := fs.Bool("no-focus", false, "don't switch to the tab (for scripts and other agents)")
	fs.Parse(args)

	p, err := openProject()
	if err != nil {
		return err
	}
	e, err := p.entry(nameArgs(name))
	if err != nil {
		return err
	}
	h, err := herdr.New()
	if err != nil {
		return err
	}

	// Idempotent: a live tab is focused, not rebuilt. A tab that was closed
	// by hand is forgotten and recreated.
	if e.Tab != "" {
		_, err := h.GetTab(e.Tab)
		switch {
		case err == nil:
			if !*noFocus {
				if err := h.FocusTab(e.Tab); err != nil {
					return err
				}
			}
			fmt.Printf("%s is already open in %s\n", e.Name, e.Tab)
			return nil
		case herdr.IsNotFound(err):
			e.Tab = ""
		default:
			return err
		}
	}

	if *agent == "" {
		*agent = p.cfg.Agent.Default
	}
	agents, err := agentsFor(p.cfg, *agent, e)
	if err != nil {
		return err
	}
	base, err := p.repo.DefaultBranch()
	if err != nil {
		return err
	}
	vars := p.cfg.Vars(e.Name, base, e.Slot)
	editor, err := config.Expand(p.cfg.Layout.Editor, vars)
	if err != nil {
		return err
	}
	env := jwEnv(*e, vars)

	tab, editorPane, err := createTab(h, p.cfg.Workspace, e, env)
	if err != nil {
		return err
	}

	// Record the tab before anything else can fail, so `jw close` can
	// always find what `jw open` created.
	e.Tab = tab.ID
	if err := p.reg.Save(p.regPath); err != nil {
		return err
	}

	//  ┌────────┬────────┐
	//  │ editor │ agent  │   split the full-width bottom off first, then
	//  ├────────┴────────┤   cut the top in two
	//  │ dev             │
	//  └─────────────────┘
	devPane, err := h.Split(editorPane.ID, "down", 0.7, e.Path, env)
	if err != nil {
		return err
	}
	agentPane, err := h.Split(editorPane.ID, "right", 0.5, e.Path, env)
	if err != nil {
		return err
	}
	_ = h.RenamePane(editorPane.ID, "editor")
	_ = h.RenamePane(devPane.ID, "dev")

	if err := h.Run(editorPane.ID, editor); err != nil {
		return err
	}

	panes := []herdr.Pane{agentPane}
	for range agents[1:] {
		extra, err := h.Split(agentPane.ID, "down", 0.5, e.Path, env)
		if err != nil {
			return err
		}
		panes = append(panes, extra)
	}

	// An agent that fails to start doesn't undo the tab: the editor and dev
	// panes are still useful, and the agent can be started by hand.
	for i, a := range agents {
		fmt.Printf("starting %s (%s %s)…\n", a.name, a.kind, strings.Join(a.args, " "))
		err := h.StartAgent(a.name, a.kind, panes[i].ID, a.args)
		switch {
		case err == nil:
		case herdr.IsNotReady(err):
			// jw never answers these: a trust or approval dialog is the user's call.
			fmt.Fprintf(os.Stderr, "note: %s is waiting at a dialog in its pane (a new folder asks whether you trust it) — answer it there\n", a.name)
		default:
			fmt.Fprintf(os.Stderr, "warning: %s did not start: %v\n", a.name, err)
		}
	}

	e.Opened = true
	if err := p.reg.Save(p.regPath); err != nil {
		return err
	}

	if !*noFocus {
		if err := h.FocusTab(tab.ID); err != nil {
			return err
		}
	}
	fmt.Printf("opened %s in %s\n", e.Name, tab.ID)
	return nil
}

// createTab puts the worktree's tab in the project's workspace, creating the
// workspace on first use (and reusing the tab herdr makes along with it).
func createTab(h *herdr.Client, workspace string, e *registry.Entry, env []string) (herdr.Tab, herdr.Pane, error) {
	all, err := h.Workspaces()
	if err != nil {
		return herdr.Tab{}, herdr.Pane{}, err
	}
	for _, ws := range all {
		if ws.Label == workspace {
			return h.CreateTab(ws.ID, e.Path, e.Name, env)
		}
	}

	_, tab, root, err := h.CreateWorkspace(e.Path, workspace, env)
	if err != nil {
		return tab, root, err
	}
	return tab, root, h.RenameTab(tab.ID, e.Name)
}

// agentsFor picks the agent command(s). A worktree that was opened before
// resumes its conversation instead of starting a new one.
func agentsFor(cfg *config.Config, which string, e *registry.Entry) ([]agentSpec, error) {
	pick := func(c config.AgentCmd, name string) (agentSpec, error) {
		line := c.Start
		if e.Opened {
			line = c.Resume
		}
		fields := strings.Fields(line)
		if len(fields) == 0 {
			return agentSpec{}, fmt.Errorf("empty agent command for %s", name)
		}
		return agentSpec{name: name, kind: fields[0], args: fields[1:]}, nil
	}

	switch which {
	case "claude", "codex":
		c := cfg.Agent.Claude
		if which == "codex" {
			c = cfg.Agent.Codex
		}
		a, err := pick(c, e.Name)
		return []agentSpec{a}, err
	case "both":
		claude, err := pick(cfg.Agent.Claude, e.Name)
		if err != nil {
			return nil, err
		}
		codex, err := pick(cfg.Agent.Codex, e.Name+"-codex")
		return []agentSpec{claude, codex}, err
	default:
		return nil, fmt.Errorf("unknown agent %q: use claude, codex or both", which)
	}
}
