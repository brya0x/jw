package commands

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
)

// fakeMux is an in-memory multiplexer that records what commands asked of it.
type fakeMux struct {
	workspaces []connectors.Workspace
	tabs       map[string]connectors.Tab
	panes      map[string]connectors.Pane
	paneEnv    map[string][]string // env each pane was created with
	running    map[string][]connectors.Process
	ran        map[string]string // pane → command typed into it
	agents     []string          // "name kind pane args"
	splits     []string          // "pane direction ratio"
	closed     []string
	focused    []string
	next       int
}

func newFakeMux() *fakeMux {
	return &fakeMux{
		tabs: map[string]connectors.Tab{}, panes: map[string]connectors.Pane{},
		paneEnv: map[string][]string{}, running: map[string][]connectors.Process{},
		ran: map[string]string{},
	}
}

func (f *fakeMux) id(prefix string) string {
	f.next++
	return fmt.Sprintf("%s%d", prefix, f.next)
}

func (f *fakeMux) newPane(tab string, env []string) connectors.Pane {
	p := connectors.Pane{ID: f.id("p"), TabID: tab}
	f.panes[p.ID], f.paneEnv[p.ID] = p, env
	return p
}

func (f *fakeMux) Workspaces() ([]connectors.Workspace, error) { return f.workspaces, nil }

func (f *fakeMux) CreateWorkspace(cwd, label string, env []string) (connectors.Workspace, connectors.Tab, connectors.Pane, error) {
	ws := connectors.Workspace{ID: f.id("w"), Label: label}
	f.workspaces = append(f.workspaces, ws)
	tab := connectors.Tab{ID: f.id("t"), WorkspaceID: ws.ID}
	f.tabs[tab.ID] = tab
	return ws, tab, f.newPane(tab.ID, env), nil
}

func (f *fakeMux) CreateTab(workspace, cwd, label string, env []string) (connectors.Tab, connectors.Pane, error) {
	tab := connectors.Tab{ID: f.id("t"), Label: label, WorkspaceID: workspace}
	f.tabs[tab.ID] = tab
	return tab, f.newPane(tab.ID, env), nil
}

func (f *fakeMux) GetTab(id string) (connectors.Tab, error) {
	t, ok := f.tabs[id]
	if !ok {
		return t, fmt.Errorf("tab %s: %w", id, connectors.ErrNotFound)
	}
	return t, nil
}

func (f *fakeMux) RenameTab(id, label string) error { return nil }
func (f *fakeMux) FocusTab(id string) error         { f.focused = append(f.focused, id); return nil }

func (f *fakeMux) CloseTab(id string) error {
	delete(f.tabs, id)
	f.closed = append(f.closed, id)
	return nil
}

func (f *fakeMux) PanesInTab(tab connectors.Tab) ([]connectors.Pane, error) {
	var in []connectors.Pane
	for _, p := range f.panes {
		if p.TabID == tab.ID {
			in = append(in, p)
		}
	}
	return in, nil
}

func (f *fakeMux) Split(pane, direction string, ratio float64, cwd string, env []string) (connectors.Pane, error) {
	f.splits = append(f.splits, fmt.Sprintf("%s %s %.1f", pane, direction, ratio))
	return f.newPane(f.panes[pane].TabID, env), nil
}

func (f *fakeMux) RenamePane(id, label string) error {
	p := f.panes[id]
	p.Label = label
	f.panes[id] = p
	return nil
}

func (f *fakeMux) Run(pane, command string) error { f.ran[pane] = command; return nil }

func (f *fakeMux) Foreground(pane string) ([]connectors.Process, error) {
	if procs, ok := f.running[pane]; ok {
		return procs, nil
	}
	return []connectors.Process{{Name: "-zsh", Cmdline: "-zsh"}}, nil
}

func (f *fakeMux) StartAgent(name, kind, pane string, args []string) error {
	f.agents = append(f.agents, strings.TrimSpace(fmt.Sprintf("%s %s %s %s", name, kind, pane, strings.Join(args, " "))))
	return nil
}

func (f *fakeMux) CurrentPane() (string, bool) { return "", false }

// fakePRs answers with a fixed PR for one branch.
type fakePRs struct{ pr *connectors.PR }

func (f fakePRs) ForBranch(dir, branch string) (*connectors.PR, error) {
	if f.pr != nil && f.pr.Branch == branch {
		return f.pr, nil
	}
	return nil, nil
}

func (f fakePRs) ByBranch(dir string) (map[string]connectors.PR, error) {
	if f.pr == nil {
		return map[string]connectors.PR{}, nil
	}
	return map[string]connectors.PR{f.pr.Branch: *f.pr}, nil
}

// fakeShell records commands instead of running them. busy marks ports as
// taken, by the given owner.
type fakeShell struct {
	ran  []string
	busy map[int]connectors.PortOwner
}

func (s *fakeShell) PortOwner(port int) (connectors.PortOwner, bool) {
	o, ok := s.busy[port]
	return o, ok
}

func (s *fakeShell) Run(dir string, env []string, cmdline string, _ io.Reader, _, _ io.Writer) error {
	s.ran = append(s.ran, cmdline)
	return nil
}
func (s *fakeShell) Replace(dir string, env []string, cmdline string) error {
	s.ran = append(s.ran, "replace: "+cmdline)
	return nil
}
func (s *fakeShell) Group(ctx context.Context, dir string, env []string, cmdline string) *exec.Cmd {
	return exec.CommandContext(ctx, "true")
}
func (s *fakeShell) IsTerminal(*os.File) bool { return false }

// testEnv is a real git repo (origin + clone) plus an App wired to fakes.
type testEnv struct {
	t     *testing.T
	repo  string
	app   *App
	mux   *fakeMux
	shell *fakeShell
	out   *bytes.Buffer
	yes   bool // what Confirm answers
	asked []string
}

func newTestEnv(t *testing.T) *testEnv {
	t.Helper()
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	dir := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(dir, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))

	origin, repo := filepath.Join(dir, "myapp.git"), filepath.Join(dir, "myapp")
	git := func(in string, args ...string) {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = in
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	git(dir, "init", "-q", "--bare", "-b", "main", origin)
	git(dir, "init", "-q", "-b", "main", repo)
	git(repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init")
	git(repo, "remote", "add", "origin", origin)
	git(repo, "push", "-q", "-u", "origin", "main")

	env := &testEnv{t: t, repo: repo, mux: newFakeMux(), shell: &fakeShell{}, out: &bytes.Buffer{}}
	env.app = &App{
		In: os.Stdin, Out: env.out, Err: env.out,
		Shell:  env.shell,
		PRs:    fakePRs{},
		NewMux: func() (connectors.Multiplexer, error) { return env.mux, nil },
		Confirm: func(q string) (bool, error) {
			env.asked = append(env.asked, q)
			return env.yes, nil
		},
	}
	return env
}

// stream creates a worktree through the real newStream and returns its project.
func (env *testEnv) stream(name string) *project {
	env.t.Helper()
	p, err := loadProject(env.repo)
	if err != nil {
		env.t.Fatal(err)
	}
	if err := env.app.newStream(p, newOptions{name: name}); err != nil {
		env.t.Fatal(err)
	}
	return p
}
