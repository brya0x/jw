package guide

import (
	"strings"
	"testing"
)

func TestSkillHasFrontmatter(t *testing.T) {
	s := Skill()
	if !strings.HasPrefix(s, "---\nname: jw\ndescription: ") || !strings.Contains(s, "\n---\n\n# Working inside a jw stream") {
		t.Fatalf("bad skill header:\n%s", s[:200])
	}
}

func TestEmbedAppendsThenReplacesInPlace(t *testing.T) {
	doc := "# Our rules\n\nUse pnpm."
	once := Embed(doc)
	if !strings.HasPrefix(once, "# Our rules\n\nUse pnpm.\n\n<!-- jw:begin") {
		t.Fatalf("append:\n%s", once)
	}

	// Stale content inside the block is replaced; what follows is kept.
	stale := strings.Replace(once, "Working inside a jw stream", "OLD TEXT", 1) + "\n## After jw\n"
	twice := Embed(stale)
	if strings.Contains(twice, "OLD TEXT") {
		t.Fatal("the block was not replaced")
	}
	if strings.Count(twice, "<!-- jw:begin") != 1 {
		t.Fatal("the block was duplicated")
	}
	if !strings.HasSuffix(twice, "## After jw\n") || !strings.HasPrefix(twice, "# Our rules") {
		t.Fatalf("content around the block was not kept:\n%s", twice)
	}
	if Embed(twice) != twice {
		t.Fatal("embedding is not idempotent")
	}
}

func TestEmbedIntoEmptyFile(t *testing.T) {
	if got := Embed(""); !strings.HasPrefix(got, "<!-- jw:begin") {
		t.Fatalf("got %q", got[:40])
	}
}
