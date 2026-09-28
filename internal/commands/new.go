package commands

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/brya0x/jw/internal/core/config"
	"github.com/brya0x/jw/internal/core/envfile"
	"github.com/brya0x/jw/internal/core/registry"
)

var validName = regexp.MustCompile(`^[a-z0-9][a-z0-9-]*$`)

type newOptions struct {
	name    string
	from    string // ref to start from; default origin/<default branch>
	branch  string // default from the config's template
	noSetup bool
}

func (a *App) runNew(args []string) error {
	var o newOptions
	o.name, args = splitName(args)
	fs := flag.NewFlagSet("new", flag.ExitOnError)
	fs.StringVar(&o.from, "from", "", "ref to start from (default origin/<default branch>)")
	fs.StringVar(&o.branch, "branch", "", "branch name (default from config, feat/<name>)")
	fs.BoolVar(&o.noSetup, "no-setup", false, "skip the config's setup commands")
	fs.Parse(args)
	if o.name == "" {
		o.name = fs.Arg(0)
	}
	if o.name == "" {
		return errors.New("usage: jw new <name> [--from <ref>] [--branch <branch>] [--no-setup]")
	}

	dir, err := os.Getwd()
	if err != nil {
		return err
	}
	p, err := loadProject(dir)
	if err != nil {
		return err
	}
	return a.newStream(p, o)
}

// newStream creates the worktree, registers it and runs setup.
func (a *App) newStream(p *project, o newOptions) error {
	if !validName.MatchString(o.name) {
		return fmt.Errorf("invalid name %q: use lowercase letters, digits and dashes", o.name)
	}

	// Check everything that can fail cheaply before touching the disk.
	if p.reg.Has(p.name, o.name) {
		return fmt.Errorf("%s/%s already exists", p.name, o.name)
	}
	base, err := p.repo.DefaultBranch()
	if err != nil {
		return err
	}
	slot := p.reg.NextSlot()
	vars := p.cfg.Vars(o.name, base, slot)

	branch := o.branch
	if branch == "" {
		if branch, err = config.Expand(p.cfg.Branch, vars); err != nil {
			return err
		}
	}
	if p.repo.BranchExists(branch) {
		return fmt.Errorf("branch %s already exists", branch)
	}
	path := filepath.Join(p.cfg.Root, o.name)
	if _, err := os.Stat(path); err == nil {
		return fmt.Errorf("%s already exists", path)
	}
	ref := o.from
	if ref == "" {
		ref = "origin/" + base
	}

	if p.cfg.Source != "" {
		a.printf("config  %s\n", p.cfg.Source)
	}
	a.printf("fetching origin…\n")
	if err := p.repo.Fetch(); err != nil {
		return err
	}
	if err := p.repo.AddWorktree(path, branch, ref); err != nil {
		return err
	}

	e := registry.Entry{
		ID:      registry.NewID(),
		Name:    o.name,
		Project: p.name,
		Branch:  branch,
		Path:    path,
		Slot:    slot,
		Created: time.Now().UTC(),
	}

	// From here on a worktree exists on disk. If anything fails, undo it so
	// a half-created stream never lingers outside the registry.
	if err := a.provision(p, e, vars); err != nil {
		_ = p.repo.RemoveWorktree(path)
		_ = p.repo.DeleteBranch(branch)
		return fmt.Errorf("rolled back %s: %w", o.name, err)
	}

	a.printf("created %s/%s\n  branch  %s (from %s)\n  path    %s\n  slot    %d\n",
		p.name, o.name, branch, ref, path, slot)
	for _, svc := range sortedKeys(vars.Ports) {
		a.printf("  %-14s %d\n", svc, vars.Ports[svc])
	}

	if o.noSetup || len(p.cfg.Setup) == 0 {
		return nil
	}
	// A failed setup (network, lockfile…) keeps the worktree: it is fixable
	// in place, and deleting a fresh install would only waste the retry.
	if err := a.setup(p.cfg, e, vars); err != nil {
		return fmt.Errorf("%w\nthe worktree is kept; fix it and run `jw setup %s`", err, o.name)
	}
	return nil
}

// runSetup is `jw setup [name]`: re-run the setup commands of a worktree.
func (a *App) runSetup(args []string) error {
	name, _ := splitName(args)
	p, e, err := open(name)
	if err != nil {
		return err
	}
	vars, err := p.vars(e)
	if err != nil {
		return err
	}
	return a.setup(p.cfg, *e, vars)
}

// provision writes .jw.env, copies env files and registers the entry.
func (a *App) provision(p *project, e registry.Entry, vars config.Vars) error {
	env := strings.Join(jwEnv(e, vars), "\n") + "\n"
	if err := os.WriteFile(filepath.Join(e.Path, ".jw.env"), []byte(env), 0o644); err != nil {
		return err
	}
	if err := p.repo.Exclude(".jw.env"); err != nil {
		return err
	}
	if err := a.copyEnvFiles(p, e, vars); err != nil {
		return err
	}
	p.reg.Add(e)
	return p.save()
}

// copyEnvFiles brings gitignored env files from the main checkout into the
// worktree and points their ports at this slot. A file missing in the main
// checkout is skipped with a warning: not every clone has every app set up.
func (a *App) copyEnvFiles(p *project, e registry.Entry, vars config.Vars) error {
	for _, f := range p.cfg.Env {
		data, err := os.ReadFile(filepath.Join(p.repo.Root, f.File))
		if errors.Is(err, os.ErrNotExist) {
			a.warnf("warning: %s not found in the main checkout, skipped\n", f.File)
			continue
		}
		if err != nil {
			return err
		}

		set := make(map[string]string, len(f.Set))
		for k, v := range f.Set {
			if set[k], err = config.Expand(v, vars); err != nil {
				return err
			}
		}

		dst := filepath.Join(e.Path, f.File)
		if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
			return err
		}
		if err := os.WriteFile(dst, envfile.Set(data, set), 0o600); err != nil {
			return err
		}
		a.printf("env     %s\n", f.File)
	}
	return nil
}

// setup runs each setup command in the worktree with the JW_* variables
// exported, streaming output as it goes.
func (a *App) setup(cfg *config.Config, e registry.Entry, vars config.Vars) error {
	env := append(os.Environ(), jwEnv(e, vars)...)
	for _, raw := range cfg.Setup {
		cmdline, err := config.Expand(raw, vars)
		if err != nil {
			return err
		}
		a.printf("\n$ %s\n", cmdline)
		if err := a.Shell.Run(e.Path, env, cmdline, a.In, a.Out, a.Err); err != nil {
			return fmt.Errorf("setup failed: %s: %w", cmdline, err)
		}
	}
	return nil
}
