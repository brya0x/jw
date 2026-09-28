// Package commands implements every jw command. It talks to the outside world
// only through App: the connectors interfaces, the terminal streams and the
// confirmation prompt. It imports no concrete connector — main wires the real
// ones, tests wire fakes.
package commands

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/brya0x/jw/internal/connectors"
)

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
		return false, errors.New("no terminal to confirm on")
	}
	a.printf("%s [y/N] ", question)
	line, _ := bufio.NewReader(a.In).ReadString('\n')
	answer := strings.ToLower(strings.TrimSpace(line))
	return answer == "y" || answer == "yes", nil
}
