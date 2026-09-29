package config

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

// Vars are the values placeholders expand to.
type Vars struct {
	Name  string         // {name}
	Base  string         // {base}: the default branch
	Slot  int            // {slot}
	Ports map[string]int // {port.<service>}
}

var placeholder = regexp.MustCompile(`\{([a-z0-9_.-]+)\}`)

// Expand replaces {name}, {base}, {slot} and {port.<service>} in s. An unknown
// placeholder is an error rather than being left in place.
func Expand(s string, v Vars) (string, error) {
	var bad []string
	out := placeholder.ReplaceAllStringFunc(s, func(m string) string {
		key := m[1 : len(m)-1]
		switch {
		case key == "name":
			return v.Name
		case key == "base":
			return v.Base
		case key == "slot":
			return strconv.Itoa(v.Slot)
		case strings.HasPrefix(key, "port."):
			if p, ok := v.Ports[strings.TrimPrefix(key, "port.")]; ok {
				return strconv.Itoa(p)
			}
		}
		bad = append(bad, m)
		return m
	})
	if len(bad) > 0 {
		return "", fmt.Errorf("unknown placeholder %s", strings.Join(bad, ", "))
	}
	return out, nil
}

// PortsIn lists the services whose port s refers to, as {port.<service>},
// in order of first use.
func PortsIn(s string) []string {
	var out []string
	seen := map[string]bool{}
	for _, m := range placeholder.FindAllStringSubmatch(s, -1) {
		if svc, ok := strings.CutPrefix(m[1], "port."); ok && !seen[svc] {
			seen[svc] = true
			out = append(out, svc)
		}
	}
	return out
}

// EnvName turns a service name into its variable: console-web → JW_PORT_CONSOLE_WEB.
func EnvName(service string) string {
	return "JW_PORT_" + strings.ToUpper(strings.NewReplacer("-", "_", ".", "_").Replace(service))
}
