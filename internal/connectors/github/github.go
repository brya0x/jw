// Package github implements connectors.PullRequests with the gh CLI.
package github

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/brya0x/jw/internal/connectors"
)

// Client finds gh lazily, on the first call: jw works without gh until a
// command actually needs PR state.
type Client struct{}

var _ connectors.PullRequests = Client{}

// prJSON is gh's shape; callers get connectors.PR.
type prJSON struct {
	Number      int       `json:"number"`
	State       string    `json:"state"`
	IsDraft     bool      `json:"isDraft"`
	URL         string    `json:"url"`
	HeadRefName string    `json:"headRefName"`
	HeadRefOid  string    `json:"headRefOid"`
	MergedAt    time.Time `json:"mergedAt"`
}

const fields = "number,state,isDraft,url,headRefName,headRefOid,mergedAt"

func (p prJSON) conv() connectors.PR {
	return connectors.PR{
		Number: p.Number, State: p.State, IsDraft: p.IsDraft, URL: p.URL,
		Branch: p.HeadRefName, HeadSHA: p.HeadRefOid, Merged: p.MergedAt,
	}
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
func list(dir string, args ...string) ([]connectors.PR, error) {
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
	var raw []prJSON
	if err := json.Unmarshal(out, &raw); err != nil {
		return nil, fmt.Errorf("gh pr list: %w", err)
	}
	prs := make([]connectors.PR, len(raw))
	for i, p := range raw {
		prs[i] = p.conv()
	}
	return prs, nil
}

func (Client) ForBranch(dir, branch string) (*connectors.PR, error) {
	prs, err := list(dir, "--head", branch, "--limit", "1")
	if err != nil || len(prs) == 0 {
		return nil, err
	}
	return &prs[0], nil
}

// ByBranch looks at the latest 200 PRs. gh lists newest first, so a reused
// branch name keeps its latest PR.
func (Client) ByBranch(dir string) (map[string]connectors.PR, error) {
	prs, err := list(dir, "--limit", "200")
	if err != nil {
		return nil, err
	}
	m := make(map[string]connectors.PR, len(prs))
	for _, p := range prs {
		if _, seen := m[p.Branch]; !seen {
			m[p.Branch] = p
		}
	}
	return m, nil
}
