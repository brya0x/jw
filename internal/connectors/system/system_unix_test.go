//go:build !windows

package system

import (
	"net"
	"os"
	"path/filepath"
	"testing"
)

func TestPortOwnerSeesARealListener(t *testing.T) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := l.Addr().(*net.TCPAddr).Port

	owner, busy := Shell{}.PortOwner(port)
	if !busy {
		t.Fatal("a port we listen on must be busy")
	}
	// lsof may be missing on a minimal machine: then the owner is unknown,
	// but if it's there it must name this very test process.
	if owner.PID != 0 && owner.PID != os.Getpid() {
		t.Fatalf("owner pid %d, want %d", owner.PID, os.Getpid())
	}
	if wd, _ := os.Getwd(); owner.Cwd != "" && realpath(owner.Cwd) != realpath(wd) {
		t.Fatalf("owner cwd %q, want %q", owner.Cwd, wd)
	}

	l.Close()
	if _, busy := (Shell{}).PortOwner(port); busy {
		t.Fatal("a closed port must be free")
	}
}

func realpath(p string) string {
	if r, err := filepath.EvalSymlinks(p); err == nil {
		return r
	}
	return p
}
