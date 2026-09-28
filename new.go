package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/brya0x/jw/internal/config"
	"github.com/brya0x/jw/internal/envfile"
	"github.com/brya0x/jw/internal/registry"
)

var validName = regexp.MustCompile(`^[a-z0-9][a-z0-9-]*$`)

func runNew(args []string) error {
	name, args := splitName(args)
	fs := flag.NewFlagSet("new", flag.ExitOnError)
	from := fs.String("from", "", "ref to start from (default origin/<default branch>)")
	branch := fs.String("branch", "", "branch name (default from config, feat/<name>)")
	noSetup := fs.Bool("no-setup", false, "skip the config's setup commands")
	fs.Parse(args)

	if name == "" {
		name = fs.Arg(0)
	}
	if name == "" {
		return errors.New("usage: jw new <name> [--from <ref>] [--branch <branch>] [--no-setup]")
	}
	if !validName.MatchString(name) {
		return fmt.Errorf("invalid name %q: use lowercase letters, digits and dashes", name)
	}

	p, err := openProject()
	if err != nil {
		return err
	}

	// Check everything that can fail cheaply before touching the disk.
	if p.reg.Has(p.name, name) {
		return fmt.Errorf("%s/%s already exists", p.name, name)
	}
	base, err := p.repo.DefaultBranch()
	if err != nil {
		return err
	}
	slot := p.reg.NextSlot()
	vars := p.cfg.Vars(name, base, slot)

	if *branch == "" {
		if *branch, err = config.Expand(p.cfg.Branch, vars); err != nil {
			return err
		}
	}
	if p.repo.BranchExists(*branch) {
		return fmt.Errorf("branch %s already exists", *branch)
	}
	path := filepath.Join(p.cfg.Root, name)
	if _, err := os.Stat(path); err == nil {
		return fmt.Errorf("%s already exists", path)
	}
	ref := *from
	if ref == "" {
		ref = "origin/" + base
	}

	if p.cfg.Source != "" {
		fmt.Printf("config  %s\n", p.cfg.Source)
	}
	fmt.Println("fetching origin…")
	if err := p.repo.Fetch(); err != nil {
		return err
	}
	if err := p.repo.AddWorktree(path, *branch, ref); err != nil {
		return err
	}

	entry := registry.Entry{
		ID:      registry.NewID(),
		Name:    name,
		Project: p.name,
		Branch:  *branch,
		Path:    path,
		Slot:    slot,
		Created: time.Now().UTC(),
	}

	// From here on a worktree exists on disk. If anything fails, undo it so
	// a half-created stream never lingers outside the registry.
	if err := provision(p, entry, vars); err != nil {
		_ = p.repo.RemoveWorktree(path)
		_ = p.repo.DeleteBranch(*branch)
		return fmt.Errorf("rolled back %s: %w", name, err)
	}

	fmt.Printf("created %s/%s\n  branch  %s (from %s)\n  path    %s\n  slot    %d\n",
		p.name, name, *branch, ref, path, slot)
	printPorts(vars.Ports)

	if *noSetup || len(p.cfg.Setup) == 0 {
		return nil
	}
	// A failed setup (network, lockfile…) keeps the worktree: it is fixable
	// in place, and deleting a fresh install would only waste the retry.
	if err := runSetup(p.cfg, entry, vars); err != nil {
		return fmt.Errorf("%w\nthe worktree is kept; fix it and run `jw setup %s`", err, name)
	}
	return nil
}

// runSetupCmd is `jw setup [name]`: re-run the setup commands of a worktree,
// the one you are standing in if no name is given.
func runSetupCmd(args []string) error {
	p, err := openProject()
	if err != nil {
		return err
	}
	e, err := p.entry(args)
	if err != nil {
		return err
	}
	base, err := p.repo.DefaultBranch()
	if err != nil {
		return err
	}
	return runSetup(p.cfg, *e, p.cfg.Vars(e.Name, base, e.Slot))
}

// provision writes .jw.env, copies env files and registers the entry.
func provision(p *project, e registry.Entry, vars config.Vars) error {
	if err := os.WriteFile(filepath.Join(e.Path, ".jw.env"), []byte(jwEnvFile(e, vars)), 0o644); err != nil {
		return err
	}
	if err := p.repo.Exclude(".jw.env"); err != nil {
		return err
	}
	if err := copyEnvFiles(p, e, vars); err != nil {
		return err
	}
	p.reg.Add(e)
	return p.reg.Save(p.regPath)
}

// copyEnvFiles brings gitignored env files from the main checkout into the
// worktree and points their ports at this slot. A file missing in the main
// checkout is skipped with a warning: not every clone has every app set up.
func copyEnvFiles(p *project, e registry.Entry, vars config.Vars) error {
	for _, f := range p.cfg.Env {
		src := filepath.Join(p.repo.Root, f.File)
		data, err := os.ReadFile(src)
		if errors.Is(err, os.ErrNotExist) {
			fmt.Fprintf(os.Stderr, "warning: %s not found in the main checkout, skipped\n", f.File)
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
		fmt.Printf("env     %s\n", f.File)
	}
	return nil
}

// runSetup runs each setup command through sh, in the worktree, with the
// JW_* variables exported, streaming output as it goes.
func runSetup(cfg *config.Config, e registry.Entry, vars config.Vars) error {
	env := append(os.Environ(), jwEnv(e, vars)...)
	for _, raw := range cfg.Setup {
		cmdline, err := config.Expand(raw, vars)
		if err != nil {
			return err
		}
		fmt.Printf("\n$ %s\n", cmdline)

		cmd := exec.Command("sh", "-c", cmdline)
		cmd.Dir = e.Path
		cmd.Env = env
		cmd.Stdin, cmd.Stdout, cmd.Stderr = os.Stdin, os.Stdout, os.Stderr
		if err := cmd.Run(); err != nil {
			return fmt.Errorf("setup failed: %s: %w", cmdline, err)
		}
	}
	return nil
}

// jwEnv is the list of JW_* variables for a worktree, as KEY=VALUE pairs.
func jwEnv(e registry.Entry, vars config.Vars) []string {
	env := []string{
		"JW_ID=" + e.ID,
		"JW_NAME=" + e.Name,
		fmt.Sprintf("JW_SLOT=%d", e.Slot),
		fmt.Sprintf("JW_PORT_BASE=%d", config.PortBase(e.Slot)),
	}
	for _, svc := range sortedKeys(vars.Ports) {
		env = append(env, fmt.Sprintf("%s=%d", config.EnvName(svc), vars.Ports[svc]))
	}
	return env
}

func jwEnvFile(e registry.Entry, vars config.Vars) string {
	return strings.Join(jwEnv(e, vars), "\n") + "\n"
}

func printPorts(ports map[string]int) {
	for _, svc := range sortedKeys(ports) {
		fmt.Printf("  %-14s %d\n", svc, ports[svc])
	}
}

// sortedKeys exists because Go randomises map iteration order on purpose.
func sortedKeys(m map[string]int) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
