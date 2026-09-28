package commands

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/brya0x/jw/internal/core/registry"
)

func key(s string) tea.KeyMsg {
	if s == "enter" {
		return tea.KeyMsg{Type: tea.KeyEnter}
	}
	return tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(s)}
}

// send feeds messages through Update the way the bubbletea loop would.
func send(m lsModel, msgs ...tea.Msg) (lsModel, tea.Cmd) {
	var cmd tea.Cmd
	for _, msg := range msgs {
		var next tea.Model
		next, cmd = m.Update(msg)
		m = next.(lsModel)
	}
	return m, cmd
}

func sampleRows() rowsMsg {
	return rowsMsg{rows: []lsRow{
		{Entry: registry.Entry{ID: "0b1c9e2a-x", Name: "web", Branch: "feat/web", Slot: 1, Path: "/wt/web"}, Tab: "open", PR: "#4 open"},
		{Entry: registry.Entry{ID: "7f3ad011-x", Name: "api", Branch: "feat/api", Slot: 2, Path: "/wt/api"}, Tab: "closed", PR: "#5 merged", State: "ready for done"},
	}}
}

func TestPickSecondRowForDone(t *testing.T) {
	m, cmd := send(newLsModel(nil, "myapp", false, ""), tea.WindowSizeMsg{Width: 120, Height: 30}, sampleRows(), key("j"), key("d"))

	if m.action != "done" || m.chosen == nil || m.chosen.Entry.Name != "api" {
		t.Fatalf("want done on api, got %q %+v", m.action, m.chosen)
	}
	if cmd == nil {
		t.Fatal("picking should quit the program")
	}
}

func TestEnterOpens(t *testing.T) {
	m, _ := send(newLsModel(nil, "myapp", false, ""), tea.WindowSizeMsg{Width: 120, Height: 30}, sampleRows(), key("enter"))
	if m.action != "open" || m.chosen.Entry.Name != "web" {
		t.Fatalf("want open on web, got %q", m.action)
	}
}

func TestNoPickWhileLoading(t *testing.T) {
	m, _ := send(newLsModel(nil, "myapp", false, ""), key("d"))
	if m.action != "" {
		t.Fatalf("nothing is loaded yet, got action %q", m.action)
	}
}

func TestViewShowsRowsDetailAndStatus(t *testing.T) {
	m, _ := send(newLsModel(nil, "myapp", false, "close web: ok"), tea.WindowSizeMsg{Width: 120, Height: 30}, sampleRows(), key("j"))
	v := m.View()
	for _, want := range []string{"web", "api", "#5 merged", "path   /wt/api", "close web: ok", "q quit"} {
		if !strings.Contains(v, want) {
			t.Errorf("view is missing %q:\n%s", want, v)
		}
	}
}

func TestProjectIsFixedAtStart(t *testing.T) {
	// After jw done deletes the worktree the cwd is gone; the scope must not
	// silently widen to every project.
	m, _ := send(newLsModel(nil, "myapp", false, "done web: ok"), tea.WindowSizeMsg{Width: 120, Height: 30}, sampleRows())
	if m.scope() != "myapp" || strings.Contains(m.View(), "PROJECT") {
		t.Fatalf("scope %q, view:\n%s", m.scope(), m.View())
	}
}

func TestToggleAllReloadsWithProjectColumn(t *testing.T) {
	m, cmd := send(newLsModel(nil, "myapp", false, ""), tea.WindowSizeMsg{Width: 120, Height: 30}, sampleRows(), key("a"))
	if !m.all || !m.loading || cmd == nil {
		t.Fatalf("a should toggle all and reload: all=%v loading=%v", m.all, m.loading)
	}

	m, _ = send(m, sampleRows())
	if !strings.Contains(m.View(), "PROJECT") {
		t.Fatal("rows from several projects should show the project column")
	}
}
