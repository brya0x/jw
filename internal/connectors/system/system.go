// Package system implements connectors.Shell for the operating system jw is
// built for. The OS-specific parts live in shell_unix.go and shell_windows.go;
// the compiler picks one by their build tags.
package system

import (
	"os"

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
