// Package backends is where a stream lives once it's open: the windows,
// panes and agent that `jw open` builds around a worktree.
//
//	terminal  the default: a herdr tab with nvim, the agent and a dev shell
//
// A backend is jw's own policy built on top of connectors (herdr, claude,
// code). internal/commands resolves everything the config says into a
// Stream and hands it over; a backend never reads the config itself.
package backends

import (
	"errors"
	"io"

	"github.com/brya0x/jw/internal/core/registry"
)

// ErrNeedsHuman marks a stop that only a person can resolve: a dialog in an
// agent's pane, a confirmation. jw turns it into exit code 3.
var ErrNeedsHuman = errors.New("needs a person to decide")

// Stream is one stream, ready to open.
type Stream struct {
	Entry *registry.Entry
	// Env is every JW_* variable, as KEY=value.
	Env []string
	// Agents to start; the first one gets the task.
	Agents []Agent
	// Editor is the command line the editor pane runs.
	Editor string
	// Workspace is the label of the workspace the stream goes in.
	Workspace string
	// Save persists Entry. Backends call it as soon as they record
	// something, so a later failure never loses track of what exists.
	Save func() error
}

// Agent is one agent to start.
type Agent struct {
	Name string   // what the agent is called, and addressed by
	Kind string   // claude, codex
	Args []string // after the executable
}

type OpenOptions struct {
	NoFocus bool
	Task    string // handed to the first agent once it's up
}

// UI is where a backend reports to the person, and how it asks them.
type UI struct {
	Out, Err io.Writer
	Confirm  func(question string) (bool, error)
}

// Backend opens, drives and closes streams.
type Backend interface {
	// Open builds the stream, or brings it forward if it's already open,
	// and hands o.Task to its agent.
	Open(s Stream, o OpenOptions) error
	// Close tears the stream down and keeps the worktree. Unless force,
	// it asks before killing work in progress, and reports false when the
	// person says no. save runs right before anything is torn down, after
	// Close forgot it in the entry: the process may not survive the close.
	Close(e *registry.Entry, force bool, save func() error) (bool, error)
	// Prompt hands text to the agent called agent, without waiting for the
	// work. fresh: the agent was started a moment ago.
	Prompt(e *registry.Entry, agent, text string, fresh bool) error
	// InsideOwn reports whether jw runs inside the stream itself, where
	// closing it would kill jw half-way.
	InsideOwn(e *registry.Entry) bool
	// Prune forgets what was closed outside jw. It reports whether e changed.
	Prune(e *registry.Entry) bool
}
