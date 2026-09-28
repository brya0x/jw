package commands

import (
	"bufio"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/brya0x/jw/internal/connectors/git"
	"github.com/brya0x/jw/internal/core/config"
)

type initOptions struct {
	repo  bool // write <repo>/.jw.toml instead of the personal config
	print bool // print the draft, write nothing
	force bool // overwrite an existing config
}

func (a *App) runInit(args []string) error {
	var o initOptions
	fs := flag.NewFlagSet("init", flag.ExitOnError)
	fs.BoolVar(&o.repo, "repo", false, "write .jw.toml in the repo (to commit) instead of your personal config")
	fs.BoolVar(&o.print, "print", false, "print the config instead of writing it")
	fs.BoolVar(&o.force, "force", false, "overwrite an existing config")
	fs.Parse(args)

	dir, err := os.Getwd()
	if err != nil {
		return err
	}
	repo, err := git.Open(dir)
	if err != nil {
		return err
	}
	return a.init(repo, o)
}

// init drafts a config from what the repository shows and writes it.
func (a *App) init(repo *git.Repo, o initOptions) error {
	project := git.ProjectName(repo.Remote)

	path, err := initPath(repo, project, o.repo)
	if err != nil {
		return err
	}
	if _, err := os.Stat(path); err == nil && !o.force && !o.print {
		return fmt.Errorf("%s already exists (--force to overwrite, --print to see the draft)", path)
	}

	draft, err := detect(repo, project, !o.repo)
	if err != nil {
		return err
	}
	text, err := draft.Render()
	if err != nil {
		return err
	}
	if o.print {
		a.printf("%s", text)
		return nil
	}

	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		return err
	}
	// Load it back the way every other command will. A draft that doesn't
	// load is a jw bug; don't leave it behind.
	if _, err := config.Load(repo.Root, repo.Remote, project); err != nil {
		_ = os.Remove(path)
		return fmt.Errorf("the generated config does not load (please report this): %w", err)
	}

	a.printf("wrote %s\n", path)
	if len(draft.Setup) > 0 {
		a.printf("  setup   %s\n", strings.Join(draft.Setup, " && "))
	}
	for _, e := range draft.Env {
		a.printf("  env     %s\n", e.File)
		for _, r := range e.Localhost {
			a.printf("            %s → localhost:%s (pick a service for it)\n", r.Key, r.Port)
		}
	}
	a.printf("next: fill in [ports] and [dev], then `jw new <name>`\n")
	return nil
}

func initPath(repo *git.Repo, project string, inRepo bool) (string, error) {
	if inRepo {
		return filepath.Join(repo.Root, ".jw.toml"), nil
	}
	dir, err := config.Dir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, project+".toml"), nil
}

// detect looks at the repository and drafts what it can: setup from the
// lockfiles, the ignored env files to copy, and their localhost ports.
func detect(repo *git.Repo, project string, personal bool) (config.Draft, error) {
	d := config.Draft{
		Project: project,
		Root:    tildify(repo.Root + "-wt"),
		Setup:   setupFor(repo.Root),
	}
	if personal {
		d.Match = repo.Remote
	}

	ignored, err := repo.IgnoredFiles()
	if err != nil {
		return d, err
	}
	for _, f := range ignored {
		if !isEnvFile(f) {
			continue
		}
		refs, err := localhostRefs(filepath.Join(repo.Root, f))
		if err != nil {
			return d, err
		}
		d.Env = append(d.Env, config.DraftEnv{File: f, Localhost: refs})
	}
	return d, nil
}

// lockfiles map to the install command that respects them. Order matters:
// the first match per ecosystem wins.
var lockfiles = []struct{ file, cmd string }{
	{"pnpm-lock.yaml", "pnpm install --frozen-lockfile"},
	{"bun.lock", "bun install --frozen-lockfile"},
	{"bun.lockb", "bun install --frozen-lockfile"},
	{"yarn.lock", "yarn install --frozen-lockfile"},
	{"package-lock.json", "npm ci"},
	{"go.mod", "go mod download"},
	{"Gemfile.lock", "bundle install"},
	{"composer.lock", "composer install"},
	{"Cargo.lock", "cargo fetch"},
}

func setupFor(root string) []string {
	var cmds []string
	node := false
	for _, l := range lockfiles {
		if _, err := os.Stat(filepath.Join(root, l.file)); err != nil {
			continue
		}
		isNode := strings.HasPrefix(l.cmd, "pnpm") || strings.HasPrefix(l.cmd, "bun") ||
			strings.HasPrefix(l.cmd, "yarn") || strings.HasPrefix(l.cmd, "npm")
		if isNode && node {
			continue // one JS package manager is enough
		}
		node = node || isNode
		cmds = append(cmds, l.cmd)
	}
	return cmds
}

// isEnvFile matches .env, .env.local, .env.development… but not templates,
// which are meant to be committed and copied by hand.
func isEnvFile(path string) bool {
	base := filepath.Base(path)
	if base != ".env" && !strings.HasPrefix(base, ".env.") {
		return false
	}
	for _, t := range []string{".example", ".sample", ".template", ".dist"} {
		if strings.HasSuffix(base, t) {
			return false
		}
	}
	return true
}

var envLocalhost = regexp.MustCompile(`^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*(?:localhost|127\.0\.0\.1):(\d+).*)$`)
var hostPort = regexp.MustCompile(`(localhost|127\.0\.0\.1):\d+`)

// localhostRefs finds KEY=value lines whose value points at a local port.
func localhostRefs(path string) ([]config.LocalhostRef, error) {
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var refs []config.LocalhostRef
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		m := envLocalhost.FindStringSubmatch(sc.Text())
		if m == nil {
			continue
		}
		value := strings.Trim(m[2], `"'`)
		refs = append(refs, config.LocalhostRef{
			Key:   m[1],
			Value: hostPort.ReplaceAllString(value, "$1:{port.SERVICE}"),
			Port:  m[3],
		})
	}
	return refs, sc.Err()
}

// tildify shortens a path under the home directory to ~/…, so the config
// reads well and survives a different username on another machine.
func tildify(p string) string {
	home, err := os.UserHomeDir()
	if err != nil {
		return p
	}
	if rel, err := filepath.Rel(home, p); err == nil && !strings.HasPrefix(rel, "..") {
		return filepath.ToSlash(filepath.Join("~", rel))
	}
	return p
}
