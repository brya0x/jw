// Package guide holds the instructions that teach coding agents to use jw.
// The text is compiled into the binary, so it always matches the jw that
// installs it.
package guide

import (
	_ "embed"
	"strings"
)

//go:embed guide.md
var text string

// Markdown is the guide itself.
func Markdown() string { return text }

// Skill is the guide as a Claude Code skill (SKILL.md): frontmatter that
// tells Claude when to load it, then the guide.
func Skill() string {
	return `---
name: jw
description: Use when working inside a jw stream (a git worktree with its own herdr tab and ports, JW_* variables in the environment) or when creating, handing work to, syncing or finishing streams with the jw CLI. Covers jw info, jw dev, jw sync, jw new --task, jw prompt, --json output and what exit code 3 means.
---

` + text
}

// Block delimiters for a guide embedded in someone else's AGENTS.md.
const (
	begin = "<!-- jw:begin — managed by `jw agents install`; edits inside are overwritten -->"
	end   = "<!-- jw:end -->"
)

// Embed puts the guide into doc between jw's markers: replacing the block
// if it's there, appending it otherwise. The rest of doc is left untouched.
func Embed(doc string) string {
	block := begin + "\n" + strings.TrimSpace(text) + "\n" + end + "\n"
	if i := strings.Index(doc, begin); i >= 0 {
		if j := strings.Index(doc[i:], end); j >= 0 {
			after := strings.TrimPrefix(doc[i+j+len(end):], "\n")
			return doc[:i] + block + after
		}
	}
	if doc != "" && !strings.HasSuffix(doc, "\n") {
		doc += "\n"
	}
	if doc != "" {
		doc += "\n"
	}
	return doc + block
}
