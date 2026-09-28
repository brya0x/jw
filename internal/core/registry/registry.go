// Package registry persists the list of jw worktrees to
// ~/.local/state/jw/registry.json.
package registry

import (
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// Entry is one worktree managed by jw.
type Entry struct {
	ID      string    `json:"id"`
	Name    string    `json:"name"`
	Project string    `json:"project"`
	Branch  string    `json:"branch"`
	Path    string    `json:"path"`
	Slot    int       `json:"slot"`
	Tab     string    `json:"tab,omitempty"` // empty when the tab is closed
	PR      int       `json:"pr,omitempty"`
	Opened  bool      `json:"opened,omitempty"`  // an agent ran here before: resume, don't start fresh
	Adopted bool      `json:"adopted,omitempty"` // the branch existed before jw: never jw's to delete
	Created time.Time `json:"created"`
}

// Registry is the whole file on disk.
type Registry struct {
	Entries []Entry `json:"entries"`
}

// DefaultPath honours XDG_STATE_HOME, falling back to ~/.local/state.
func DefaultPath() (string, error) {
	if dir := os.Getenv("XDG_STATE_HOME"); dir != "" {
		return filepath.Join(dir, "jw", "registry.json"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".local", "state", "jw", "registry.json"), nil
}

// Load reads the registry. A missing file is not an error: it is an empty registry.
func Load(path string) (*Registry, error) {
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return &Registry{}, nil
	}
	if err != nil {
		return nil, err
	}

	var r Registry
	if err := json.Unmarshal(data, &r); err != nil {
		return nil, fmt.Errorf("parse %s: %w", path, err)
	}
	return &r, nil
}

// Save writes to a temp file and renames it, so a crash never leaves half a file.
func (r *Registry) Save(path string) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(r, "", "  ")
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// Find looks up an entry of a project by name or by id prefix.
func (r *Registry) Find(project, key string) (*Entry, error) {
	var match *Entry
	for i := range r.Entries {
		e := &r.Entries[i]
		if e.Project != project {
			continue
		}
		if e.Name == key {
			return e, nil
		}
		if len(key) >= 4 && len(e.ID) >= len(key) && e.ID[:len(key)] == key {
			if match != nil {
				return nil, fmt.Errorf("id prefix %q is ambiguous", key)
			}
			match = e
		}
	}
	if match == nil {
		return nil, fmt.Errorf("no worktree %q in %s", key, project)
	}
	return match, nil
}

// Has reports whether project already has a worktree with exactly this name.
func (r *Registry) Has(project, name string) bool {
	for _, e := range r.Entries {
		if e.Project == project && e.Name == name {
			return true
		}
	}
	return false
}

// Add appends an entry.
func (r *Registry) Add(e Entry) {
	r.Entries = append(r.Entries, e)
}

// Remove deletes the entry with that id, freeing its slot. It shifts
// Entries, so a *Entry obtained from Find is invalid afterwards.
func (r *Registry) Remove(id string) {
	for i, e := range r.Entries {
		if e.ID == id {
			r.Entries = append(r.Entries[:i], r.Entries[i+1:]...)
			return
		}
	}
}

// NextSlot returns the lowest slot not used by any project.
func (r *Registry) NextSlot() int {
	used := map[int]bool{}
	for _, e := range r.Entries {
		used[e.Slot] = true
	}
	slot := 1
	for used[slot] {
		slot++
	}
	return slot
}

// NewID returns a random RFC 4122 version 4 UUID.
func NewID() string {
	var b [16]byte
	rand.Read(b[:])             // crypto/rand.Read never fails on supported platforms
	b[6] = (b[6] & 0x0f) | 0x40 // version 4
	b[8] = (b[8] & 0x3f) | 0x80 // variant 10
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}
