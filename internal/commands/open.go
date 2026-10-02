package commands

import (
	"errors"
	"flag"
	"fmt"
	"regexp"
	"strings"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/core/config"
	"github.com/brya0x/jw/internal/core/registry"
)

type openOptions struct {
	agent   string // claude, codex or both; default from config
	noFocus bool
	task    string // prompt for the agent once it's up
}

// agentSpec is one agent to start: its multiplexer name, kind and arguments.
type agentSpec struct {
	name string
	kind string
	args []string
}

func (a *App) runOpen(args []string) error {
	name, args := splitName(args)
	var o openOptions
	fs := flag.NewFlagSet("open", flag.ExitOnError)
	fs.StringVar(&o.agent, "agent", "", "claude, codex or both (default from config)")
	fs.BoolVar(&o.noFocus, "no-focus", false, "don't switch to the tab (for scripts and other agents)")
	fs.StringVar(&o.task, "task", "", "hand this task to the stream's agent once it's up")
	fs.Parse(args)

	p, e, err := a.target(name)
	if err != nil {
		return err
	}
	return a.open(p, e, o)
}

// open builds the worktree's tab, or focuses it if it's already there.
func (a *App) open(p *project, e *registry.Entry, o openOptions) error {
	mux, err := a.NewMux()
	if err != nil {
		return err
	}

	// Idempotent: a live tab is focused, not rebuilt. A tab that was closed
	// by hand is forgotten and recreated.
	if e.Tab != "" {
		_, err := mux.GetTab(e.Tab)
		switch {
		case err == nil:
			if !o.noFocus {
				if err := mux.FocusTab(e.Tab); err != nil {
					return err
				}
			}
			a.printf("%s is already open in %s\n", e.Name, e.Tab)
			return a.handTask(p, e, o.task, false)
		case errors.Is(err, connectors.ErrNotFound):
			e.Tab = ""
		default:
			return err
		}
	}

	if o.agent == "" {
		o.agent = p.cfg.Agent.Default
	}
	name, err := p.agentName(e)
	if err != nil {
		return err
	}
	agents, err := agentsFor(p.cfg, o.agent, e, name)
	if err != nil {
		return err
	}
	for _, ag := range agents {
		if !agentName.MatchString(ag.name) {
			return fmt.Errorf("herdr won't name an agent %q: it takes a lowercase letter, then up to 31 of a-z, 0-9, - and _ — shorten the stream name or the workspace label", ag.name)
		}
	}
	vars, err := p.vars(e)
	if err != nil {
		return err
	}
	editor, err := config.Expand(p.cfg.Layout.Editor, vars)
	if err != nil {
		return err
	}
	workspace, err := config.Expand(p.cfg.Workspace, vars)
	if err != nil {
		return err
	}
	env := jwEnv(*e, vars)

	tab, editorPane, err := createTab(mux, workspace, e, env)
	if err != nil {
		return err
	}

	// Record the tab before anything else can fail, so `jw close` can
	// always find what `jw open` created.
	e.Tab = tab.ID
	if err := p.save(); err != nil {
		return err
	}

	//  ┌────────┬────────┐
	//  │ editor │ agent  │   split the full-width bottom off first, then
	//  ├────────┴────────┤   cut the top in two
	//  │ dev             │
	//  └─────────────────┘
	devPane, err := mux.Split(editorPane.ID, "down", 0.7, e.Path, env)
	if err != nil {
		return err
	}
	agentPane, err := mux.Split(editorPane.ID, "right", 0.5, e.Path, env)
	if err != nil {
		return err
	}
	_ = mux.RenamePane(editorPane.ID, "editor")
	_ = mux.RenamePane(devPane.ID, "dev")

	if err := mux.Run(editorPane.ID, editor); err != nil {
		return err
	}

	panes := []connectors.Pane{agentPane}
	for range agents[1:] {
		extra, err := mux.Split(agentPane.ID, "down", 0.5, e.Path, env)
		if err != nil {
			return err
		}
		panes = append(panes, extra)
	}

	// An agent that fails to start doesn't undo the tab: the editor and dev
	// panes are still useful, and the agent can be started by hand.
	for i, ag := range agents {
		a.printf("starting %s (%s %s)…\n", ag.name, ag.kind, strings.Join(ag.args, " "))
		err := mux.StartAgent(ag.name, ag.kind, panes[i].ID, ag.args)
		switch {
		case err == nil:
		case errors.Is(err, connectors.ErrAgentNotReady):
			// jw never answers these: a trust or approval dialog is the user's call.
			a.warnf("note: %s is waiting at a dialog in its pane (a new folder asks whether you trust it) — answer it there\n", ag.name)
		default:
			a.warnf("warning: %s did not start: %v\n", ag.name, err)
		}
	}

	e.Opened = true
	if err := p.save(); err != nil {
		return err
	}
	if !o.noFocus {
		if err := mux.FocusTab(tab.ID); err != nil {
			return err
		}
	}
	a.printf("opened %s in %s\n", e.Name, tab.ID)
	return a.handTask(p, e, o.task, true)
}

// handTask prompts the stream's agent with task, if there is one. fresh:
// the agent was just started.
func (a *App) handTask(p *project, e *registry.Entry, task string, fresh bool) error {
	if task == "" {
		return nil
	}
	return a.prompt(p, e, false, task, fresh)
}

// createTab puts the worktree's tab in the workspace labelled workspace,
// creating it on first use (and reusing the tab it comes with). A label with
// {name} in it gives every stream a workspace of its own.
func createTab(mux connectors.Multiplexer, workspace string, e *registry.Entry, env []string) (connectors.Tab, connectors.Pane, error) {
	all, err := mux.Workspaces()
	if err != nil {
		return connectors.Tab{}, connectors.Pane{}, err
	}
	for _, ws := range all {
		if ws.Label == workspace {
			return mux.CreateTab(ws.ID, e.Path, e.Name, env)
		}
	}

	_, tab, root, err := mux.CreateWorkspace(e.Path, workspace, env)
	if err != nil {
		return tab, root, err
	}
	return tab, root, mux.RenameTab(tab.ID, e.Name)
}

// agentName is what herdr accepts as an agent's name.
var agentName = regexp.MustCompile(`^[a-z][a-z0-9_-]{0,31}$`)

// agentsFor picks the agent command(s). A worktree that was opened before
// resumes its conversation instead of starting a new one. name is the agent's
// herdr name (see project.agentName).
func agentsFor(cfg *config.Config, which string, e *registry.Entry, name string) ([]agentSpec, error) {
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
		ag, err := pick(c, name)
		return []agentSpec{ag}, err
	case "both":
		claude, err := pick(cfg.Agent.Claude, name)
		if err != nil {
			return nil, err
		}
		codex, err := pick(cfg.Agent.Codex, name+"-codex")
		return []agentSpec{claude, codex}, err
	default:
		return nil, fmt.Errorf("unknown agent %q: use claude, codex or both", which)
	}
}
