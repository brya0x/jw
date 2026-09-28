package registry

import (
	"path/filepath"
	"regexp"
	"testing"
)

func TestLoadMissingFileIsEmpty(t *testing.T) {
	r, err := Load(filepath.Join(t.TempDir(), "nope.json"))
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Entries) != 0 {
		t.Fatalf("want empty, got %d entries", len(r.Entries))
	}
}

func TestSaveThenLoad(t *testing.T) {
	path := filepath.Join(t.TempDir(), "jw", "registry.json")
	in := &Registry{Entries: []Entry{{ID: NewID(), Name: "web", Project: "myapp", Slot: 1}}}
	if err := in.Save(path); err != nil {
		t.Fatal(err)
	}
	out, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if out.Entries[0].Name != "web" {
		t.Fatalf("round trip lost data: %+v", out.Entries[0])
	}
}

func TestNextSlotFillsGaps(t *testing.T) {
	r := &Registry{Entries: []Entry{{Slot: 1}, {Slot: 3}}}
	if got := r.NextSlot(); got != 2 {
		t.Fatalf("want 2, got %d", got)
	}
}

func TestRemoveFreesSlot(t *testing.T) {
	r := &Registry{Entries: []Entry{{ID: "a", Slot: 1}, {ID: "b", Slot: 2}}}
	r.Remove("a")
	if len(r.Entries) != 1 || r.Entries[0].ID != "b" {
		t.Fatalf("got %+v", r.Entries)
	}
	if got := r.NextSlot(); got != 1 {
		t.Fatalf("slot 1 should be free again, got %d", got)
	}
}

func TestFindByNameAndPrefix(t *testing.T) {
	r := &Registry{Entries: []Entry{
		{ID: "0b1c9e2a-0000-4000-8000-000000000000", Name: "web", Project: "myapp"},
		{ID: "7f3ad011-0000-4000-8000-000000000000", Name: "api", Project: "myapp"},
	}}
	if e, _ := r.Find("myapp", "api"); e == nil || e.Name != "api" {
		t.Fatal("find by name failed")
	}
	if e, _ := r.Find("myapp", "0b1c"); e == nil || e.Name != "web" {
		t.Fatal("find by id prefix failed")
	}
	if _, err := r.Find("other", "web"); err == nil {
		t.Fatal("name should be scoped to project")
	}
}

func TestNewIDIsV4(t *testing.T) {
	v4 := regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)
	if id := NewID(); !v4.MatchString(id) {
		t.Fatalf("not a v4 uuid: %s", id)
	}
}
