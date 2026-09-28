// Package git wraps the git commands jw needs. It shells out to the git
// binary instead of using a library, so behaviour matches what you get typing
// the same command yourself.
package git

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// Repo is the main checkout of a repository, even when opened from inside one
// of its worktrees.
type Repo struct {
	Root      string // main checkout directory
	CommonDir string // the shared .git directory
	Remote    string // URL of origin
}

// run executes git in dir and returns trimmed stdout. On failure the error
// carries git's own stderr, which is usually the most useful message.
func run(dir string, args ...string) (string, error) {
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	var stderr bytes.Buffer
	cmd.Stderr = &stderr

	out, err := cmd.Output()
	if err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg == "" {
			msg = err.Error()
		}
		return "", fmt.Errorf("git %s: %s", strings.Join(args, " "), msg)
	}
	return strings.TrimSpace(string(out)), nil
}

// Open finds the repository that contains dir.
func Open(dir string) (*Repo, error) {
	common, err := run(dir, "rev-parse", "--path-format=absolute", "--git-common-dir")
	if err != nil {
		return nil, fmt.Errorf("%s is not inside a git repository", dir)
	}
	root := filepath.Dir(common)

	remote, err := run(root, "remote", "get-url", "origin")
	if err != nil {
		return nil, fmt.Errorf("%s has no origin remote", root)
	}
	return &Repo{Root: root, CommonDir: common, Remote: remote}, nil
}

// DefaultBranch reads origin/HEAD. It never assumes main: if the ref is
// missing locally it asks the remote once, and fails if that doesn't work.
func (r *Repo) DefaultBranch() (string, error) {
	ref, err := run(r.Root, "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
	if err != nil {
		if _, err := run(r.Root, "remote", "set-head", "origin", "--auto"); err != nil {
			return "", fmt.Errorf("cannot resolve the default branch of origin: %w", err)
		}
		if ref, err = run(r.Root, "symbolic-ref", "--short", "refs/remotes/origin/HEAD"); err != nil {
			return "", err
		}
	}
	return strings.TrimPrefix(ref, "origin/"), nil
}

// Fetch updates the remote-tracking refs from origin.
func (r *Repo) Fetch() error {
	_, err := run(r.Root, "fetch", "--quiet", "origin")
	return err
}

// BranchExists reports whether a local branch with that name exists.
func (r *Repo) BranchExists(branch string) bool {
	_, err := run(r.Root, "rev-parse", "--verify", "--quiet", "refs/heads/"+branch)
	return err == nil
}

// AddWorktree creates path with a new branch starting at ref.
func (r *Repo) AddWorktree(path, branch, ref string) error {
	_, err := run(r.Root, "worktree", "add", "--quiet", "-b", branch, path, ref)
	return err
}

// RemoveWorktree deletes the worktree at path, discarding local changes.
func (r *Repo) RemoveWorktree(path string) error {
	_, err := run(r.Root, "worktree", "remove", "--force", path)
	return err
}

// DeleteBranch force-deletes a local branch.
func (r *Repo) DeleteBranch(branch string) error {
	_, err := run(r.Root, "branch", "-D", branch)
	return err
}

// IgnoredFiles lists the files git ignores in the main checkout, as paths
// relative to it. An ignored directory (node_modules/, a nested worktree)
// counts as one entry and is left out, so the list stays short.
func (r *Repo) IgnoredFiles() ([]string, error) {
	out, err := run(r.Root, "ls-files", "--others", "--ignored", "--exclude-standard", "--directory")
	if err != nil || out == "" {
		return nil, err
	}
	var files []string
	for _, line := range strings.Split(out, "\n") {
		if !strings.HasSuffix(line, "/") {
			files = append(files, line)
		}
	}
	return files, nil
}

// Dirty reports whether the worktree at dir has uncommitted or untracked files.
func Dirty(dir string) (bool, error) {
	out, err := run(dir, "status", "--porcelain")
	return out != "", err
}

// Head returns the commit checked out in dir.
func Head(dir string) (string, error) {
	return run(dir, "rev-parse", "HEAD")
}

// HasCommit reports whether the object database has commit oid.
func (r *Repo) HasCommit(oid string) bool {
	_, err := run(r.Root, "cat-file", "-e", oid+"^{commit}")
	return err == nil
}

// FetchCommit downloads one commit by sha (GitHub allows this).
func (r *Repo) FetchCommit(oid string) error {
	_, err := run(r.Root, "fetch", "--quiet", "origin", oid)
	return err
}

// IsAncestor reports whether commit a is contained in commit b's history.
func (r *Repo) IsAncestor(a, b string) bool {
	_, err := run(r.Root, "merge-base", "--is-ancestor", a, b)
	return err == nil
}

// Exclude adds pattern to .git/info/exclude unless it is already there. That
// file is shared by every worktree and never committed.
func (r *Repo) Exclude(pattern string) error {
	path := filepath.Join(r.CommonDir, "info", "exclude")
	data, err := os.ReadFile(path)
	if err != nil && !os.IsNotExist(err) {
		return err
	}
	for _, line := range strings.Split(string(data), "\n") {
		if strings.TrimSpace(line) == pattern {
			return nil
		}
	}

	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	defer f.Close()

	if len(data) > 0 && !bytes.HasSuffix(data, []byte("\n")) {
		pattern = "\n" + pattern
	}
	_, err = f.WriteString(pattern + "\n")
	return err
}

// ProjectName derives a short project name from a remote URL:
// git@github.com:acme/myapp.git and https://github.com/acme/myapp both give "myapp".
func ProjectName(remote string) string {
	name := strings.TrimSuffix(strings.TrimRight(remote, "/"), ".git")
	if i := strings.LastIndexAny(name, "/:"); i >= 0 {
		name = name[i+1:]
	}
	return name
}
