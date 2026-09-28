package main

import (
	"reflect"
	"testing"

	"github.com/brya0x/jw/internal/config"
	"github.com/brya0x/jw/internal/registry"
)

func TestAgentsForStartThenResume(t *testing.T) {
	cfg := &config.Config{Agent: config.Agent{
		Claude: config.AgentCmd{Start: "claude", Resume: "claude --continue"},
		Codex:  config.AgentCmd{Start: "codex", Resume: "codex resume --last"},
	}}
	e := &registry.Entry{Name: "web"}

	first, err := agentsFor(cfg, "claude", e)
	if err != nil {
		t.Fatal(err)
	}
	if want := []agentSpec{{name: "web", kind: "claude", args: []string{}}}; !reflect.DeepEqual(first, want) {
		t.Fatalf("first open: got %+v", first)
	}

	e.Opened = true
	both, err := agentsFor(cfg, "both", e)
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

	if _, err := agentsFor(cfg, "gemini", e); err == nil {
		t.Fatal("unknown agent should fail")
	}
}
