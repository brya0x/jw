package version

import (
	"runtime/debug"
	"testing"
)

func TestFrom(t *testing.T) {
	cases := []struct {
		name string
		bi   debug.BuildInfo
		want string
	}{
		{
			name: "go install module@main: only the pseudo-version",
			bi:   debug.BuildInfo{Main: debug.Module{Version: "v0.0.0-20260928204820-c1f6fdb4eac8"}},
			want: "jw v0.0.0-20260928204820-c1f6fdb4eac8 (commit c1f6fdb, 2026-09-28)",
		},
		{
			name: "go build in the repo, with local changes",
			bi: debug.BuildInfo{
				Main: debug.Module{Version: "v0.0.0-20260928221755-6565cbd0e9c1+dirty"},
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: "6565cbd0e9c10492a6db42971dbd454216f5ea1a"},
					{Key: "vcs.time", Value: "2026-09-28T22:17:55Z"},
					{Key: "vcs.modified", Value: "true"},
				},
			},
			want: "jw v0.0.0-20260928221755-6565cbd0e9c1 (commit 6565cbd, 2026-09-28, built with uncommitted changes)",
		},
		{
			name: "a tagged release",
			bi:   debug.BuildInfo{Main: debug.Module{Version: "v0.3.0"}},
			want: "jw v0.3.0",
		},
		{
			name: "no version at all",
			bi:   debug.BuildInfo{},
			want: "jw (devel)",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := From(&tc.bi).String(); got != tc.want {
				t.Fatalf("got  %q\nwant %q", got, tc.want)
			}
		})
	}
}
