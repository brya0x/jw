package main

import (
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"sort"
	"strings"
	"sync"
	"syscall"

	"github.com/brya0x/jw/internal/config"
	"github.com/brya0x/jw/internal/herdr"
)

// runDev is `jw dev [service] [-w name]`: start a service on this worktree's
// ports. Without a service it lists what the config defines.
func runDev(args []string) error {
	service, args := splitName(args)
	fs := flag.NewFlagSet("dev", flag.ExitOnError)
	worktree := fs.String("w", "", "worktree name (default: the one you're in)")
	fs.Parse(args)

	p, err := openProject()
	if err != nil {
		return err
	}
	e, err := p.entry(nameArgs(*worktree))
	if err != nil {
		return err
	}
	base, err := p.repo.DefaultBranch()
	if err != nil {
		return err
	}
	vars := p.cfg.Vars(e.Name, base, e.Slot)

	if service == "" {
		return listServices(p.cfg, vars)
	}
	cmds, err := devCommands(p.cfg, service, vars)
	if err != nil {
		return err
	}
	env := append(os.Environ(), jwEnv(*e, vars)...)

	switch {
	case len(cmds) == 1:
		return execShell(cmds[0], e.Path, env)
	case os.Getenv("HERDR_ENV") == "1" && os.Getenv("HERDR_PANE_ID") != "":
		return runInSplits(cmds, e.Path, jwEnv(*e, vars), env)
	default:
		return runConcurrently(cmds, e.Path, env)
	}
}

// devCommands expands a service's commands for this worktree.
func devCommands(cfg *config.Config, service string, vars config.Vars) ([]string, error) {
	raw, ok := cfg.Dev[service]
	if !ok || len(raw) == 0 {
		names := make([]string, 0, len(cfg.Dev))
		for n := range cfg.Dev {
			names = append(names, n)
		}
		sort.Strings(names)
		if len(names) == 0 {
			return nil, fmt.Errorf("no [dev] services in config %s", configName(cfg))
		}
		return nil, fmt.Errorf("unknown service %q; have: %s", service, strings.Join(names, ", "))
	}
	cmds := make([]string, len(raw))
	for i, r := range raw {
		c, err := config.Expand(r, vars)
		if err != nil {
			return nil, err
		}
		cmds[i] = c
	}
	return cmds, nil
}

func listServices(cfg *config.Config, vars config.Vars) error {
	if len(cfg.Dev) == 0 {
		fmt.Printf("no [dev] services in config %s\n", configName(cfg))
		return nil
	}
	names := make([]string, 0, len(cfg.Dev))
	for n := range cfg.Dev {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, n := range names {
		cmds, err := devCommands(cfg, n, vars)
		if err != nil {
			return err
		}
		fmt.Printf("%s\n", n)
		for _, c := range cmds {
			fmt.Printf("  $ %s\n", c)
		}
	}
	return nil
}

func configName(cfg *config.Config) string {
	if cfg.Source == "" {
		return "(none — using defaults)"
	}
	return cfg.Source
}

// execShell replaces the jw process with `sh -c cmdline`. The service then
// owns the terminal directly: colours, ctrl+c and interactive keys (Expo's
// "press i for iOS") behave as if you had typed it yourself.
func execShell(cmdline, dir string, env []string) error {
	sh, err := exec.LookPath("sh")
	if err != nil {
		return err
	}
	if err := os.Chdir(dir); err != nil {
		return err
	}
	fmt.Printf("$ %s\n", cmdline)
	return syscall.Exec(sh, []string{"sh", "-c", cmdline}, env)
}

// runInSplits gives every command after the first its own herdr pane,
// split off the current one, then execs the first command here.
func runInSplits(cmds []string, dir string, jwEnv, env []string) error {
	h, err := herdr.New()
	if err != nil {
		return err
	}
	current := os.Getenv("HERDR_PANE_ID")
	for _, c := range cmds[1:] {
		pane, err := h.Split(current, "right", 0.5, dir, jwEnv)
		if err != nil {
			return err
		}
		_ = h.RenamePane(pane.ID, "dev")
		if err := h.Run(pane.ID, c); err != nil {
			return err
		}
		current = pane.ID
	}
	return execShell(cmds[0], dir, env)
}

// runConcurrently runs every command at once, each line prefixed with its
// number, until all exit. ctrl+c stops all of them.
func runConcurrently(cmds []string, dir string, env []string) error {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	var (
		wg   sync.WaitGroup
		mu   sync.Mutex // one line at a time on the terminal
		errs = make([]error, len(cmds))
	)
	for i, c := range cmds {
		prefix := fmt.Sprintf("[%d] ", i+1)
		fmt.Printf("%s$ %s\n", prefix, c)

		cmd := exec.CommandContext(ctx, "sh", "-c", c)
		cmd.Dir, cmd.Env = dir, env
		out := &prefixWriter{w: os.Stdout, prefix: prefix, mu: &mu}
		errOut := &prefixWriter{w: os.Stderr, prefix: prefix, mu: &mu}
		cmd.Stdout, cmd.Stderr = out, errOut

		// sh starts the real server as a child. Put both in their own
		// process group and signal the whole group, or cancelling would
		// kill sh and leave the server running with the port taken.
		cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
		cmd.Cancel = func() error { return syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM) }

		wg.Add(1)
		go func() {
			defer wg.Done()
			errs[i] = cmd.Run()
			out.Flush()
			errOut.Flush()
		}()
	}
	wg.Wait()

	if ctx.Err() != nil {
		return nil // stopped with ctrl+c: not a failure
	}
	for i, err := range errs {
		if err != nil {
			errs[i] = fmt.Errorf("[%d] %s: %w", i+1, cmds[i], err)
		}
	}
	return errors.Join(errs...)
}

// prefixWriter writes whole lines, each starting with prefix. Output from
// several processes shares mu so lines never interleave mid-line.
type prefixWriter struct {
	w      io.Writer
	prefix string
	mu     *sync.Mutex
	buf    []byte
}

func (p *prefixWriter) Write(b []byte) (int, error) {
	p.buf = append(p.buf, b...)
	for {
		i := bytes.IndexByte(p.buf, '\n')
		if i < 0 {
			break
		}
		p.writeLine(p.buf[:i])
		p.buf = p.buf[i+1:]
	}
	return len(b), nil
}

// Flush writes a last line that had no trailing newline.
func (p *prefixWriter) Flush() {
	if len(p.buf) > 0 {
		p.writeLine(p.buf)
		p.buf = nil
	}
}

func (p *prefixWriter) writeLine(line []byte) {
	p.mu.Lock()
	defer p.mu.Unlock()
	fmt.Fprintf(p.w, "%s%s\n", p.prefix, line)
}
