package envfile

import "testing"

func TestSet(t *testing.T) {
	cases := []struct {
		name string
		in   string
		set  map[string]string
		want string
	}{
		{
			name: "replaces value, keeps comments and other keys",
			in:   "# api\nAPI_URL=http://localhost:8787\nTOKEN=abc\n",
			set:  map[string]string{"API_URL": "http://localhost:20302"},
			want: "# api\nAPI_URL=http://localhost:20302\nTOKEN=abc\n",
		},
		{
			name: "keeps export prefix",
			in:   "export PORT=3000\n",
			set:  map[string]string{"PORT": "20100"},
			want: "export PORT=20100\n",
		},
		{
			name: "appends missing keys sorted",
			in:   "A=1",
			set:  map[string]string{"C": "3", "B": "2"},
			want: "A=1\nB=2\nC=3\n",
		},
		{
			name: "empty file",
			in:   "",
			set:  map[string]string{"PORT": "20100"},
			want: "PORT=20100\n",
		},
		{
			name: "does not touch a key that only starts the same",
			in:   "API_URL_V2=x\n",
			set:  map[string]string{"API_URL": "y"},
			want: "API_URL_V2=x\nAPI_URL=y\n",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := string(Set([]byte(tc.in), tc.set)); got != tc.want {
				t.Fatalf("got:\n%q\nwant:\n%q", got, tc.want)
			}
		})
	}
}
