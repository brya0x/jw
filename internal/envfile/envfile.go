// Package envfile edits dotenv files line by line, keeping comments, order
// and every key it was not asked to touch.
package envfile

import (
	"regexp"
	"sort"
	"strings"
)

// A line that assigns a key: optional `export`, the key, then `=`.
var assign = regexp.MustCompile(`^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=`)

// Set overwrites the value of each key in set, keeping any `export` prefix,
// and appends keys that were missing (sorted, so output is deterministic).
func Set(data []byte, set map[string]string) []byte {
	if len(set) == 0 {
		return data
	}

	text := string(data)
	lines := strings.Split(strings.TrimSuffix(text, "\n"), "\n")
	if text == "" {
		lines = nil
	}

	done := map[string]bool{}
	for i, line := range lines {
		m := assign.FindStringSubmatchIndex(line)
		if m == nil {
			continue
		}
		key := line[m[2]:m[3]]
		if v, ok := set[key]; ok {
			// Keep everything up to and including `=`, replace the value.
			lines[i] = line[:m[1]] + v
			done[key] = true
		}
	}

	var missing []string
	for k := range set {
		if !done[k] {
			missing = append(missing, k)
		}
	}
	sort.Strings(missing)
	for _, k := range missing {
		lines = append(lines, k+"="+set[k])
	}

	return []byte(strings.Join(lines, "\n") + "\n")
}
