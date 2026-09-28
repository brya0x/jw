package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/brya0x/jw/internal/config"
	"github.com/brya0x/jw/internal/git"
	"github.com/brya0x/jw/internal/registry"
)

// project bundles what every command needs: the repo the cwd belongs to, its
// config and the registry.
type project struct {
	name    string
	repo    *git.Repo
	cfg     *config.Config
	reg     *registry.Registry
	regPath string
}

func openProject() (*project, error) {
	cwd, err := os.Getwd()
	if err != nil {
		return nil, err
	}
	repo, err := git.Open(cwd)
	if err != nil {
		return nil, err
	}
	name := git.ProjectName(repo.Remote)

	cfg, err := config.Load(repo.Root, repo.Remote, name)
	if err != nil {
		return nil, err
	}
	regPath, err := registry.DefaultPath()
	if err != nil {
		return nil, err
	}
	reg, err := registry.Load(regPath)
	if err != nil {
		return nil, err
	}
	return &project{name: name, repo: repo, cfg: cfg, reg: reg, regPath: regPath}, nil
}

// entry resolves the worktree a command acts on: the name or id prefix in
// args[0], or else the worktree that contains the cwd.
func (p *project) entry(args []string) (*registry.Entry, error) {
	if len(args) > 0 {
		return p.reg.Find(p.name, args[0])
	}

	cwd, err := os.Getwd()
	if err != nil {
		return nil, err
	}
	cwd = realpath(cwd)
	for i := range p.reg.Entries {
		e := &p.reg.Entries[i]
		dir := realpath(e.Path)
		if e.Project == p.name && (cwd == dir || strings.HasPrefix(cwd, dir+string(filepath.Separator))) {
			return e, nil
		}
	}
	return nil, fmt.Errorf("not inside a jw worktree; pass a name")
}

// realpath resolves symlinks (macOS /var → /private/var) so paths compare equal.
func realpath(p string) string {
	if r, err := filepath.EvalSymlinks(p); err == nil {
		return r
	}
	return p
}
