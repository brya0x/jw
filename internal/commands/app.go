// Package commands implements every jw command. It talks to the outside world
// only through App: the connectors interfaces, the terminal streams and the
// confirmation prompt. It imports no concrete connector — main wires the real
// ones, tests wire fakes.
package commands

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/brya0x/jw/internal/backends"
	"github.com/brya0x/jw/internal/backends/terminal"
	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/core/registry"
)

// Exit codes. They are part of jw's interface: scripts and agents branch on them.
const (
	ExitOK         = 0
	ExitError      = 1 // something failed; the message says what
	ExitUsage      = 2 // unknown command or bad arguments
	ExitNeedsHuman = 3 // a person has to decide: a confirmation, a dialog. Don't retry — ask.
)

// ErrNeedsHuman marks a stop that only a person can resolve. Commands wrap
// it; Run turns it into ExitNeedsHuman.
var ErrNeedsHuman = backends.ErrNeedsHuman

type App struct {
	In       *os.File
	Out, Err io.Writer

	Shell connectors.Shell
	PRs   connectors.PullRequests
	// NewMux is called only by commands that need a multiplexer, so jw
	// new and jw ls keep working where herdr isn't installed.
	NewMux func() (connectors.Multiplexer, error)
	// Confirm asks a yes/no question. Nil means ask on the terminal.
	Confirm func(question string) (bool, error)
}

func (a *App) printf(format string, args ...any) {
	fmt.Fprintf(a.Out, format, args...)
}

func (a *App) warnf(format string, args ...any) {
	fmt.Fprintf(a.Err, format, args...)
}

// confirm asks through a.Confirm, or on the terminal by default. Anything but
// y/yes is no; without a terminal there is nobody to answer, so it refuses.
func (a *App) confirm(question string) (bool, error) {
	if a.Confirm != nil {
		return a.Confirm(question)
	}
	if !a.Shell.IsTerminal(a.In) {
		return false, fmt.Errorf("no terminal to confirm on: %w", ErrNeedsHuman)
	}
	a.printf("%s [y/N] ", question)
	line, _ := bufio.NewReader(a.In).ReadString('\n')
	answer := strings.ToLower(strings.TrimSpace(line))
	return answer == "y" || answer == "yes", nil
}

// backendFor is the backend e lives in once it's open.
func (a *App) backendFor(e *registry.Entry) (backends.Backend, error) {
	mux, err := a.NewMux()
	if err != nil {
		return nil, err
	}
	t := terminal.New(mux, a.ui())
	t.Settle = agentSettle
	return t, nil
}

// ui is how backends report to the person and ask them.
func (a *App) ui() backends.UI {
	return backends.UI{Out: a.Out, Err: a.Err, Confirm: a.confirm}
}
