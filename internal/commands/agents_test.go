package commands

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/brya0x/jw/internal/core/guide"
)

func TestAgentsInstallSkill(t *testing.T) {
	env := newTestEnv(t)
	dir := t.TempDir()
	t.Setenv("CLAUDE_CONFIG_DIR", dir)
	skill := filepath.Join(dir, "skills", "jw", "SKILL.md")

	if code := env.app.Run([]string{"agents", "install"}); code != ExitOK {
		t.Fatalf("exit %d\n%s", code, env.out)
	}
	if got, _ := os.ReadFile(skill); string(got) != guide.Skill() {
		t.Fatal("skill not written")
	}

	// A stale copy jw wrote is updated without --force.
	os.WriteFile(skill, []byte("---\nname: jw\ndescription: old\n---\nold"), 0o644)
	if code := env.app.Run([]string{"agents", "install"}); code != ExitOK {
		t.Fatal("updating jw's own skill must not need --force")
	}

	// Someone else's skill named jw is not clobbered.
	os.WriteFile(skill, []byte("my own notes"), 0o644)
	if code := env.app.Run([]string{"agents", "install"}); code != ExitError {
		t.Fatal("must refuse to overwrite a skill jw didn't write")
	}
	if code := env.app.Run([]string{"agents", "install", "--force"}); code != ExitOK {
		t.Fatal("--force replaces it")
	}
}

func TestAgentsInstallIntoAgentsMD(t *testing.T) {
	env := newTestEnv(t)
	path := filepath.Join(t.TempDir(), "AGENTS.md")
	os.WriteFile(path, []byte("# Rules\n\nUse pnpm.\n"), 0o644)

	for range 2 {
		if code := env.app.Run([]string{"agents", "install", "--into", path}); code != ExitOK {
			t.Fatalf("exit %d\n%s", code, env.out)
		}
	}
	got, _ := os.ReadFile(path)
	if !strings.HasPrefix(string(got), "# Rules\n\nUse pnpm.\n") || strings.Count(string(got), "jw:begin") != 1 {
		t.Fatalf("AGENTS.md:\n%s", got)
	}
	if !strings.Contains(env.out.String(), "up to date") {
		t.Error("the second run should be a no-op")
	}
}

func TestAgentsPrintsTheGuide(t *testing.T) {
	env := newTestEnv(t)
	env.app.Run([]string{"agents"})
	if !strings.Contains(env.out.String(), "Exit code 3 means a person has to decide") {
		t.Fatalf("guide not printed:\n%.200s", env.out)
	}
}
