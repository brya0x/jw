package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/brya0x/jw/internal/git"
	"github.com/brya0x/jw/internal/registry"
)

// Every slot owns a block of ports: 20000 + slot*100 + offset.
const (
	portBlockStart = 20000
	portBlockSize  = 100
)

var validName = regexp.MustCompile(`^[a-z0-9][a-z0-9-]*$`)

func runNew(args []string) error {
	// Go's flag package stops at the first non-flag argument, so
	// `jw new web --from x` would leave --from unparsed. Take the name off
	// the front first, then parse whatever flags follow it.
	var name string
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		name, args = args[0], args[1:]
	}

	fs := flag.NewFlagSet("new", flag.ExitOnError)
	from := fs.String("from", "", "ref to start from (default origin/<default branch>)")
	branch := fs.String("branch", "", "branch name (default feat/<name>)")
	fs.Parse(args)

	if name == "" {
		name = fs.Arg(0)
	}
	if name == "" {
		return errors.New("usage: jw new <name> [--from <ref>] [--branch <branch>]")
	}
	if !validName.MatchString(name) {
		return fmt.Errorf("invalid name %q: use lowercase letters, digits and dashes", name)
	}

	cwd, err := os.Getwd()
	if err != nil {
		return err
	}
	repo, err := git.Open(cwd)
	if err != nil {
		return err
	}
	project := git.ProjectName(repo.Remote)

	regPath, err := registry.DefaultPath()
	if err != nil {
		return err
	}
	reg, err := registry.Load(regPath)
	if err != nil {
		return err
	}

	// Check everything that can fail cheaply before touching the disk.
	if reg.Has(project, name) {
		return fmt.Errorf("%s/%s already exists", project, name)
	}
	if *branch == "" {
		*branch = "feat/" + name
	}
	if repo.BranchExists(*branch) {
		return fmt.Errorf("branch %s already exists", *branch)
	}
	path := filepath.Join(repo.Root+"-wt", name)
	if _, err := os.Stat(path); err == nil {
		return fmt.Errorf("%s already exists", path)
	}

	ref := *from
	if ref == "" {
		def, err := repo.DefaultBranch()
		if err != nil {
			return err
		}
		ref = "origin/" + def
	}

	fmt.Printf("fetching origin…\n")
	if err := repo.Fetch(); err != nil {
		return err
	}
	if err := repo.AddWorktree(path, *branch, ref); err != nil {
		return err
	}

	entry := registry.Entry{
		ID:      registry.NewID(),
		Name:    name,
		Project: project,
		Branch:  *branch,
		Path:    path,
		Slot:    reg.NextSlot(),
		Created: time.Now().UTC(),
	}

	// From here on a worktree exists on disk. If anything fails, undo it so
	// a half-created stream never lingers outside the registry.
	if err := finishNew(repo, reg, regPath, entry); err != nil {
		_ = repo.RemoveWorktree(path)
		_ = repo.DeleteBranch(*branch)
		return fmt.Errorf("rolled back %s: %w", name, err)
	}

	fmt.Printf("created %s/%s\n  branch %s (from %s)\n  path   %s\n  slot   %d (ports %d–%d)\n",
		project, name, *branch, ref, path,
		entry.Slot, portBase(entry.Slot), portBase(entry.Slot)+portBlockSize-1)
	return nil
}

func finishNew(repo *git.Repo, reg *registry.Registry, regPath string, e registry.Entry) error {
	env := fmt.Sprintf("JW_ID=%s\nJW_NAME=%s\nJW_SLOT=%d\nJW_PORT_BASE=%d\n",
		e.ID, e.Name, e.Slot, portBase(e.Slot))
	if err := os.WriteFile(filepath.Join(e.Path, ".jw.env"), []byte(env), 0o644); err != nil {
		return err
	}
	if err := repo.Exclude(".jw.env"); err != nil {
		return err
	}
	reg.Add(e)
	return reg.Save(regPath)
}

func portBase(slot int) int {
	return portBlockStart + slot*portBlockSize
}
