//go:build !windows

package system

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"syscall"

	"github.com/brya0x/jw/internal/connectors"
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

// PortOwner asks lsof who listens on the port. Without lsof, or when the
// process belongs to another user, the port is still reported busy, with an
// unknown owner.
func (Shell) PortOwner(port int) (connectors.PortOwner, bool) {
	if !listening(port) {
		return connectors.PortOwner{}, false
	}
	out, err := exec.Command("lsof", "-nP", fmt.Sprintf("-iTCP:%d", port), "-sTCP:LISTEN", "-t").Output()
	if err != nil {
		return connectors.PortOwner{}, true
	}
	pid, err := strconv.Atoi(strings.Fields(string(out) + " 0")[0])
	if err != nil || pid == 0 {
		return connectors.PortOwner{}, true
	}

	owner := connectors.PortOwner{PID: pid}
	if cmd, err := exec.Command("ps", "-o", "command=", "-p", strconv.Itoa(pid)).Output(); err == nil {
		owner.Cmdline = strings.TrimSpace(string(cmd))
	}
	// -Fn prints fields one per line; the cwd is the line starting with n.
	if cwd, err := exec.Command("lsof", "-a", "-p", strconv.Itoa(pid), "-d", "cwd", "-Fn").Output(); err == nil {
		for _, line := range strings.Split(string(cwd), "\n") {
			if strings.HasPrefix(line, "n") {
				owner.Cwd = line[1:]
			}
		}
	}
	return owner, true
}
