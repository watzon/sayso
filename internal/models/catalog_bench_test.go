package models

import "testing"

func BenchmarkFind(b *testing.B) {
	id := Catalog()[len(Catalog())-1].ID
	b.ReportAllocs()
	for range b.N {
		if _, ok := Find(id); !ok {
			b.Fatal("not found")
		}
	}
}

// The overlay finds the preview model in every frame: the speech catalog is
// built once, and an append to it never writes into the shared copy.
func TestCatalogIsBuiltOnce(t *testing.T) {
	if n := testing.AllocsPerRun(100, func() { Catalog() }); n != 0 {
		t.Fatalf("Catalog allocates %v times", n)
	}
	cat := Catalog()
	_ = append(cat, ModelInfo{ID: "intruder"})
	_ = append(LocalModels(), ModelInfo{ID: "intruder"})
	if _, ok := Find("intruder"); ok {
		t.Fatal("an append changed the catalog")
	}
	if _, ok := FindLocal("intruder"); ok {
		t.Fatal("an append changed the local models")
	}
}
