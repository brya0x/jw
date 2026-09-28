package main

import (
	"bytes"
	"strings"
	"sync"
	"testing"

	"github.com/brya0x/jw/internal/config"
)

func TestPrefixWriterWholeLines(t *testing.T) {
	var out bytes.Buffer
	w := &prefixWriter{w: &out, prefix: "[1] ", mu: &sync.Mutex{}}

	// Writes arrive in arbitrary chunks; lines must come out whole.
	w.Write([]byte("hel"))
	w.Write([]byte("lo\nwor"))
	w.Write([]byte("ld\npartial"))
	w.Flush()

	if want := "[1] hello\n[1] world\n[1] partial\n"; out.String() != want {
		t.Fatalf("got %q want %q", out.String(), want)
	}
}

func TestDevCommandsExpandPorts(t *testing.T) {
	cfg := &config.Config{
		Ports: map[string]int{"web": 0, "api": 2},
		Dev: map[string][]string{
			"web": {"watch-packages", "vite --port {port.web} --strictPort"},
		},
	}
	cmds, err := devCommands(cfg, "web", cfg.Vars("web", "main", 3))
	if err != nil {
		t.Fatal(err)
	}
	if cmds[1] != "vite --port 20300 --strictPort" {
		t.Fatalf("got %q", cmds[1])
	}

	_, err = devCommands(cfg, "wbe", cfg.Vars("web", "main", 3))
	if err == nil || !strings.Contains(err.Error(), "have: web") {
		t.Fatalf("want list of known services, got %v", err)
	}
}
