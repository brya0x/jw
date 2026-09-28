// Package gh reads pull request state through the GitHub CLI.
package gh

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
)

type PR struct {
	Number      int    `json:"number"`
	State       string `json:"state"` // OPEN, CLOSED or MERGED
	IsDraft     bool   `json:"isDraft"`
	URL         string `json:"url"`
	HeadRefName string `json:"headRefName"`
	HeadRefOid  string `json:"headRefOid"` // the commit GitHub has for the branch
}

const fields = "number,state,isDraft,url,headRefName,headRefOid"

// Status is the state in lower case, with "draft" for open drafts.
func (p PR) Status() string {
	if p.IsDraft && p.State == "OPEN" {
		return "draft"
	}
	return strings.ToLower(p.State)
}

// Label is the short form jw ls prints: "#12 merged", "#9 draft".
func (p PR) Label() string {
	return fmt.Sprintf("#%d %s", p.Number, p.Status())
}

func bin() (string, error) {
	if b := os.Getenv("JW_GH"); b != "" {
		return b, nil
	}
	b, err := exec.LookPath("gh")
	if err != nil {
		return "", errors.New("gh not found on PATH (set JW_GH to its path)")
	}
	return b, nil
}

// list runs `gh pr list` in dir (any directory of the repository).
func list(dir string, args ...string) ([]PR, error) {
	b, err := bin()
	if err != nil {
		return nil, err
	}
	cmd := exec.Command(b, append([]string{"pr", "list", "--state", "all", "--json", fields}, args...)...)
	cmd.Dir = dir
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("gh pr list: %s", strings.TrimSpace(stderr.String()))
	}
	var prs []PR
	if err := json.Unmarshal(out, &prs); err != nil {
		return nil, fmt.Errorf("gh pr list: %w", err)
	}
	return prs, nil
}

// ForBranch returns the newest PR whose head is branch, or nil if there is none.
func ForBranch(dir, branch string) (*PR, error) {
	prs, err := list(dir, "--head", branch, "--limit", "1")
	if err != nil || len(prs) == 0 {
		return nil, err
	}
	return &prs[0], nil
}

// ByBranch returns the newest PR of each head branch among the latest 200.
func ByBranch(dir string) (map[string]PR, error) {
	prs, err := list(dir, "--limit", "200")
	if err != nil {
		return nil, err
	}
	m := make(map[string]PR, len(prs))
	for _, p := range prs { // gh lists newest first: keep the first seen
		if _, seen := m[p.HeadRefName]; !seen {
			m[p.HeadRefName] = p
		}
	}
	return m, nil
}
