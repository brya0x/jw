// Command jw manages parallel workstreams: one git worktree, one herdr tab,
// one port slot and one PR per stream.
package main

import (
	"fmt"
	"os"
)

func main() {
	if len(os.Args) < 2 {
		usage()
		os.Exit(2)
	}

	cmd, args := os.Args[1], os.Args[2:]

	var err error
	switch cmd {
	case "new":
		err = runNew(args)
	case "setup":
		err = runSetupCmd(args)
	case "ls":
		err = runLs(args)
	case "help", "-h", "--help":
		usage()
		return
	default:
		fmt.Fprintf(os.Stderr, "jw: unknown command %q\n\n", cmd)
		usage()
		os.Exit(2)
	}

	if err != nil {
		fmt.Fprintln(os.Stderr, "jw:", err)
		os.Exit(1)
	}
}

func usage() {
	fmt.Fprint(os.Stderr, `usage: jw <command> [args]

commands:
  new     create a worktree and register it
  setup   re-run the setup commands of a worktree
  ls      list worktrees of this project (-a: all projects)
`)
}
