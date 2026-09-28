//go:build !windows

package system

import (
	"context"
	"io"
	"os"
	"os/exec"
	"syscall"
)

func (Shell) Run(dir string, env []string, cmdline string, stdin io.Reader, stdout, stderr io.Writer) error {
	cmd := exec.Command("sh", "-c", cmdline)
	cmd.Dir, cmd.Env = dir, env
	cmd.Stdin, cmd.Stdout, cmd.Stderr = stdin, stdout, stderr
	return cmd.Run()
}

// Replace execs the shell in place of jw: same PID, same terminal, so
// colours, ctrl+c and interactive keys behave as if typed by hand.
func (Shell) Replace(dir string, env []string, cmdline string) error {
	sh, err := exec.LookPath("sh")
	if err != nil {
		return err
	}
	if err := os.Chdir(dir); err != nil {
		return err
	}
	return syscall.Exec(sh, []string{"sh", "-c", cmdline}, env)
}

// Group puts sh and its children in their own process group and signals the
// whole group on cancel. Killing only sh would leave the server it started
// running, still holding its port.
func (Shell) Group(ctx context.Context, dir string, env []string, cmdline string) *exec.Cmd {
	cmd := exec.CommandContext(ctx, "sh", "-c", cmdline)
	cmd.Dir, cmd.Env = dir, env
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error { return syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM) }
	return cmd
}
