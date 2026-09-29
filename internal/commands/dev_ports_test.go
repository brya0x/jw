package commands

import (
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
)

func TestDevRefusesATakenPort(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	p.cfg.Ports = map[string]int{"web": 0, "api": 2}
	p.cfg.Dev = map[string][]string{"web": {"vite --port {port.web}"}}

	// Something from this very worktree already holds web's port.
	env.shell.busy = map[int]connectors.PortOwner{
		20100: {PID: 4242, Cmdline: "node vite --port 20100", Cwd: e.Path + "/apps/web"},
	}
	err := env.app.dev(p, e, "web")
	if err == nil {
		t.Fatal("want a refusal")
	}
	for _, want := range []string{"20100 (web)", "pid 4242", "node vite", "already running in this worktree"} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("error is missing %q:\n%v", want, err)
		}
	}
	if len(env.shell.ran) != 0 {
		t.Fatalf("nothing may start when a port is taken, ran %v", env.shell.ran)
	}

	// A port the service doesn't use (api) being busy is none of its business.
	env.shell.busy = map[int]connectors.PortOwner{20102: {PID: 1}}
	if err := env.app.dev(p, e, "web"); err != nil {
		t.Fatal(err)
	}
	if len(env.shell.ran) != 1 || !strings.Contains(env.shell.ran[0], "vite --port 20100") {
		t.Fatalf("ran %v", env.shell.ran)
	}
}

func TestDevNamesAnUnknownOwner(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	p.cfg.Ports = map[string]int{"web": 0}
	p.cfg.Dev = map[string][]string{"web": {"vite --port {port.web}"}}
	env.shell.busy = map[int]connectors.PortOwner{20100: {}}

	err := env.app.dev(p, e, "web")
	if err == nil || !strings.Contains(err.Error(), "can't see") {
		t.Fatalf("got %v", err)
	}
}
