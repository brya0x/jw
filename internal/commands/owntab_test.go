package commands

import (
	"os"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/registry"
)

// openMergedStream is a stream with its tab open and a merged PR: jw done
// would go all the way.
func openMergedStream(t *testing.T) (*testEnv, *project, *registry.Entry) {
	t.Helper()
	env := newTestEnv(t)
	p := env.stream("web")
	e, _ := p.reg.Find(p.name, "web")
	env.app.open(p, e, openOptions{noFocus: true})
	head, _ := git.Head(e.Path)
	env.app.PRs = fakePRs{&connectors.PR{Number: 7, State: "MERGED", Branch: e.Branch, HeadSHA: head}}
	env.yes = true
	return env, p, e
}

func paneOf(env *testEnv, tab string) string {
	for id, p := range env.mux.panes {
		if p.TabID == tab {
			return id
		}
	}
	return ""
}

func TestDoneRefusesFromItsOwnTab(t *testing.T) {
	t.Run("known from JW_ID", func(t *testing.T) {
		env, p, e := openMergedStream(t)
		t.Setenv("JW_ID", e.ID)
		err := env.app.done(p, e)
		if err == nil || !strings.Contains(err.Error(), "own tab") {
			t.Fatalf("want refusal, got %v", err)
		}
		if len(env.asked) != 0 {
			t.Error("must refuse before asking")
		}
		if _, err := os.Stat(e.Path); err != nil {
			t.Error("the worktree must stay")
		}
	})
	t.Run("known from the current pane", func(t *testing.T) {
		env, p, e := openMergedStream(t)
		env.mux.current = paneOf(env, e.Tab)
		if err := env.app.rm(p, e, rmOptions{}); err == nil || !strings.Contains(err.Error(), "jw rm web") {
			t.Fatalf("want refusal naming the command to run elsewhere, got %v", err)
		}
	})
	t.Run("fine from another tab", func(t *testing.T) {
		env, p, e := openMergedStream(t)
		env.mux.current = "p-elsewhere"
		if err := env.app.done(p, e); err != nil {
			t.Fatal(err)
		}
		if p.reg.Has(p.name, "web") {
			t.Fatal("done should have finished")
		}
	})
}

func TestCloseFromItsOwnTabSavesFirst(t *testing.T) {
	env, p, e := openMergedStream(t)
	t.Setenv("JW_ID", e.ID)

	// When the tab goes, jw (running inside it) dies: by then the registry
	// must already say closed.
	env.mux.onClose = func() {
		path, _ := registry.DefaultPath()
		reg, _ := registry.Load(path)
		got, _ := reg.Find(p.name, "web")
		if got.Tab != "" {
			t.Error("the registry still had the tab when it closed")
		}
	}
	if err := env.app.close(p, e, true); err != nil {
		t.Fatal(err)
	}
	if len(env.mux.closed) != 1 {
		t.Fatal("close from inside is allowed: the tab must close")
	}
}
