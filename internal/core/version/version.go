// Package version describes the running jw binary from the build info Go
// embeds in it, so there is nothing to bump by hand.
package version

import (
	"fmt"
	"regexp"
	"runtime/debug"
	"strings"
	"time"
)

// Info is what jw version prints.
type Info struct {
	Version  string // module version: a tag, a pseudo-version, or "(devel)"
	Commit   string // short commit, "" if unknown
	Date     string // commit date, YYYY-MM-DD, "" if unknown
	Modified bool   // built from a tree with uncommitted changes
}

// Current reads the running binary's build info.
func Current() Info {
	bi, ok := debug.ReadBuildInfo()
	if !ok {
		return Info{Version: "(unknown)"}
	}
	return From(bi)
}

// pseudo matches a Go pseudo-version: vX.Y.Z-[pre.]yyyymmddhhmmss-abcdefabcdef.
var pseudo = regexp.MustCompile(`(\d{14})-([0-9a-f]{12})`)

// From describes a build. `go build` inside the repo records vcs.* settings;
// `go install module@version` records only the module version, which for an
// untagged commit is a pseudo-version carrying the date and commit.
func From(bi *debug.BuildInfo) Info {
	info := Info{Version: strings.TrimSuffix(bi.Main.Version, "+dirty")}
	if strings.HasSuffix(bi.Main.Version, "+dirty") {
		info.Modified = true
	}

	for _, s := range bi.Settings {
		switch s.Key {
		case "vcs.revision":
			info.Commit = short(s.Value)
		case "vcs.time":
			if t, err := time.Parse(time.RFC3339, s.Value); err == nil {
				info.Date = t.Format("2006-01-02")
			}
		case "vcs.modified":
			info.Modified = info.Modified || s.Value == "true"
		}
	}

	if info.Commit == "" {
		if m := pseudo.FindStringSubmatch(info.Version); m != nil {
			info.Commit = short(m[2])
			if t, err := time.Parse("20060102150405", m[1]); err == nil {
				info.Date = t.Format("2006-01-02")
			}
		}
	}
	if info.Version == "" {
		info.Version = "(devel)"
	}
	return info
}

func (i Info) String() string {
	s := "jw " + i.Version
	var details []string
	if i.Commit != "" {
		details = append(details, "commit "+i.Commit)
	}
	if i.Date != "" {
		details = append(details, i.Date)
	}
	if i.Modified {
		details = append(details, "built with uncommitted changes")
	}
	if len(details) > 0 {
		s += fmt.Sprintf(" (%s)", strings.Join(details, ", "))
	}
	return s
}

func short(rev string) string {
	if len(rev) > 7 {
		return rev[:7]
	}
	return rev
}
