// Package herdr drives the herdr CLI. Every call parses herdr's JSON reply
// and takes IDs from it; nothing is guessed from what is focused.
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
)

type Client struct {
	bin string
}

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

type Workspace struct {
	ID    string `json:"workspace_id"`
	Label string `json:"label"`
}

type Tab struct {
	ID          string `json:"tab_id"`
	Label       string `json:"label"`
	WorkspaceID string `json:"workspace_id"`
}

type Pane struct {
	ID    string `json:"pane_id"`
	TabID string `json:"tab_id"`
	Label string `json:"label"`
}

// Error is herdr's own error reply, e.g. {"code":"tab_not_found", ...}.
type Error struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

func (e *Error) Error() string { return "herdr: " + e.Message }

// IsNotFound reports whether err is herdr saying the target doesn't exist.
func IsNotFound(err error) bool {
	var he *Error
	return errors.As(err, &he) && strings.HasSuffix(he.Code, "_not_found")
}

// IsNotReady reports whether an agent started but stopped at a dialog
// (folder trust, login, approval) instead of reaching its prompt.
func IsNotReady(err error) bool {
	var he *Error
	return errors.As(err, &he) && he.Code == "agent_not_ready"
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

func (c *Client) Workspaces() ([]Workspace, error) {
	var r struct {
		Workspaces []Workspace `json:"workspaces"`
	}
	err := c.call(&r, "workspace", "list")
	return r.Workspaces, err
}

// CreateWorkspace makes a workspace; herdr gives it one tab with one pane.
// env reaches only that first pane.
func (c *Client) CreateWorkspace(cwd, label string, env []string) (Workspace, Tab, Pane, error) {
	var r struct {
		Workspace Workspace `json:"workspace"`
		Tab       Tab       `json:"tab"`
		Root      Pane      `json:"root_pane"`
	}
	args := append([]string{"workspace", "create", "--cwd", cwd, "--label", label, "--no-focus"}, envFlags(env)...)
	err := c.call(&r, args...)
	return r.Workspace, r.Tab, r.Root, err
}

// CreateTab adds a tab with one pane to a workspace. env reaches only that pane.
func (c *Client) CreateTab(workspace, cwd, label string, env []string) (Tab, Pane, error) {
	var r struct {
		Tab  Tab  `json:"tab"`
		Root Pane `json:"root_pane"`
	}
	args := append([]string{"tab", "create", "--workspace", workspace, "--cwd", cwd, "--label", label, "--no-focus"}, envFlags(env)...)
	err := c.call(&r, args...)
	return r.Tab, r.Root, err
}

func (c *Client) GetTab(id string) (Tab, error) {
	var r struct {
		Tab Tab `json:"tab"`
	}
	err := c.call(&r, "tab", "get", id)
	return r.Tab, err
}

// CloseTab closes a tab and every process in its panes.
func (c *Client) CloseTab(id string) error {
	return c.call(nil, "tab", "close", id)
}

// PanesInTab lists the panes of one tab.
func (c *Client) PanesInTab(tab Tab) ([]Pane, error) {
	var r struct {
		Panes []Pane `json:"panes"`
	}
	if err := c.call(&r, "pane", "list", "--workspace", tab.WorkspaceID); err != nil {
		return nil, err
	}
	var in []Pane
	for _, p := range r.Panes {
		if p.TabID == tab.ID {
			in = append(in, p)
		}
	}
	return in, nil
}

// Process is one process in the foreground of a pane.
type Process struct {
	Name    string `json:"name"`
	Cmdline string `json:"cmdline"`
}

// Foreground lists what the pane is running right now — just the shell when idle.
func (c *Client) Foreground(pane string) ([]Process, error) {
	var r struct {
		Info struct {
			Processes []Process `json:"foreground_processes"`
		} `json:"process_info"`
	}
	err := c.call(&r, "pane", "process-info", "--pane", pane)
	return r.Info.Processes, err
}

func (c *Client) RenameTab(id, label string) error {
	return c.call(nil, "tab", "rename", id, label)
}

func (c *Client) FocusTab(id string) error {
	return c.call(nil, "tab", "focus", id)
}

// Split cuts pane in two and returns the new one. ratio is the share the
// original pane keeps. env reaches only the new pane — splits don't inherit it.
func (c *Client) Split(pane, direction string, ratio float64, cwd string, env []string) (Pane, error) {
	var r struct {
		Pane Pane `json:"pane"`
	}
	args := []string{"pane", "split", pane, "--direction", direction,
		"--ratio", strconv.FormatFloat(ratio, 'f', 2, 64), "--cwd", cwd, "--no-focus"}
	err := c.call(&r, append(args, envFlags(env)...)...)
	return r.Pane, err
}

// Run types command into the pane's shell and presses enter.
func (c *Client) Run(pane, command string) error {
	return c.call(nil, "pane", "run", pane, command)
}

func (c *Client) RenamePane(pane, label string) error {
	return c.call(nil, "pane", "rename", pane, label)
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
