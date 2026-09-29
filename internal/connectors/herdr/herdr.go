// Package herdr implements connectors.Multiplexer with the herdr CLI. Every
// call parses herdr's JSON reply and takes IDs from it; nothing is guessed
// from what is focused.
package herdr

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"time"

	"github.com/brya0x/jw/internal/connectors"
)

type Client struct {
	bin string
}

var _ connectors.Multiplexer = (*Client)(nil)

// New finds the herdr binary: $JW_HERDR if set, else herdr on PATH.
func New() (*Client, error) {
	if bin := os.Getenv("JW_HERDR"); bin != "" {
		return &Client{bin: bin}, nil
	}
	bin, err := exec.LookPath("herdr")
	if err != nil {
		return nil, errors.New("herdr not found on PATH (set JW_HERDR to its path)")
	}
	return &Client{bin: bin}, nil
}

// herdr's JSON shapes. They stay private: callers get connectors types.
type (
	workspaceJSON struct {
		ID    string `json:"workspace_id"`
		Label string `json:"label"`
	}
	tabJSON struct {
		ID          string `json:"tab_id"`
		Label       string `json:"label"`
		WorkspaceID string `json:"workspace_id"`
	}
	paneJSON struct {
		ID    string `json:"pane_id"`
		TabID string `json:"tab_id"`
		Label string `json:"label"`
	}
	processJSON struct {
		Name    string `json:"name"`
		Cmdline string `json:"cmdline"`
	}
)

func (w workspaceJSON) conv() connectors.Workspace { return connectors.Workspace(w) }
func (t tabJSON) conv() connectors.Tab             { return connectors.Tab(t) }
func (p paneJSON) conv() connectors.Pane           { return connectors.Pane(p) }

// Error is herdr's own error reply, e.g. {"code":"tab_not_found", ...}.
type Error struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

func (e *Error) Error() string { return "herdr: " + e.Message }

// Is maps herdr's codes onto the connectors errors, so callers write
// errors.Is(err, connectors.ErrNotFound) without knowing herdr's codes.
func (e *Error) Is(target error) bool {
	switch target {
	case connectors.ErrNotFound:
		return strings.HasSuffix(e.Code, "_not_found")
	case connectors.ErrAgentNotReady:
		return e.Code == "agent_not_ready"
	case connectors.ErrAgentBlocked:
		return e.Code == "agent_blocked"
	}
	return false
}

// call runs herdr with args and decodes the reply's "result" into out.
func (c *Client) call(out any, args ...string) error {
	cmd := exec.Command(c.bin, args...)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdout, runErr := cmd.Output()

	// Some commands (pane run) print nothing at all when they succeed.
	if runErr == nil && len(bytes.TrimSpace(stdout)) == 0 {
		return nil
	}

	// Successful replies come on stdout; error replies on stderr.
	body := stdout
	if len(bytes.TrimSpace(body)) == 0 {
		body = stderr.Bytes()
	}

	var reply struct {
		Result json.RawMessage `json:"result"`
		Error  *Error          `json:"error"`
	}
	if err := json.Unmarshal(body, &reply); err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg == "" && runErr != nil {
			msg = runErr.Error()
		}
		if msg == "" {
			msg = "unexpected output: " + err.Error()
		}
		return fmt.Errorf("herdr %s: %s", strings.Join(args[:min(2, len(args))], " "), msg)
	}
	if reply.Error != nil {
		return reply.Error
	}
	if out == nil {
		return nil
	}
	return json.Unmarshal(reply.Result, out)
}

func envFlags(env []string) []string {
	var flags []string
	for _, kv := range env {
		flags = append(flags, "--env", kv)
	}
	return flags
}

func (c *Client) Workspaces() ([]connectors.Workspace, error) {
	var r struct {
		Workspaces []workspaceJSON `json:"workspaces"`
	}
	if err := c.call(&r, "workspace", "list"); err != nil {
		return nil, err
	}
	out := make([]connectors.Workspace, len(r.Workspaces))
	for i, w := range r.Workspaces {
		out[i] = w.conv()
	}
	return out, nil
}

// CreateWorkspace makes a workspace; herdr gives it one tab with one pane.
func (c *Client) CreateWorkspace(cwd, label string, env []string) (connectors.Workspace, connectors.Tab, connectors.Pane, error) {
	var r struct {
		Workspace workspaceJSON `json:"workspace"`
		Tab       tabJSON       `json:"tab"`
		Root      paneJSON      `json:"root_pane"`
	}
	args := append([]string{"workspace", "create", "--cwd", cwd, "--label", label, "--no-focus"}, envFlags(env)...)
	err := c.call(&r, args...)
	return r.Workspace.conv(), r.Tab.conv(), r.Root.conv(), err
}

