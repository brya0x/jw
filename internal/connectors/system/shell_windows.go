//go:build windows

package system

import (
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"strconv"
	"syscall"

	"github.com/brya0x/jw/internal/connectors"
)

func (Shell) Run(dir string, env []string, cmdline string, stdin io.Reader, stdout, stderr io.Writer) error {
	cmd := exec.Command("cmd", "/C", cmdline)
	cmd.Dir, cmd.Env = dir, env
	cmd.Stdin, cmd.Stdout, cmd.Stderr = stdin, stdout, stderr
	return cmd.Run()
}

// Replace: Windows can't swap the running process for another, so run the
// command on this terminal, wait, and exit with its code.
func (s Shell) Replace(dir string, env []string, cmdline string) error {
	err := s.Run(dir, env, cmdline, os.Stdin, os.Stdout, os.Stderr)
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		os.Exit(exit.ExitCode())
	}
	if err != nil {
		return err
	}
	os.Exit(0)
	return nil
}

// Group starts the command in a new process group and, on cancel, kills its
// whole process tree: taskkill /T is the Windows way to reach the children.
func (Shell) Group(ctx context.Context, dir string, env []string, cmdline string) *exec.Cmd {
	cmd := exec.CommandContext(ctx, "cmd", "/C", cmdline)
	cmd.Dir, cmd.Env = dir, env
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: syscall.CREATE_NEW_PROCESS_GROUP}
	cmd.Cancel = func() error {
		return exec.Command("taskkill", "/T", "/F", "/PID", strconv.Itoa(cmd.Process.Pid)).Run()
	}
	return cmd
}

// PortOwner reports a busy port. Naming its process (netstat -ano, then
// tasklist) isn't implemented on Windows yet.
func (Shell) PortOwner(port int) (connectors.PortOwner, bool) {
	return connectors.PortOwner{}, listening(port)
}
