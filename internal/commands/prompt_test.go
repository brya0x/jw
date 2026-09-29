package commands

import (
	"slices"
	"strings"
	"testing"
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
