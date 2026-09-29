package commands

import (
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"

	"github.com/brya0x/jw/internal/connectors"
	"github.com/brya0x/jw/internal/core/config"
	"github.com/brya0x/jw/internal/core/registry"
)

func (a *App) runDev(args []string) error {
	service, args := splitName(args)
	fs := flag.NewFlagSet("dev", flag.ExitOnError)
	worktree := fs.String("w", "", "worktree name (default: the one you're in)")
	fs.Parse(args)

	p, e, err := open(*worktree)
	if err != nil {
		return err
	}
	return a.dev(p, e, service)
}

// dev starts a service on the worktree's ports; with no service it lists them.
func (a *App) dev(p *project, e *registry.Entry, service string) error {
	vars, err := p.vars(e)
	if err != nil {
		return err
	}
	if service == "" {
		return a.listServices(p.cfg, vars)
	}
	cmds, err := devCommands(p.cfg, service, vars)
	if err != nil {
		return err
	}
	// Check the ports before starting anything: a server that finds its port
	// taken fails late, from inside the app, often half-way up.
	if err := a.checkPorts(p, e, service, vars); err != nil {
		return err
	}
	env := append(os.Environ(), jwEnv(*e, vars)...)

	if len(cmds) == 1 {
		a.printf("$ %s\n", cmds[0])
		return a.Shell.Replace(e.Path, env, cmds[0])
	}
	if mux, err := a.NewMux(); err == nil {
		if pane, inside := mux.CurrentPane(); inside {
			return a.devInSplits(mux, pane, cmds, e.Path, jwEnv(*e, vars), env)
		}
	}
	return a.devConcurrently(cmds, e.Path, env)
}

// checkPorts fails if any port the service's commands use is already taken,
// naming the process, and the stream when it's one of jw's.
func (a *App) checkPorts(p *project, e *registry.Entry, service string, vars config.Vars) error {
	var taken []string
	for _, raw := range p.cfg.Dev[service] {
		for _, svc := range config.PortsIn(raw) {
			port, ok := vars.Ports[svc]
			if !ok {
				continue
			}
			owner, busy := a.Shell.PortOwner(port)
			if !busy {
				continue
			}
			taken = append(taken, fmt.Sprintf("  %d (%s): %s", port, svc, a.describeOwner(p, e, owner)))
		}
	}
	if len(taken) == 0 {
		return nil
	}
	return fmt.Errorf("port(s) already in use, not starting %s:\n%s", service, strings.Join(taken, "\n"))
}

// describeOwner says who holds a port, in terms of streams when it can.
func (a *App) describeOwner(p *project, e *registry.Entry, o connectors.PortOwner) string {
	if o.PID == 0 {
		return "taken by a process jw can't see (another user, or no lsof)"
	}
	who := fmt.Sprintf("pid %d", o.PID)
	if o.Cmdline != "" {
		who += " — " + o.Cmdline
	}
	if o.Cwd == "" {
		return who
	}
	if inside(o.Cwd, e.Path) {
		return who + "\n      already running in this worktree: stop it, or use the pane it runs in"
	}
	for _, other := range p.reg.Entries {
		if inside(o.Cwd, other.Path) {
			return fmt.Sprintf("%s\n      started from stream %s — its slot should never overlap this one's; `jw ls` to check", who, other.Name)
		}
	}
	return who + "\n      in " + o.Cwd
}

// inside reports whether dir is root or below it.
func inside(dir, root string) bool {
	dir, root = realpath(dir), realpath(root)
	return dir == root || strings.HasPrefix(dir, root+string(os.PathSeparator))
}

// devCommands expands a service's commands for this worktree.
func devCommands(cfg *config.Config, service string, vars config.Vars) ([]string, error) {
	raw, ok := cfg.Dev[service]
	if !ok || len(raw) == 0 {
		names := sortedKeys(cfg.Dev)
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

func (a *App) listServices(cfg *config.Config, vars config.Vars) error {
	if len(cfg.Dev) == 0 {
		a.printf("no [dev] services in config %s\n", configName(cfg))
		return nil
	}
	for _, n := range sortedKeys(cfg.Dev) {
		cmds, err := devCommands(cfg, n, vars)
		if err != nil {
			return err
		}
		a.printf("%s\n", n)
		for _, c := range cmds {
			a.printf("  $ %s\n", c)
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

// devInSplits gives every command after the first its own pane, split off
// the current one, then runs the first command here.
func (a *App) devInSplits(mux connectors.Multiplexer, current string, cmds []string, dir string, jwEnv, env []string) error {
	for _, c := range cmds[1:] {
		pane, err := mux.Split(current, "right", 0.5, dir, jwEnv)
		if err != nil {
			return err
		}
		_ = mux.RenamePane(pane.ID, "dev")
		if err := mux.Run(pane.ID, c); err != nil {
			return err
		}
		current = pane.ID
	}
	a.printf("$ %s\n", cmds[0])
	return a.Shell.Replace(dir, env, cmds[0])
}

// devConcurrently runs every command at once, each line prefixed with its
// number, until all exit. ctrl+c stops all of them.
func (a *App) devConcurrently(cmds []string, dir string, env []string) error {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	var (
		wg   sync.WaitGroup
		mu   sync.Mutex // one line at a time on the terminal
		errs = make([]error, len(cmds))
	)
	for i, c := range cmds {
		prefix := fmt.Sprintf("[%d] ", i+1)
		a.printf("%s$ %s\n", prefix, c)

		// Group makes cancelling stop the command and whatever it started:
		// how that works differs per OS, so it's the Shell's job.
		cmd := a.Shell.Group(ctx, dir, env, c)
		out := &prefixWriter{w: a.Out, prefix: prefix, mu: &mu}
		errOut := &prefixWriter{w: a.Err, prefix: prefix, mu: &mu}
		cmd.Stdout, cmd.Stderr = out, errOut

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
