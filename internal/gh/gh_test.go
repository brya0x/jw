package gh

import (
	"os"
	"path/filepath"
	"testing"
)

func fakeGh(t *testing.T, reply string) {
	t.Helper()
	bin := filepath.Join(t.TempDir(), "gh")
	script := "#!/bin/sh\ncat <<'EOF'\n" + reply + "\nEOF\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("JW_GH", bin)
}

func TestForBranch(t *testing.T) {
	fakeGh(t, `[{"number":4,"state":"MERGED","headRefName":"feat/web","headRefOid":"abc"}]`)
	pr, err := ForBranch(t.TempDir(), "feat/web")
	if err != nil {
		t.Fatal(err)
	}
	if pr == nil || pr.Number != 4 || pr.HeadRefOid != "abc" {
		t.Fatalf("got %+v", pr)
	}
}

func TestForBranchNone(t *testing.T) {
	fakeGh(t, `[]`)
	pr, err := ForBranch(t.TempDir(), "feat/web")
	if err != nil || pr != nil {
		t.Fatalf("want nil, nil — got %+v, %v", pr, err)
	}
}

func TestByBranchKeepsNewest(t *testing.T) {
	// gh lists newest first; a reused branch name keeps its latest PR.
	fakeGh(t, `[{"number":9,"state":"OPEN","headRefName":"feat/web"},{"number":3,"state":"CLOSED","headRefName":"feat/web"}]`)
	prs, err := ByBranch(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if prs["feat/web"].Number != 9 {
		t.Fatalf("got %+v", prs["feat/web"])
	}
}

func TestLabel(t *testing.T) {
	cases := map[string]PR{
		"#1 merged": {Number: 1, State: "MERGED"},
		"#2 draft":  {Number: 2, State: "OPEN", IsDraft: true},
		"#3 open":   {Number: 3, State: "OPEN"},
	}
	for want, pr := range cases {
		if got := pr.Label(); got != want {
			t.Errorf("got %q want %q", got, want)
		}
	}
}
