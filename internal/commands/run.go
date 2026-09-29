package commands

import (
	"fmt"

	"github.com/brya0x/jw/internal/core/version"
)

// Run executes jw with args (without the program name) and returns the
// process exit code.
func (a *App) Run(args []string) int {
	if len(args) == 0 {
		a.usage()
		return 2
	}
	cmd, rest := args[0], args[1:]

	var err error
	switch cmd {
	case "init":
		err = a.runInit(rest)
	case "new":
		err = a.runNew(rest)
	case "open":
		err = a.runOpen(rest)
	case "close":
		err = a.runClose(rest)
	case "done":
		err = a.runDone(rest)
	case "sync":
		err = a.runSync(rest)
	case "rm":
		err = a.runRm(rest)
	case "dev":
		err = a.runDev(rest)
	case "setup":
		err = a.runSetup(rest)
	case "ls":
		err = a.runLs(rest)
	case "version", "--version", "-v":
		a.printf("%s\n", version.Current())
		return 0
	case "help", "-h", "--help":
		a.usage()
		return 0
	default:
		fmt.Fprintf(a.Err, "jw: unknown command %q\n\n", cmd)
		a.usage()
		return 2
	}

	if err != nil {
		fmt.Fprintln(a.Err, "jw:", err)
		return 1
	}
	return 0
}

func (a *App) usage() {
	fmt.Fprint(a.Err, `usage: jw <command> [args]

commands:
  init    write a starting config for this repo (detects setup and .env files)
  new     create a worktree and register it
  open    open the worktree in a herdr tab: editor | agent | dev
  close   close the tab, keep the worktree
  done    after the PR is merged: confirm, then delete worktree and branch
  sync    bring the branch up to date with the base (rebase, or --merge)
  rm      remove a stream whatever its PR says (guards unpushed work)
  dev     start a service on this worktree's ports (no service: list them)
  setup   re-run the setup commands of a worktree
  version show which build of jw this is
  ls      list worktrees of this project (-a: all projects, -i: interactive)
`)
}
