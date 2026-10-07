package commands

import (
	"errors"
	"slices"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
)

func TestPromptReachesTheStreamsAgent(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	t.Chdir(env.repo)

	// Closed stream: a clear way forward, not a herdr error.
	if code := env.app.Run([]string{"prompt", "web", "fix the login"}); code != ExitError {
		t.Fatalf("exit %d", code)
	}
	if !strings.Contains(env.out.String(), "jw open web") {
		t.Errorf("should point at jw open:\n%s", env.out)
	}

	env.app.open(p, e, openOptions{noFocus: true})
	if code := env.app.Run([]string{"prompt", "web", "fix", "the", "login"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, env.out)
	}
	if !slices.Equal(env.mux.prompts, []string{"web: fix the login"}) {
		t.Fatalf("prompts %v", env.mux.prompts)
	}

	if code := env.app.Run([]string{"prompt", "web", "--codex", "review it"}); code != ExitOK {
		t.Fatalf("exit %d", code)
	}
	if env.mux.prompts[1] != "web-codex: review it" {
		t.Fatalf("prompts %v", env.mux.prompts)
	}
}

func TestPromptToABlockedAgentNeedsAPerson(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	env.app.open(p, e, openOptions{noFocus: true})
	env.mux.blocked = true
	t.Chdir(env.repo)

	if code := env.app.Run([]string{"prompt", "web", "go"}); code != ExitNeedsHuman {
		t.Fatalf("a blocked agent must exit %d, got %d\n%s", ExitNeedsHuman, code, env.out)
	}
	if !strings.Contains(env.out.String(), "waiting at a dialog") {
		t.Errorf("output:\n%s", env.out)
	}
}

func TestNewWithTaskOpensAndHandsItOver(t *testing.T) {
	env := newTestEnv(t)
	t.Chdir(env.repo)

	if code := env.app.Run([]string{"new", "api", "--no-setup", "--task", "add the /health route"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, env.out)
	}
	p, _ := loadProject(env.repo)
	e, _ := p.reg.Find(p.name, "api")
	if e.Tab == "" {
		t.Fatal("--task must open the stream")
	}
	if len(env.mux.focused) != 0 {
		t.Error("--task opens without stealing focus")
	}
	if !slices.Equal(env.mux.prompts, []string{"api: add the /health route"}) {
		t.Fatalf("prompts %v", env.mux.prompts)
	}
}

func TestNewOpensWithoutFocusUnlessToldNotTo(t *testing.T) {
	env := newTestEnv(t)
	t.Chdir(env.repo)

	if code := env.app.Run([]string{"new", "web", "--no-setup"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, env.out)
	}
	p, _ := loadProject(env.repo)
	if e, _ := p.reg.Find(p.name, "web"); e.Tab == "" {
		t.Fatal("jw new must open the stream")
	}
	if len(env.mux.focused) != 0 {
		t.Error("jw new opens without stealing focus")
	}

	if code := env.app.Run([]string{"new", "api", "--no-setup", "--no-open"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, env.out)
	}
	p, _ = loadProject(env.repo)
	if e, _ := p.reg.Find(p.name, "api"); e.Tab != "" {
		t.Fatal("--no-open must not open")
	}

	if code := env.app.Run([]string{"new", "x", "--no-open", "--task", "go"}); code != ExitError {
		t.Fatalf("--no-open with --task: exit %d", code)
	}
}

func TestNewStillCreatesWhenItCantOpen(t *testing.T) {
	env := newTestEnv(t)
	env.app.NewMux = func() (connectors.Multiplexer, error) { return nil, errors.New("herdr not found") }
	t.Chdir(env.repo)

	if code := env.app.Run([]string{"new", "web", "--no-setup"}); code != ExitOK {
		t.Fatalf("a failed open must not fail jw new: exit %d\n%s", code, env.out)
	}
	if !strings.Contains(env.out.String(), "jw open web") {
		t.Errorf("should say how to open it:\n%s", env.out)
	}
	if code := env.app.Run([]string{"new", "api", "--no-setup", "--task", "go"}); code != ExitError {
		t.Fatalf("with a task, a failed open is an error: exit %d", code)
	}
}
