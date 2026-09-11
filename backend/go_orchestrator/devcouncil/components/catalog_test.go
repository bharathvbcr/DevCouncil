package components

import (
	"reflect"
	"testing"
)

func TestResolveEmptyIsAll(t *testing.T) {
	got, err := Resolve(nil)
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"host", "devmap", "dcstore", "dcverify", "dcgrep"}
	if ids := idsOf(got); !reflect.DeepEqual(ids, want) {
		t.Fatalf("ids=%v want %v", ids, want)
	}
}

func TestResolveDevmapIsStandalone(t *testing.T) {
	got, err := Resolve([]string{"devmap"})
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0].ID != "devmap" || IncludesHost(got) {
		t.Fatalf("%+v", got)
	}
	if !got[0].Standalone {
		t.Fatal("devmap must be marked standalone")
	}
}

func TestResolveAnalysisSkipsHost(t *testing.T) {
	got, err := Resolve([]string{"analysis"})
	if err != nil {
		t.Fatal(err)
	}
	if IncludesHost(got) {
		t.Fatalf("analysis included host: %v", idsOf(got))
	}
	if !reflect.DeepEqual(idsOf(got), []string{"devmap", "dcstore", "dcverify", "dcgrep"}) {
		t.Fatalf("%v", idsOf(got))
	}
}

func TestResolveUnknown(t *testing.T) {
	if _, err := Resolve([]string{"uv"}); err == nil {
		t.Fatal("expected unknown")
	}
}

func TestPresetNamesStayInSyncWithCatalog(t *testing.T) {
	for name, ids := range Presets {
		for _, id := range ids {
			if _, ok := Lookup(id); !ok {
				t.Fatalf("preset %q names unknown id %q", name, id)
			}
		}
	}
}

func idsOf(cs []Component) []string {
	out := make([]string, len(cs))
	for i, c := range cs {
		out[i] = c.ID
	}
	return out
}
