package commands

import (
	"flag"
	"fmt"
	"strings"

	"github.com/brya0x/jw/internal/backends"
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

// open builds the worktree's stream in its backend, or brings it forward if
// it's already open.
func (a *App) open(p *project, e *registry.Entry, o openOptions) error {
	b, err := a.backendFor(e)
	if err != nil {
		return err
	}
	s, err := a.stream(p, e, o.agent)
	if err != nil {
		return err
	}
	return b.Open(s, backends.OpenOptions{NoFocus: o.noFocus, Task: o.task})
}

// stream resolves what the config says about e into what a backend opens.
func (a *App) stream(p *project, e *registry.Entry, agent string) (backends.Stream, error) {
	if agent == "" {
		agent = p.cfg.Agent.Default
	}
	name, err := p.agentName(e)
	if err != nil {
		return backends.Stream{}, err
	}
	agents, err := agentsFor(p.cfg, agent, e, name)
	if err != nil {
		return backends.Stream{}, err
	}
	vars, err := p.vars(e)
	if err != nil {
		return backends.Stream{}, err
	}
	editor, err := config.Expand(p.cfg.Layout.Editor, vars)
	if err != nil {
		return backends.Stream{}, err
	}
	workspace, err := config.Expand(p.cfg.Workspace, vars)
	if err != nil {
		return backends.Stream{}, err
	}
	s := backends.Stream{Entry: e, Env: jwEnv(*e, vars), Editor: editor, Workspace: workspace, Save: p.save}
	for _, ag := range agents {
		s.Agents = append(s.Agents, backends.Agent{Name: ag.name, Kind: ag.kind, Args: ag.args})
	}
	return s, nil
}

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
