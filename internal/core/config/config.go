// Package config loads the per-project jw config: where worktrees go, how to
// set them up, which ports each service gets and which env files to copy.
package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/BurntSushi/toml"
)

// Every slot owns a block of ports: PortBlockStart + slot*PortBlockSize + offset.
const (
	PortBlockStart = 20000
	PortBlockSize  = 100
)

type Config struct {
	Match     string              `toml:"match"`
	Root      string              `toml:"root"`
	Workspace string              `toml:"workspace"`
	Branch    string              `toml:"branch"`
	Setup     []string            `toml:"setup"`
	Ports     map[string]int      `toml:"ports"`
	Dev       map[string][]string `toml:"dev"`
	Env       []EnvFile           `toml:"env"`
	Agent     Agent               `toml:"agent"`
	Layout    Layout              `toml:"layout"`

	// Source is the file this config came from, empty for defaults.
	Source string `toml:"-"`
}

// EnvFile is copied from the main checkout into each worktree. Keys in Set
// are overwritten (or appended) after placeholders are expanded.
type EnvFile struct {
	File string            `toml:"file"`
	Set  map[string]string `toml:"set"`
}

type Agent struct {
	Default string   `toml:"default"`
	Claude  AgentCmd `toml:"claude"`
	Codex   AgentCmd `toml:"codex"`
}

type AgentCmd struct {
	Start  string `toml:"start"`
	Resume string `toml:"resume"`
}

type Layout struct {
	Editor string `toml:"editor"`
}

// Dir returns the config directory, honouring XDG_CONFIG_HOME.
func Dir() (string, error) {
	if dir := os.Getenv("XDG_CONFIG_HOME"); dir != "" {
		return filepath.Join(dir, "jw"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".config", "jw"), nil
}

// Load finds the config for a repository, in order:
//
//  1. <repoRoot>/.jw.toml
//  2. a file in Dir() whose `match` equals the origin URL
//  3. Dir()/<project>.toml without a `match`
//  4. defaults
func Load(repoRoot, remote, project string) (*Config, error) {
	if c, err := decodeFile(filepath.Join(repoRoot, ".jw.toml")); err == nil {
		return c.withDefaults(repoRoot, project)
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}

	dir, err := Dir()
	if err != nil {
		return nil, err
	}
	c, err := findPersonal(dir, remote, project)
	if err != nil {
		return nil, err
	}
	if c == nil {
		c = &Config{}
	}
	return c.withDefaults(repoRoot, project)
}

func findPersonal(dir, remote, project string) (*Config, error) {
	files, err := filepath.Glob(filepath.Join(dir, "*.toml"))
	if err != nil {
		return nil, err
	}
	sort.Strings(files)

	var byMatch, byName *Config
	for _, f := range files {
		c, err := decodeFile(f)
		if err != nil {
			return nil, err
		}
		switch {
		case c.Match != "" && sameRepo(c.Match, remote):
			if byMatch != nil {
				return nil, fmt.Errorf("both %s and %s match %s", byMatch.Source, c.Source, remote)
			}
			byMatch = c
		case c.Match == "" && strings.TrimSuffix(filepath.Base(f), ".toml") == project:
			byName = c
		}
	}
	if byMatch != nil {
		return byMatch, nil
	}
	return byName, nil
}

// decodeFile parses one config file and rejects keys it does not know, so a
// typo like `setpu` fails loudly instead of being silently ignored.
func decodeFile(path string) (*Config, error) {
	var c Config
	md, err := toml.DecodeFile(path, &c)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, err
		}
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	if extra := md.Undecoded(); len(extra) > 0 {
		keys := make([]string, len(extra))
		for i, k := range extra {
			keys[i] = k.String()
		}
		return nil, fmt.Errorf("%s: unknown keys: %s", path, strings.Join(keys, ", "))
	}
	c.Source = path
	return &c, nil
}

func (c *Config) withDefaults(repoRoot, project string) (*Config, error) {
	if c.Root == "" {
		c.Root = repoRoot + "-wt"
	}
	c.Root = expandHome(c.Root)
	if !filepath.IsAbs(c.Root) {
		c.Root = filepath.Join(repoRoot, c.Root)
	}
	if c.Workspace == "" {
		c.Workspace = project
	}
	if c.Branch == "" {
		c.Branch = "feat/{name}"
	}
	if c.Agent.Default == "" {
		c.Agent.Default = "claude"
	}
	if c.Agent.Claude == (AgentCmd{}) {
		c.Agent.Claude = AgentCmd{Start: "claude", Resume: "claude --continue"}
	}
	if c.Agent.Codex == (AgentCmd{}) {
		c.Agent.Codex = AgentCmd{Start: "codex", Resume: "codex resume --last"}
	}
	if c.Layout.Editor == "" {
		c.Layout.Editor = "nvim -c 'DiffviewOpen origin/{base}...HEAD'"
	}
	return c, c.validate()
}

// validate catches bad offsets and unknown placeholders at load time, not
// halfway through creating a worktree.
func (c *Config) validate() error {
	for svc, off := range c.Ports {
		if off < 0 || off >= PortBlockSize {
			return fmt.Errorf("%s: port offset %s = %d, must be 0–%d", c.Source, svc, off, PortBlockSize-1)
		}
	}

	probe := c.Vars("name", "main", 1)
	check := func(where, s string) error {
		if _, err := Expand(s, probe); err != nil {
			return fmt.Errorf("%s: %s: %w", c.Source, where, err)
		}
		return nil
	}

	if err := check("branch", c.Branch); err != nil {
		return err
	}
	if err := check("layout.editor", c.Layout.Editor); err != nil {
		return err
	}
	for i, s := range c.Setup {
		if err := check(fmt.Sprintf("setup[%d]", i), s); err != nil {
			return err
		}
	}
	for svc, cmds := range c.Dev {
		for _, s := range cmds {
			if err := check("dev."+svc, s); err != nil {
				return err
			}
		}
	}
	for _, e := range c.Env {
		if e.File == "" {
			return fmt.Errorf("%s: [[env]] entry without file", c.Source)
		}
		for k, v := range e.Set {
			if err := check(e.File+" "+k, v); err != nil {
				return err
			}
		}
	}
	return nil
}

// PortsFor returns the concrete port of every service for a slot.
func (c *Config) PortsFor(slot int) map[string]int {
	ports := make(map[string]int, len(c.Ports))
	for svc, off := range c.Ports {
		ports[svc] = PortBase(slot) + off
	}
	return ports
}

// Vars builds the placeholder values for one worktree.
func (c *Config) Vars(name, base string, slot int) Vars {
	return Vars{Name: name, Base: base, Slot: slot, Ports: c.PortsFor(slot)}
}

func PortBase(slot int) int {
	return PortBlockStart + slot*PortBlockSize
}

// sameRepo compares two remote URLs ignoring scheme, user, .git and ssh vs https.
func sameRepo(a, b string) bool {
	return normalizeRemote(a) == normalizeRemote(b)
}

func normalizeRemote(u string) string {
	u = strings.ToLower(strings.TrimSpace(u))
	for _, p := range []string{"https://", "http://", "ssh://", "git://"} {
		u = strings.TrimPrefix(u, p)
	}
	if at := strings.Index(u, "@"); at >= 0 {
		u = u[at+1:]
	}
	u = strings.Replace(u, ":", "/", 1)
	u = strings.TrimSuffix(strings.TrimRight(u, "/"), ".git")
	return u
}

func expandHome(p string) string {
	if p == "~" || strings.HasPrefix(p, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, strings.TrimPrefix(p, "~"))
		}
	}
	return p
}
