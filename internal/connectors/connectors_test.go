package connectors

import "testing"

func TestPRLabel(t *testing.T) {
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
