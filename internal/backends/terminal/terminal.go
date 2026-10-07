// Package terminal is jw's default backend: each stream is a herdr tab with
// the editor, the agent and a dev shell.
//
//	┌────────┬────────┐
//	│ editor │ agent  │
//	├────────┴────────┤
//	│ dev             │
//	└─────────────────┘
package terminal

import (
	"errors"
	"fmt"
	"os"
	"regexp"
	"strings"
	"time"

	"github.com/brya0x/jw/internal/backends"
	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/core/registry"
)

type Backend struct {
	Mux connectors.Multiplexer
	UI  backends.UI
	// Settle is how long a freshly started agent gets before jw looks at
	// it again: its first dialog (folder trust) can appear a moment after it
	// already looked ready.
	Settle time.Duration
}

var _ backends.Backend = (*Backend)(nil)

func New(mux connectors.Multiplexer, ui backends.UI) *Backend {
	return &Backend{Mux: mux, UI: ui, Settle: 2 * time.Second}
}

func (b *Backend) printf(format string, args ...any) { fmt.Fprintf(b.UI.Out, format, args...) }
func (b *Backend) warnf(format string, args ...any)  { fmt.Fprintf(b.UI.Err, format, args...) }

// agentName is what herdr accepts as an agent's name.
var agentName = regexp.MustCompile(`^[a-z][a-z0-9_-]{0,31}$`)

// Open builds the worktree's tab, or focuses it if it's already there.
func (b *Backend) Open(s backends.Stream, o backends.OpenOptions) error {
	mux, e := b.Mux, s.Entry

	// Idempotent: a live tab is focused, not rebuilt. A tab that was closed
	// by hand is forgotten and recreated.
	if e.Tab != "" {
		_, err := mux.GetTab(e.Tab)
		switch {
		case err == nil:
			if !o.NoFocus {
				if err := mux.FocusTab(e.Tab); err != nil {
					return err
				}
			}
			b.printf("%s is already open in %s\n", e.Name, e.Tab)
			return b.handTask(s, o.Task, false)
		case errors.Is(err, connectors.ErrNotFound):
			e.Tab = ""
		default:
			return err
		}
	}

	for _, ag := range s.Agents {
		if !agentName.MatchString(ag.Name) {
			return fmt.Errorf("herdr won't name an agent %q: it takes a lowercase letter, then up to 31 of a-z, 0-9, - and _ — shorten the stream name or the workspace label", ag.Name)
		}
	}

	tab, editorPane, err := createTab(mux, s.Workspace, e, s.Env)
	if err != nil {
		return err
	}

	// Record the tab before anything else can fail, so `jw close` can
	// always find what `jw open` created.
	e.Tab = tab.ID
	if err := s.Save(); err != nil {
		return err
	}

	// Split the full-width bottom off first, then cut the top in two.
	devPane, err := mux.Split(editorPane.ID, "down", 0.7, e.Path, s.Env)
	if err != nil {
		return err
	}
	agentPane, err := mux.Split(editorPane.ID, "right", 0.5, e.Path, s.Env)
	if err != nil {
		return err
	}
	_ = mux.RenamePane(editorPane.ID, "editor")
	_ = mux.RenamePane(devPane.ID, "dev")

	if err := mux.Run(editorPane.ID, s.Editor); err != nil {
		return err
	}

	panes := []connectors.Pane{agentPane}
	for range s.Agents[1:] {
		extra, err := mux.Split(agentPane.ID, "down", 0.5, e.Path, s.Env)
		if err != nil {
			return err
		}
		panes = append(panes, extra)
	}

	// An agent that fails to start doesn't undo the tab: the editor and dev
	// panes are still useful, and the agent can be started by hand.
	for i, ag := range s.Agents {
		b.printf("starting %s (%s %s)…\n", ag.Name, ag.Kind, strings.Join(ag.Args, " "))
		err := mux.StartAgent(ag.Name, ag.Kind, panes[i].ID, ag.Args)
		switch {
		case err == nil:
		case errors.Is(err, connectors.ErrAgentNotReady):
			// jw never answers these: a trust or approval dialog is the user's call.
			b.warnf("note: %s is waiting at a dialog in its pane (a new folder asks whether you trust it) — answer it there\n", ag.Name)
		default:
			b.warnf("warning: %s did not start: %v\n", ag.Name, err)
		}
	}

	e.Opened = true
	if err := s.Save(); err != nil {
		return err
	}
	if !o.NoFocus {
		if err := mux.FocusTab(tab.ID); err != nil {
			return err
		}
	}
	b.printf("opened %s in %s\n", e.Name, tab.ID)
	return b.handTask(s, o.Task, true)
}

// handTask prompts the stream's first agent with task, if there is one.
// fresh: the agent was just started.
func (b *Backend) handTask(s backends.Stream, task string, fresh bool) error {
	if task == "" {
		return nil
	}
	return b.Prompt(s.Entry, s.Agents[0].Name, task, fresh)
}

// createTab puts the worktree's tab in the workspace labelled workspace,
// creating it on first use (and reusing the tab it comes with). A label with
// {name} in it gives every stream a workspace of its own.
func createTab(mux connectors.Multiplexer, workspace string, e *registry.Entry, env []string) (connectors.Tab, connectors.Pane, error) {
	all, err := mux.Workspaces()
	if err != nil {
		return connectors.Tab{}, connectors.Pane{}, err
	}
	for _, ws := range all {
		if ws.Label == workspace {
			return mux.CreateTab(ws.ID, e.Path, e.Name, env)
		}
	}

	_, tab, root, err := mux.CreateWorkspace(e.Path, workspace, env)
	if err != nil {
		return tab, root, err
	}
	return tab, root, mux.RenameTab(tab.ID, e.Name)
}

