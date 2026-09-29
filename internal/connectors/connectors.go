// Package connectors is the contract between jw and the tools it drives.
//
// internal/commands depends on the interfaces here, never on a concrete tool.
// Each subpackage implements one of them against something real:
//
//	herdr   Multiplexer   the terminal workspace the tabs live in
//	github  PullRequests  PR state, through the gh CLI
//	system  Shell         running commands, which differs per OS
//
// Swapping a tool (herdr for tmux, gh for another forge, Unix for Windows)
// means a new implementation here, and no change in commands.
package connectors

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"time"
)

// Implementations wrap their own errors so that errors.Is(err, ErrNotFound)
// works no matter which tool produced them.
var (
	// ErrNotFound: the tab, pane or workspace does not exist (any more).
	ErrNotFound = errors.New("not found")
	// ErrAgentNotReady: the agent started but stopped at a dialog (folder
	// trust, login, approval) instead of reaching its prompt.
	ErrAgentNotReady = errors.New("agent not ready")
	// ErrAgentBlocked: the agent sits at an approval or question dialog and
	// won't take a prompt until someone answers it.
	ErrAgentBlocked = errors.New("agent blocked at a dialog")
)

type Workspace struct {
	ID    string
	Label string
}

type Tab struct {
	ID          string
	Label       string
	WorkspaceID string
}

type Pane struct {
	ID    string
	TabID string
	Label string
}

// Process is one process in the foreground of a pane.
type Process struct {
	Name    string
	Cmdline string
}

// Multiplexer lays out and drives terminal panes. env passed at creation
// reaches only the pane being created.
type Multiplexer interface {
	Workspaces() ([]Workspace, error)
	CreateWorkspace(cwd, label string, env []string) (Workspace, Tab, Pane, error)
	CreateTab(workspace, cwd, label string, env []string) (Tab, Pane, error)
	GetTab(id string) (Tab, error)
	RenameTab(id, label string) error
	FocusTab(id string) error
	CloseTab(id string) error
	PanesInTab(tab Tab) ([]Pane, error)

	// Split cuts pane in two; ratio is the share the original keeps.
	Split(pane, direction string, ratio float64, cwd string, env []string) (Pane, error)
	RenamePane(pane, label string) error
	// Run types command into the pane's shell.
	Run(pane, command string) error
	// Foreground lists what the pane runs right now: only its shell when idle.
	Foreground(pane string) ([]Process, error)

	// StartAgent launches an agent (claude, codex) in pane under name.
	StartAgent(name, kind, pane string, args []string) error
	// Prompt sends text to a running agent as if typed, without waiting for
	// it to finish.
	Prompt(agent, text string) error
	// WaitAgent waits until the agent settles — idle, done or blocked at a
	// dialog — and returns that status.
	WaitAgent(agent string, timeout time.Duration) (string, error)
	// CurrentPane is the pane jw itself runs in, when it runs inside.
	CurrentPane() (string, bool)
}

type PR struct {
	Number  int
	State   string // OPEN, CLOSED or MERGED
	IsDraft bool
	URL     string
	Branch  string
	HeadSHA string // the commit the forge has for the branch
}

// Status is the state in lower case, with "draft" for open drafts.
func (p PR) Status() string {
	if p.IsDraft && p.State == "OPEN" {
		return "draft"
	}
	return strings.ToLower(p.State)
}

// Label is the short form jw ls prints: "#12 merged", "#9 draft".
func (p PR) Label() string {
	return fmt.Sprintf("#%d %s", p.Number, p.Status())
}

// PullRequests reads PR state. dir is any directory of the repository.
type PullRequests interface {
	// ForBranch returns the newest PR whose head is branch, or nil.
	ForBranch(dir, branch string) (*PR, error)
	// ByBranch returns the newest PR of each head branch.
	ByBranch(dir string) (map[string]PR, error)
}

// Shell runs command lines. This is the part that differs per operating
// system: the shell itself, replacing the process, and stopping a command
// together with everything it spawned.
type Shell interface {
	// Run runs cmdline in dir and waits for it.
	Run(dir string, env []string, cmdline string, stdin io.Reader, stdout, stderr io.Writer) error
	// Replace hands the terminal to cmdline for good. It returns only on
	// failure to start.
	Replace(dir string, env []string, cmdline string) error
	// Group prepares cmdline so that cancelling ctx stops it and every
	// process it started.
	Group(ctx context.Context, dir string, env []string, cmdline string) *exec.Cmd
	// IsTerminal reports whether f is an interactive terminal.
	IsTerminal(f *os.File) bool
	// PortOwner reports whether something listens on a local port and,
	// when the OS can tell, which process. owner is zero when unknown.
	PortOwner(port int) (owner PortOwner, busy bool)
}

// PortOwner is the process listening on a port.
type PortOwner struct {
	PID     int
	Cmdline string
	Cwd     string // where it runs: tells which worktree started it
}
