package commands

import (
	"reflect"
	"slices"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/core/config"
	"github.com/brya0x/jw/internal/core/registry"
)

func TestAgentsForStartThenResume(t *testing.T) {
	cfg := &config.Config{Agent: config.Agent{
		Claude: config.AgentCmd{Start: "claude", Resume: "claude --continue"},
		Codex:  config.AgentCmd{Start: "codex", Resume: "codex resume --last"},
	}}
	e := &registry.Entry{Name: "web"}

	first, err := agentsFor(cfg, "claude", e, "web")
	if err != nil {
		t.Fatal(err)
	}
	if want := []agentSpec{{name: "web", kind: "claude", args: []string{}}}; !reflect.DeepEqual(first, want) {
		t.Fatalf("first open: got %+v", first)
	}

	e.Opened = true
	both, err := agentsFor(cfg, "both", e, "web")
	if err != nil {
		t.Fatal(err)
	}
	want := []agentSpec{
		{name: "web", kind: "claude", args: []string{"--continue"}},
		{name: "web-codex", kind: "codex", args: []string{"resume", "--last"}},
	}
	if !reflect.DeepEqual(both, want) {
		t.Fatalf("reopen both: got %+v", both)
	}

	if _, err := agentsFor(cfg, "gemini", e, "web"); err == nil {
		t.Fatal("unknown agent should fail")
	}
}

func TestWorkspaceWithNameGivesEachStreamItsOwn(t *testing.T) {
	env := newTestEnv(t)
	writeConfig(t, env, "workspace = \"myapp-{name}\"\n")
	for _, name := range []string{"web", "api"} {
		p := env.stream(name)
		e, _ := p.reg.Find(p.name, name)
		if err := env.app.open(p, e, openOptions{noFocus: true}); err != nil {
			t.Fatal(err)
		}
	}
	var labels []string
	for _, ws := range env.mux.workspaces {
		labels = append(labels, ws.Label)
	}
	if !slices.Equal(labels, []string{"myapp-web", "myapp-api"}) {
		t.Fatalf("workspaces %v", labels)
	}
	// The agent reads the same as its workspace, and prompts find it.
	if len(env.mux.agents) != 2 || !strings.HasPrefix(env.mux.agents[0], "myapp-web claude ") {
		t.Fatalf("agents %v", env.mux.agents)
	}
	t.Chdir(env.repo)
	if code := env.app.Run([]string{"prompt", "api", "go"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, env.out)
	}
	if !slices.Equal(env.mux.prompts, []string{"myapp-api: go"}) {
		t.Fatalf("prompts %v", env.mux.prompts)
	}
}

func TestOpenRefusesAnAgentNameHerdrWouldReject(t *testing.T) {
	env := newTestEnv(t)
	writeConfig(t, env, "workspace = \"myapp-{name}\"\n")
	p := env.stream("a-stream-name-this-long")
	e, _ := p.reg.Find(p.name, "a-stream-name-this-long")

	err := env.app.open(p, e, openOptions{noFocus: true, agent: "both"})
	if err == nil || !strings.Contains(err.Error(), "myapp-a-stream-name-this-long-codex") {
		t.Fatalf("got %v", err)
	}
	if len(env.mux.workspaces) != 0 {
		t.Error("nothing should be created for an agent that can't start")
	}
}
