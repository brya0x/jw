package commands

import (
	"encoding/json"
	"sort"

	"github.com/brya0x/jw/internal/core/config"
	"github.com/brya0x/jw/internal/core/registry"
)

// streamJSON is one stream as jw ls/info/new --json print it. Its field
// names are part of jw's interface: add fields, don't rename them.
type streamJSON struct {
	ID        string              `json:"id"`
	Name      string              `json:"name"`
	Project   string              `json:"project"`
	Branch    string              `json:"branch"`
	Adopted   bool                `json:"adopted"`
	Path      string              `json:"path"`
	Missing   bool                `json:"missing"` // the worktree is gone from disk
	Slot      int                 `json:"slot"`
	PortBase  int                 `json:"port_base"`
	Ports     map[string]portJSON `json:"ports"`
	Tab       string              `json:"tab,omitempty"`
	Open      bool                `json:"open"`
	PR        *prJSON             `json:"pr"` // null: no PR (or unknown, see pr_unknown)
	PRUnknown bool                `json:"pr_unknown,omitempty"`
	State     string              `json:"state"` // "", "missing", "dirty" or "ready for done"
}

type portJSON struct {
	Port      int  `json:"port"`
	Listening bool `json:"listening"` // something serves on it right now
}

type prJSON struct {
	Number int    `json:"number"`
	State  string `json:"state"` // open, draft, closed or merged
	URL    string `json:"url"`
}

// streamJSON builds the JSON view of one row. cfg may be nil when the
// project's config can't be loaded (its worktrees are all gone): ports are
// then left empty rather than guessed.
func (a *App) streamJSON(r lsRow, cfg *config.Config) streamJSON {
	e := r.Entry
	s := streamJSON{
		ID: e.ID, Name: e.Name, Project: e.Project, Branch: e.Branch, Adopted: e.Adopted,
		Path: e.Path, Missing: r.State == "missing", Slot: e.Slot,
		PortBase: config.PortBase(e.Slot), Ports: map[string]portJSON{},
		Tab: e.Tab, Open: e.Tab != "", State: r.State, PRUnknown: r.unknown,
	}
	if r.pr != nil {
		s.PR = &prJSON{Number: r.pr.Number, State: r.pr.Status(), URL: r.pr.URL}
	}
	if cfg != nil {
		for svc, port := range cfg.PortsFor(e.Slot) {
			_, busy := a.Shell.PortOwner(port)
			s.Ports[svc] = portJSON{Port: port, Listening: busy}
		}
	}
	return s
}

// streamsJSON converts rows from any number of projects, loading each
// project's config once.
func (a *App) streamsJSON(rows []lsRow) []streamJSON {
	cfgs := map[string]*config.Config{}
	for _, r := range rows {
		if _, done := cfgs[r.Entry.Project]; done || r.State == "missing" {
			continue
		}
		if p, err := loadProject(r.Entry.Path); err == nil {
			cfgs[r.Entry.Project] = p.cfg
		}
	}
	out := make([]streamJSON, 0, len(rows))
	for _, r := range rows {
		out = append(out, a.streamJSON(r, cfgs[r.Entry.Project]))
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Slot < out[j].Slot })
	return out
}

// row reconciles a single entry the way jw ls does, for info and new.
func (a *App) row(p *project, e registry.Entry) lsRow {
	rows, err := a.loadRows(p.name)
	if err == nil {
		for _, r := range rows {
			if r.Entry.ID == e.ID {
				return r
			}
		}
	}
	return lsRow{Entry: e, Tab: "closed", PR: "?", unknown: true}
}

func (a *App) writeJSON(v any) error {
	enc := json.NewEncoder(a.Out)
	enc.SetIndent("", "  ")
	return enc.Encode(v)
}
