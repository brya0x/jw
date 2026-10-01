package commands

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/config"
	"github.com/brya0x/jw/internal/core/registry"
)

// project bundles what every command needs: the repo, its config and the
// registry.
type project struct {
	name    string
	repo    *git.Repo
	cfg     *config.Config
	reg     *registry.Registry
	regPath string
}

// loadProject opens the project that dir belongs to.
func loadProject(dir string) (*project, error) {
	repo, err := git.Open(dir)
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

func (p *project) save() error {
	return p.reg.Save(p.regPath)
}

// resolve finds the worktree a command acts on: by name or id prefix if one
// is given, else the worktree that contains dir.
func (p *project) resolve(name, dir string) (*registry.Entry, error) {
	if name != "" {
		return p.reg.Find(p.name, name)
	}
	dir = realpath(dir)
	for i := range p.reg.Entries {
		e := &p.reg.Entries[i]
		root := realpath(e.Path)
		if e.Project == p.name && (dir == root || strings.HasPrefix(dir, root+string(filepath.Separator))) {
			return e, nil
		}
	}
	return nil, fmt.Errorf("not inside a jw worktree; pass a name")
}

// vars are the placeholder values of one of this project's worktrees.
func (p *project) vars(e *registry.Entry) (config.Vars, error) {
	base, err := p.repo.DefaultBranch()
	if err != nil {
		return config.Vars{}, err
	}
	return p.cfg.Vars(e.Name, base, e.Slot), nil
}

// target is loadProject + resolve for the run* functions: the project and
// worktree of the cwd, or the named worktree of the cwd's project. The entry
// follows the branch the worktree is on now.
func (a *App) target(name string) (*project, *registry.Entry, error) {
	dir, err := os.Getwd()
	if err != nil {
		return nil, nil, err
	}
	p, err := loadProject(dir)
	if err != nil {
		return nil, nil, err
	}
	e, err := p.resolve(name, dir)
	if err != nil {
		return p, e, err
	}
	return p, e, a.follow(p, e)
}

// follow updates the entry when the branch checked out in its worktree is
// no longer the one registered — someone (or an agent) switched branches
// inside it. Every later check (PR, unpushed work, sync) must be about the
// branch actually there. A detached HEAD keeps the registered branch.
func (a *App) follow(p *project, e *registry.Entry) error {
	if _, err := os.Stat(e.Path); err != nil {
		return nil
	}
	current, err := git.CurrentBranch(e.Path)
	if err != nil || current == "" || current == e.Branch {
		return nil
	}
	a.warnf("%s: branch is now %s (was %s)\n", e.Name, current, e.Branch)
	if e.Original == "" && !e.Adopted {
		e.Original = e.Branch // the branch jw made: done cleans it up later
	}
	e.Branch = current
	return p.save()
}

// jwEnv is the list of JW_* variables of a worktree, as KEY=VALUE pairs.
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

// splitName takes a leading positional name off args. Go's flag package stops
// at the first non-flag argument, so `jw new web --from x` would otherwise
// leave --from unparsed.
func splitName(args []string) (string, []string) {
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		return args[0], args[1:]
	}
	return "", args
}

// sortedKeys exists because Go randomises map iteration order on purpose.
func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// realpath resolves symlinks (macOS /var → /private/var) so paths compare equal.
func realpath(p string) string {
	if r, err := filepath.EvalSymlinks(p); err == nil {
		return r
	}
	return p
}
