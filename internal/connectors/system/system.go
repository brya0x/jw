// Package system implements connectors.Shell for the operating system jw is
// built for. The OS-specific parts live in shell_unix.go and shell_windows.go;
// the compiler picks one by their build tags.
package system

import (
	"fmt"
	"net"
	"os"
	"time"

	"golang.org/x/term"

	"github.com/brya0x/jw/internal/connectors"
)

// Shell is the connectors.Shell of this OS.
type Shell struct{}

// The compiler checks here that Shell satisfies the contract.
var _ connectors.Shell = Shell{}

func (Shell) IsTerminal(f *os.File) bool {
	return term.IsTerminal(int(f.Fd()))
}

// listening reports whether anything accepts connections on the port, over
// IPv4 or IPv6 — dev servers often bind only one of them (Vite: localhost,
// which can resolve to ::1). A port we can't bind counts as taken too.
func listening(port int) bool {
	for _, host := range []string{"127.0.0.1", "::1"} {
		c, err := net.DialTimeout("tcp", net.JoinHostPort(host, fmt.Sprint(port)), 200*time.Millisecond)
		if err == nil {
			c.Close()
			return true
		}
	}
	l, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", port))
	if err != nil {
		return true
	}
	l.Close()
	return false
}
