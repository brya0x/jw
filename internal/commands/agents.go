package commands

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/brya0x/jw/internal/core/guide"
)

// runAgents is `jw agents`: print the guide that teaches coding agents to use
// jw, or install it (`jw agents install`).
func (a *App) runAgents(args []string) error {
	if len(args) == 0 || args[0] != "install" {
		if len(args) > 0 {
			return fmt.Errorf("usage: jw agents            print the guide\n       jw agents install    install it for Claude Code, or --into an AGENTS.md")
		}
		a.printf("%s", guide.Markdown())
		return nil
	}

	fs := flag.NewFlagSet("agents install", flag.ExitOnError)
	into := fs.String("into", "", "embed the guide in this AGENTS.md (Codex, or a repo's) instead of installing a Claude Code skill")
	force := fs.Bool("force", false, "overwrite a skill named jw that jw didn't write")
	fs.Parse(args[1:])

	if *into != "" {
		return a.embedGuide(*into)
	}
	return a.installSkill(*force)
}

// installSkill writes the Claude Code skill. A SKILL.md jw wrote before is
// updated in place; anything else there needs --force.
func (a *App) installSkill(force bool) error {
	dir, err := claudeDir()
	if err != nil {
		return err
	}
	path := filepath.Join(dir, "skills", "jw", "SKILL.md")

	if old, err := os.ReadFile(path); err == nil {
		switch {
		case string(old) == guide.Skill():
			a.printf("%s is up to date\n", path)
			return nil
		case !strings.HasPrefix(string(old), "---\nname: jw\n") && !force:
			return fmt.Errorf("%s exists and wasn't written by jw (--force to replace it)", path)
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		return err
	}

	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(path, []byte(guide.Skill()), 0o644); err != nil {
		return err
	}
	a.printf("installed the Claude Code skill: %s\n", path)
	a.printf("claude loads it when a task involves a jw stream. For Codex: jw agents install --into ~/.codex/AGENTS.md (or a repo's AGENTS.md)\n")
	return nil
}

// embedGuide puts the guide between jw's markers in an AGENTS.md, creating
// the file if needed and replacing an older copy of the block.
func (a *App) embedGuide(path string) error {
	path = expandTilde(path)
	old, err := os.ReadFile(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	updated := guide.Embed(string(old))
	if updated == string(old) {
		a.printf("%s is up to date\n", path)
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(path, []byte(updated), 0o644); err != nil {
		return err
	}
	a.printf("wrote the jw guide into %s (between the jw:begin/jw:end markers)\n", path)
	return nil
}

// claudeDir is Claude Code's config directory: $CLAUDE_CONFIG_DIR, else ~/.claude.
func claudeDir() (string, error) {
	if dir := os.Getenv("CLAUDE_CONFIG_DIR"); dir != "" {
		return dir, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".claude"), nil
}

func expandTilde(p string) string {
	if p == "~" || strings.HasPrefix(p, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, strings.TrimPrefix(p, "~"))
		}
	}
	return p
}
