// Command jw manages parallel workstreams: one git worktree, one herdr tab,
// one port slot and one PR per stream.
//
// This is the only place that knows which tools jw uses: it plugs the real
// connectors into commands.App. Swapping one (herdr for tmux, say) is a change
// here plus a new package under internal/connectors.
package main

import (
	"os"

	"github.com/brya0x/jw/internal/commands"
	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/connectors/github"
	"github.com/brya0x/jw/internal/connectors/herdr"
	"github.com/brya0x/jw/internal/connectors/system"
)

func main() {
	app := &commands.App{
		In:    os.Stdin,
		Out:   os.Stdout,
		Err:   os.Stderr,
		Shell: system.Shell{},
		PRs:   github.Client{},
		NewMux: func() (connectors.Multiplexer, error) {
			return herdr.New()
		},
	}
	os.Exit(app.Run(os.Args[1:]))
}
