package commands

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/bubbles/table"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/brya0x/jw/internal/core/config"
	"github.com/brya0x/jw/internal/core/registry"
)

// The TUI is a picker: it chooses a worktree and an action, then exits so the
// action runs on the real terminal (jw done asks for confirmation, jw open
// moves focus). Afterwards it comes back, unless the action was open.
func (a *App) lsInteractive(project string, all bool) error {
	status := ""
	for {
		final, err := tea.NewProgram(newLsModel(a, project, all, status), tea.WithAltScreen()).Run()
		if err != nil {
			return err
		}
		m := final.(lsModel)
		if m.action == "" {
			return nil
		}
		all = m.all

		err = a.act(m.action, m.chosen.Entry)
		if m.action == "open" && err == nil {
			return nil // focus is on the new tab now
		}
		if err != nil {
			status = fmt.Sprintf("%s %s: %v", m.action, m.chosen.Entry.Name, err)
		} else {
			status = fmt.Sprintf("%s %s: ok", m.action, m.chosen.Entry.Name)
		}
	}
}

// act runs an action on the chosen worktree. The row may belong to any
// project, so its own project is loaded from its path; nothing depends on
// the cwd.
func (a *App) act(action string, chosen registry.Entry) error {
	p, err := loadProject(chosen.Path)
	if err != nil {
		return fmt.Errorf("worktree is missing: %w", err)
	}
	e, err := p.reg.Find(p.name, chosen.ID)
	if err != nil {
		return err
	}
	if err := a.follow(p, e); err != nil {
		return err
	}
	switch action {
	case "open":
		return a.open(p, e, openOptions{})
	case "close":
		return a.close(p, e, false)
	case "done":
		return a.done(p, e)
	case "rm":
		return a.rm(p, e, rmOptions{})
	case "sync":
		return a.sync(p, e, syncOptions{})
	}
	return fmt.Errorf("unknown action %q", action)
}

// rowsMsg carries the result of loading rows in the background.
type rowsMsg struct {
	rows []lsRow
	err  error
}

type lsModel struct {
	app         *App
	table       table.Model
	rows        []lsRow
	project     string // the project jw ls -i started in; "" outside a repo
	showProject bool
	all         bool
	loading     bool
	status      string
	err         error

	// set when the user picks something; read by runLsInteractive
	action string
	chosen *lsRow
}

func newLsModel(app *App, project string, all bool, status string) lsModel {
	t := table.New(table.WithFocused(true))
	styles := table.DefaultStyles()
	styles.Header = styles.Header.Bold(true).BorderStyle(lipgloss.NormalBorder()).BorderBottom(true)
	styles.Selected = styles.Selected.Foreground(lipgloss.Color("0")).Background(lipgloss.Color("6"))
	t.SetStyles(styles)
	m := lsModel{app: app, table: t, project: project, all: all, loading: true, status: status}
	m.showProject = m.scope() == ""
	return m
}

// scope is the project to list: none (every project) when all is on.
func (m lsModel) scope() string {
	if m.all {
		return ""
	}
	return m.project
}

// loadRowsCmd runs loadRows off the UI loop: gh and herdr calls take a moment,
// and the screen should not freeze while they do.
func (m lsModel) loadRowsCmd() tea.Cmd {
	app, project := m.app, m.scope()
	return func() tea.Msg {
		rows, err := app.loadRows(project)
		return rowsMsg{rows: rows, err: err}
	}
}

func (m lsModel) Init() tea.Cmd {
	return m.loadRowsCmd()
}

func (m lsModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case rowsMsg:
		m.loading = false
		m.err = msg.err
		m.rows = msg.rows
		m.showProject = m.scope() == ""
		m.fillTable()
		return m, nil

	case tea.WindowSizeMsg:
		m.table.SetHeight(max(3, msg.Height-10))
		return m, nil

	case tea.KeyMsg:
		switch msg.String() {
		case "q", "esc", "ctrl+c":
			return m, tea.Quit
		case "a":
			m.all, m.loading, m.status = !m.all, true, ""
			return m, m.loadRowsCmd()
		case "r":
			m.loading, m.status = true, ""
			return m, m.loadRowsCmd()
		case "enter", "o":
			return m.pick("open")
		case "c":
			return m.pick("close")
		case "d":
			return m.pick("done")
		case "x":
			return m.pick("rm")
		case "s":
			return m.pick("sync")
		}
	}

	var cmd tea.Cmd
	m.table, cmd = m.table.Update(msg) // arrows, j/k, pgup/pgdown
	return m, cmd
}

