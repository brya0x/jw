package commands

import (
	"bytes"
	"encoding/json"
	"slices"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
)

func TestLsAndInfoJSON(t *testing.T) {
	env := newTestEnv(t)
	p := env.stream("web")
	p.cfg.Ports = map[string]int{"web": 0, "api": 2}
	// streamsJSON reloads the config from disk, so the ports must live there.
	writeConfig(t, env, "[ports]\nweb = 0\napi = 2\n[dev]\nweb = [\"vite\"]\napi = [\"node api\"]\n")
	e, _ := p.reg.Find(p.name, "web")
	env.app.PRs = fakePRs{&connectors.PR{Number: 7, State: "OPEN", IsDraft: true, URL: "https://x/7", Branch: e.Branch}}
	env.shell.busy = map[int]connectors.PortOwner{20100: {PID: 1}} // web is up, api isn't
	t.Chdir(e.Path)

	env.out.Reset()
	if code := env.app.Run([]string{"ls", "--json"}); code != ExitOK {
		t.Fatalf("exit %d: %s", code, env.out)
	}
	var streams []streamJSON
	if err := json.Unmarshal(env.out.Bytes(), &streams); err != nil {
		t.Fatalf("ls --json is not JSON: %v\n%s", err, env.out)
	}
	if len(streams) != 1 {
		t.Fatalf("got %d streams", len(streams))
	}
	s := streams[0]
	if s.Name != "web" || s.Slot != 1 || s.PortBase != 20100 {
		t.Errorf("stream %+v", s)
	}
	if !s.Ports["web"].Listening || s.Ports["api"].Listening || s.Ports["api"].Port != 20102 {
		t.Errorf("ports %+v", s.Ports)
	}
	if !slices.Equal(s.Dev, []string{"api", "web"}) {
		t.Errorf("dev %v", s.Dev)
	}
	if s.PR == nil || s.PR.Number != 7 || s.PR.State != "draft" {
		t.Errorf("pr %+v", s.PR)
	}

	env.out.Reset()
	if code := env.app.Run([]string{"info", "--json"}); code != ExitOK {
		t.Fatalf("exit %d: %s", code, env.out)
	}
	var one streamJSON
	if err := json.Unmarshal(env.out.Bytes(), &one); err != nil || one.ID != e.ID {
		t.Fatalf("info --json: %v\n%s", err, env.out)
	}

	env.out.Reset()
	env.app.Run([]string{"info"})
	for _, want := range []string{"web ", "20100  listening", "#7 draft"} {
		if !strings.Contains(env.out.String(), want) {
			t.Errorf("info is missing %q:\n%s", want, env.out)
		}
	}
}

func TestNewJSONKeepsStdoutForTheJSON(t *testing.T) {
	env := newTestEnv(t)
	var stdout, stderr bytes.Buffer
	env.app.Out, env.app.Err = &stdout, &stderr
	t.Chdir(env.repo)

	if code := env.app.Run([]string{"new", "api", "--no-setup", "--json"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, stderr.String())
	}
	var s streamJSON
	if err := json.Unmarshal(stdout.Bytes(), &s); err != nil {
		t.Fatalf("stdout must be only JSON: %v\n%q", err, stdout.String())
	}
	if s.Name != "api" || s.Branch != "feat/api" {
		t.Errorf("stream %+v", s)
	}
	if !strings.Contains(stderr.String(), "fetching origin") {
		t.Errorf("progress should go to stderr:\n%s", stderr.String())
	}
}

// writeConfig puts a .jw.toml in the test repo.
func writeConfig(t *testing.T, env *testEnv, content string) {
	t.Helper()
	if err := writeFile(env.repo+"/.jw.toml", content); err != nil {
		t.Fatal(err)
	}
}
