package herdr

import (
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/connectors"
)

// fakeHerdr points JW_HERDR at a script that logs its args and prints reply.
func fakeHerdr(t *testing.T, reply string, exit int) (*Client, string) {
	t.Helper()
	dir := t.TempDir()
	log := filepath.Join(dir, "args")
	// Like the real herdr: replies on stdout, but errors (exit != 0) on stderr.
	redirect := ""
	if exit != 0 {
		redirect = " >&2"
	}
	script := "#!/bin/sh\necho \"$@\" >> " + log + "\ncat" + redirect + " <<'EOF'\n" + reply + "\nEOF\nexit " + strconv.Itoa(exit) + "\n"
	bin := filepath.Join(dir, "herdr")
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("JW_HERDR", bin)
	c, err := New()
	if err != nil {
		t.Fatal(err)
	}
	return c, log
}

func TestCreateTabParsesIDsAndPassesEnv(t *testing.T) {
	c, log := fakeHerdr(t, `{"id":"x","result":{"tab":{"tab_id":"w4:t2","label":"web"},"root_pane":{"pane_id":"w4:p7"}}}`, 0)

	tab, pane, err := c.CreateTab("w4", "/src/web", "web", []string{"JW_SLOT=1", "JW_NAME=web"})
	if err != nil {
		t.Fatal(err)
	}
	if tab.ID != "w4:t2" || pane.ID != "w4:p7" {
		t.Fatalf("got tab %s pane %s", tab.ID, pane.ID)
	}

	args, _ := os.ReadFile(log)
	for _, want := range []string{"--no-focus", "--env JW_SLOT=1", "--env JW_NAME=web"} {
		if !strings.Contains(string(args), want) {
			t.Errorf("args %q missing %q", args, want)
		}
	}
}

func TestErrorReply(t *testing.T) {
	c, _ := fakeHerdr(t, `{"error":{"code":"tab_not_found","message":"tab w9:t1 not found"},"id":"x"}`, 1)

	_, err := c.GetTab("w9:t1")
	if !errors.Is(err, connectors.ErrNotFound) {
		t.Fatalf("want not-found, got %v", err)
	}
}

func TestAgentNotReadyMapsToContract(t *testing.T) {
	c, _ := fakeHerdr(t, `{"error":{"code":"agent_not_ready","message":"blocked"},"id":"x"}`, 1)
	err := c.StartAgent("web", "claude", "w1:p2", nil)
	if !errors.Is(err, connectors.ErrAgentNotReady) || errors.Is(err, connectors.ErrNotFound) {
		t.Fatalf("got %v", err)
	}
}

func TestSilentSuccess(t *testing.T) {
	c, _ := fakeHerdr(t, "", 0) // `pane run` prints nothing when it works
	if err := c.Run("w4:p7", "nvim"); err != nil {
		t.Fatalf("empty output with exit 0 is success, got %v", err)
	}
}

func TestStartAgentSeparatesArgs(t *testing.T) {
	c, log := fakeHerdr(t, `{"id":"x","result":{}}`, 0)
	if err := c.StartAgent("web", "claude", "w4:p8", []string{"--continue"}); err != nil {
		t.Fatal(err)
	}
	args, _ := os.ReadFile(log)
	if want := "agent start web --kind claude --pane w4:p8 -- --continue"; strings.TrimSpace(string(args)) != want {
		t.Fatalf("got %q", args)
	}
}