func (c *Client) CreateTab(workspace, cwd, label string, env []string) (connectors.Tab, connectors.Pane, error) {
	var r struct {
		Tab  tabJSON  `json:"tab"`
		Root paneJSON `json:"root_pane"`
	}
	args := append([]string{"tab", "create", "--workspace", workspace, "--cwd", cwd, "--label", label, "--no-focus"}, envFlags(env)...)
	err := c.call(&r, args...)
	return r.Tab.conv(), r.Root.conv(), err
}

func (c *Client) GetTab(id string) (connectors.Tab, error) {
	var r struct {
		Tab tabJSON `json:"tab"`
	}
	err := c.call(&r, "tab", "get", id)
	return r.Tab.conv(), err
}

func (c *Client) RenameTab(id, label string) error {
	return c.call(nil, "tab", "rename", id, label)
}

func (c *Client) FocusTab(id string) error {
	return c.call(nil, "tab", "focus", id)
}

// CloseTab closes a tab and every process in its panes.
func (c *Client) CloseTab(id string) error {
	return c.call(nil, "tab", "close", id)
}

// PanesInTab lists the panes of one tab (herdr lists per workspace).
func (c *Client) PanesInTab(tab connectors.Tab) ([]connectors.Pane, error) {
	var r struct {
		Panes []paneJSON `json:"panes"`
	}
	if err := c.call(&r, "pane", "list", "--workspace", tab.WorkspaceID); err != nil {
		return nil, err
	}
	var in []connectors.Pane
	for _, p := range r.Panes {
		if p.TabID == tab.ID {
			in = append(in, p.conv())
		}
	}
	return in, nil
}

func (c *Client) Split(pane, direction string, ratio float64, cwd string, env []string) (connectors.Pane, error) {
	var r struct {
		Pane paneJSON `json:"pane"`
	}
	args := []string{"pane", "split", pane, "--direction", direction,
		"--ratio", strconv.FormatFloat(ratio, 'f', 2, 64), "--cwd", cwd, "--no-focus"}
	err := c.call(&r, append(args, envFlags(env)...)...)
	return r.Pane.conv(), err
}

func (c *Client) RenamePane(pane, label string) error {
	return c.call(nil, "pane", "rename", pane, label)
}

func (c *Client) Run(pane, command string) error {
	return c.call(nil, "pane", "run", pane, command)
}

func (c *Client) Foreground(pane string) ([]connectors.Process, error) {
	var r struct {
		Info struct {
			Processes []processJSON `json:"foreground_processes"`
		} `json:"process_info"`
	}
	if err := c.call(&r, "pane", "process-info", "--pane", pane); err != nil {
		return nil, err
	}
	out := make([]connectors.Process, len(r.Info.Processes))
	for i, p := range r.Info.Processes {
		out[i] = connectors.Process(p)
	}
	return out, nil
}

// StartAgent launches an agent of kind in pane and waits until herdr sees it
// ready. The name is how other tools address it afterwards.
func (c *Client) StartAgent(name, kind, pane string, args []string) error {
	cmd := []string{"agent", "start", name, "--kind", kind, "--pane", pane}
	if len(args) > 0 {
		cmd = append(append(cmd, "--"), args...)
	}
	return c.call(nil, cmd...)
}

// Prompt submits text to the agent and returns once it's sent; herdr refuses
// an agent sitting at a dialog (agent_blocked) before typing anything.
func (c *Client) Prompt(agent, text string) error {
	return c.call(nil, "agent", "prompt", agent, text)
}

func (c *Client) WaitAgent(agent string, timeout time.Duration) (string, error) {
	var r struct {
		Agent struct {
			Status string `json:"agent_status"`
		} `json:"agent"`
	}
	err := c.call(&r, "agent", "wait", agent, "--timeout", strconv.FormatInt(timeout.Milliseconds(), 10))
	return r.Agent.Status, err
}

// CurrentPane is set by herdr in every pane it starts.
func (c *Client) CurrentPane() (string, bool) {
	id := os.Getenv("HERDR_PANE_ID")
	return id, os.Getenv("HERDR_ENV") == "1" && id != ""
}