// Prompt hands a task to the agent running in a stream's tab and returns
// without waiting for it: the work shows up in that tab.
//
// It never types into a dialog: it waits for the agent to settle and, if it
// sits blocked at one (folder trust, an approval), stops for a person
// instead.
func (b *Backend) Prompt(e *registry.Entry, agent, text string, fresh bool) error {
	if e.Tab == "" {
		return fmt.Errorf("%s is not open: `jw open %s` first, or `jw open %s --task …`", e.Name, e.Name, e.Name)
	}
	mux := b.Mux

	blocked := func() error {
		return fmt.Errorf("%s is waiting at a dialog in its pane — someone has to answer it first, then `jw prompt %s …`: %w",
			agent, e.Name, backends.ErrNeedsHuman)
	}
	status, err := mux.WaitAgent(agent, 20*time.Second)
	if err == nil && status != "blocked" && fresh {
		time.Sleep(b.Settle)
		status, err = mux.WaitAgent(agent, 5*time.Second)
	}
	switch {
	case errors.Is(err, connectors.ErrNotFound):
		return fmt.Errorf("no agent %s is running in %s's tab; `jw open %s` starts it", agent, e.Name, e.Name)
	case err != nil:
		return fmt.Errorf("%s isn't ready for a prompt: %w", agent, err)
	case status == "blocked":
		return blocked()
	}

	err = mux.Prompt(agent, text)
	switch {
	case err == nil:
		b.printf("sent to %s (see tab %s)\n", agent, e.Tab)
		return nil
	case errors.Is(err, connectors.ErrAgentBlocked):
		return blocked()
	case errors.Is(err, connectors.ErrNotFound):
		return fmt.Errorf("no agent %s is running in %s's tab; `jw open %s` starts it", agent, e.Name, e.Name)
	default:
		return err
	}
}

// Close closes e's tab, asking first if its dev pane is running something.
func (b *Backend) Close(e *registry.Entry, force bool, save func() error) (bool, error) {
	mux := b.Mux
	// Forget the tab before closing it: when jw runs inside that very tab,
	// closing it kills this process, and nothing after would run.
	forget := func() error {
		e.Tab = ""
		if save != nil {
			return save()
		}
		return nil
	}

	tab, err := mux.GetTab(e.Tab)
	if errors.Is(err, connectors.ErrNotFound) {
		return true, forget() // already gone
	}
	if err != nil {
		return false, err
	}

	if !force {
		running, err := devProcesses(mux, tab)
		if err != nil {
			return false, err
		}
		if len(running) > 0 {
			b.printf("the dev pane of %s is running: %s\n", e.Name, strings.Join(running, "; "))
			ok, err := b.UI.Confirm("close it anyway?")
			if err != nil {
				return false, fmt.Errorf("%w (pass -y to close without asking)", err)
			}
			if !ok {
				b.printf("left open\n")
				return false, nil
			}
		}
	}
	if err := forget(); err != nil {
		return false, err
	}
	return true, mux.CloseTab(tab.ID)
}

// InsideOwn reports whether jw is running in one of e's own panes. It knows
// from JW_ID, which every pane of the tab is born with, or else from the
// multiplexer's current pane.
func (b *Backend) InsideOwn(e *registry.Entry) bool {
	if os.Getenv("JW_ID") == e.ID {
		return true
	}
	current, inside := b.Mux.CurrentPane()
	if !inside || e.Tab == "" {
		return false
	}
	tab, err := b.Mux.GetTab(e.Tab)
	if err != nil {
		return false
	}
	panes, err := b.Mux.PanesInTab(tab)
	if err != nil {
		return false
	}
	for _, p := range panes {
		if p.ID == current {
			return true
		}
	}
	return false
}

// Prune forgets a tab that was closed by hand, so the registry never keeps
// pointing at a dead one.
func (b *Backend) Prune(e *registry.Entry) bool {
	if e.Tab == "" {
		return false
	}
	if _, err := b.Mux.GetTab(e.Tab); errors.Is(err, connectors.ErrNotFound) {
		e.Tab = ""
		return true
	}
	return false
}

// devProcesses returns what runs in the tab's dev panes besides the shell.
func devProcesses(mux connectors.Multiplexer, tab connectors.Tab) ([]string, error) {
	panes, err := mux.PanesInTab(tab)
	if err != nil {
		return nil, err
	}
	var running []string
	for _, p := range panes {
		if p.Label != "dev" {
			continue
		}
		procs, err := mux.Foreground(p.ID)
		if err != nil {
			return nil, err
		}
		for _, proc := range procs {
			if !isShell(proc.Name) {
				running = append(running, proc.Cmdline)
			}
		}
	}
	return running, nil
}

func isShell(name string) bool {
	switch strings.TrimPrefix(name, "-") { // login shells show up as "-zsh"
	case "zsh", "bash", "sh", "fish", "dash", "nu", "ksh", "cmd.exe", "powershell.exe", "pwsh.exe":
		return true
	}
	return false
}
