package commands

import (
	"errors"
	"flag"
	"fmt"
	"strings"
	"time"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/core/registry"
)

func (a *App) runPrompt(args []string) error {
	name, args := splitName(args)
	fs := flag.NewFlagSet("prompt", flag.ExitOnError)
	codex := fs.Bool("codex", false, "the Codex agent of a stream opened with --agent both")
	fs.Parse(args)
	text := strings.TrimSpace(strings.Join(fs.Args(), " "))
	if name == "" || text == "" {
		return errors.New(`usage: jw prompt <name> [--codex] "<task>"`)
	}

	p, e, err := open(name)
	if err != nil {
		return err
	}
	return a.prompt(p, e, *codex, text, false)
}

// agentSettle is how long a freshly started agent gets before jw looks at
// it again: its first dialog (folder trust) can appear a moment after it
// already looked ready. Tests set it to zero.
var agentSettle = 2 * time.Second

// prompt hands a task to the agent running in a stream's tab and returns
// without waiting for it: the work shows up in that tab.
//
// It never types into a dialog: it waits for the agent to settle and, if it
// sits blocked at one (folder trust, an approval), stops for a person
// instead. fresh says the agent was started a moment ago.
func (a *App) prompt(p *project, e *registry.Entry, codex bool, text string, fresh bool) error {
	if e.Tab == "" {
		return fmt.Errorf("%s is not open: `jw open %s` first, or `jw open %s --task …`", e.Name, e.Name, e.Name)
	}
	mux, err := a.NewMux()
	if err != nil {
		return err
	}
	target := e.Name
	if codex {
		target += "-codex"
	}

	blocked := func() error {
		return fmt.Errorf("%s is waiting at a dialog in its pane — someone has to answer it first, then `jw prompt %s …`: %w",
			target, e.Name, ErrNeedsHuman)
	}
	status, err := mux.WaitAgent(target, 20*time.Second)
	if err == nil && status != "blocked" && fresh {
		time.Sleep(agentSettle)
		status, err = mux.WaitAgent(target, 5*time.Second)
	}
	switch {
	case errors.Is(err, connectors.ErrNotFound):
		return fmt.Errorf("no agent %s is running in %s's tab; `jw open %s` starts it", target, e.Name, e.Name)
	case err != nil:
		return fmt.Errorf("%s isn't ready for a prompt: %w", target, err)
	case status == "blocked":
		return blocked()
	}

	err = mux.Prompt(target, text)
	switch {
	case err == nil:
		a.printf("sent to %s (see tab %s)\n", target, e.Tab)
		return nil
	case errors.Is(err, connectors.ErrAgentBlocked):
		return blocked()
	case errors.Is(err, connectors.ErrNotFound):
		return fmt.Errorf("no agent %s is running in %s's tab; `jw open %s` starts it", target, e.Name, e.Name)
	default:
		return err
	}
}