func (m lsModel) pick(action string) (tea.Model, tea.Cmd) {
	if len(m.rows) == 0 || m.loading {
		return m, nil
	}
	row := m.rows[m.table.Cursor()]
	m.action, m.chosen = action, &row
	return m, tea.Quit
}

func (m *lsModel) fillTable() {
	cols := []table.Column{
		{Title: "NAME", Width: 16}, {Title: "BRANCH", Width: 24}, {Title: "SLOT", Width: 4},
		{Title: "TAB", Width: 6}, {Title: "PR", Width: 12}, {Title: "STATE", Width: 14},
	}
	if m.showProject {
		cols = append([]table.Column{{Title: "PROJECT", Width: 12}}, cols...)
	}

	rows := make([]table.Row, len(m.rows))
	for i, r := range m.rows {
		row := table.Row{r.Entry.Name, r.Entry.Branch, fmt.Sprint(r.Entry.Slot), r.Tab, r.PR, r.State}
		if m.showProject {
			row = append(table.Row{r.Entry.Project}, row...)
		}
		rows[i] = row
	}

	// Columns and rows must agree in length at every step, so clear the rows
	// before switching columns (the project column comes and goes with `a`).
	// SetRows(nil) drops the cursor to -1, so keep it and put it back.
	cursor := m.table.Cursor()
	m.table.SetRows(nil)
	m.table.SetColumns(cols)
	m.table.SetRows(rows)
	m.table.SetCursor(min(max(cursor, 0), max(len(rows)-1, 0)))
}

var (
	titleStyle  = lipgloss.NewStyle().Bold(true)
	dimStyle    = lipgloss.NewStyle().Faint(true)
	statusStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("3"))
	errStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("1"))
	stateStyles = map[string]lipgloss.Style{
		"ready for done": lipgloss.NewStyle().Foreground(lipgloss.Color("2")),
		"dirty":          lipgloss.NewStyle().Foreground(lipgloss.Color("3")),
		"missing":        lipgloss.NewStyle().Foreground(lipgloss.Color("1")),
	}
)

func (m lsModel) View() string {
	var b strings.Builder

	scope := "this project"
	if m.showProject {
		scope = "all projects"
	}
	b.WriteString(titleStyle.Render("jw") + dimStyle.Render(" — "+scope) + "\n\n")

	switch {
	case m.err != nil:
		b.WriteString(errStyle.Render(m.err.Error()) + "\n")
	case m.loading && len(m.rows) == 0:
		b.WriteString(dimStyle.Render("loading…") + "\n")
	case len(m.rows) == 0:
		b.WriteString("no worktrees yet — create one with `jw new <name>`\n")
	default:
		b.WriteString(m.table.View() + "\n\n")
		b.WriteString(m.detail())
	}

	if m.status != "" {
		b.WriteString("\n" + statusStyle.Render(m.status) + "\n")
	}
	help := "↑/↓ move · enter open · c close · s sync · d done · x rm · a all projects · r refresh · q quit"
	if m.loading && len(m.rows) > 0 {
		help = "refreshing… · " + help
	}
	b.WriteString("\n" + dimStyle.Render(help))
	return b.String()
}

// detail describes the selected worktree under the table.
func (m lsModel) detail() string {
	r := m.rows[m.table.Cursor()]
	e := r.Entry
	base := config.PortBase(e.Slot)

	lines := []string{
		fmt.Sprintf("%s  %s", titleStyle.Render(e.Name), dimStyle.Render(e.ID)),
		"path   " + e.Path,
		fmt.Sprintf("ports  %d–%d (slot %d)", base, base+config.PortBlockSize-1, e.Slot),
	}
	if r.PRURL != "" {
		lines = append(lines, "pr     "+r.PRURL)
	}
	if r.State != "" {
		lines = append(lines, "state  "+stateStyles[r.State].Render(r.State))
	}
	return strings.Join(lines, "\n") + "\n"
}
