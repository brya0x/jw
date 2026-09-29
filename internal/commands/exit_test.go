package commands

import (
	"strings"
	"testing"
)

func TestExitCodes(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	t.Chdir(e.Path)

	// No Confirm hook and no terminal: the way an agent runs jw.
	env.app.Confirm = nil

	if code := env.app.Run([]string{"rm"}); code != ExitNeedsHuman {
		t.Fatalf("a confirmation without a terminal must exit %d, got %d\n%s", ExitNeedsHuman, code, env.out)
	}
	if !strings.Contains(env.out.String(), "ask them, don't retry") {
		t.Errorf("should tell the caller to ask:\n%s", env.out)
	}
	if !p.reg.Has(p.name, "web") {
		t.Fatal("nothing may be removed without an answer")
	}

	if code := env.app.Run([]string{"nope"}); code != ExitUsage {
		t.Errorf("unknown command: got %d", code)
	}
	if code := env.app.Run([]string{"sync", "missing-stream"}); code != ExitError {
		t.Errorf("plain failure: got %d", code)
	}
	if code := env.app.Run([]string{"version"}); code != ExitOK {
		t.Errorf("version: got %d", code)
	}
}
