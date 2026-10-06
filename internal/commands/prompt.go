package commands

import (
	"errors"
	"flag"
	"strings"
	"time"

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

	p, e, err := a.target(name)
	if err != nil {
		return err
	}
	return a.prompt(p, e, *codex, text, false)
}

// agentSettle is how long a freshly started agent gets before jw looks at
// it again (see terminal.Backend.Settle). Tests set it to zero.
var agentSettle = 2 * time.Second

// prompt hands a task to one of the stream's agents and returns without
// waiting for the work.
func (a *App) prompt(p *project, e *registry.Entry, codex bool, text string, fresh bool) error {
	b, err := a.backendFor(e)
	if err != nil {
		return err
	}
	target, err := p.agentName(e)
	if err != nil {
		return err
	}
	if codex {
		target += "-codex"
	}
	return b.Prompt(e, target, text, fresh)
}
