package models

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
)

func TestTheEngineCopyOfTheDescriptorsIsTheSame(t *testing.T) {
	engine, err := os.ReadFile(filepath.Join("..", "localengine", "catalog", "descriptors.json"))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(engine, descriptorsJSON) {
		t.Fatal("internal/localengine/catalog/descriptors.json differs from internal/models/data/descriptors.json; copy one over the other")
	}
}

func TestTheSwiftDescriptorsAreTheJSONDescriptorsOfItsFormats(t *testing.T) {
	src, err := os.ReadFile(filepath.Join("..", "..", "native", "macos", "SaysoEngine", "Sources", "SaysoEngine", "Descriptors.generated.swift"))
	if err != nil {
		t.Fatal(err)
	}
	_, rest, ok := strings.Cut(string(src), `#"""`)
	body, _, ok2 := strings.Cut(rest, `"""#`)
	if !ok || !ok2 {
		t.Fatal("no JSON literal in Descriptors.generated.swift")
	}
	var swift Descriptors
	if err := json.Unmarshal([]byte(body), &swift); err != nil {
		t.Fatal(err)
	}
	var formats []string
	for _, d := range swift.Descriptors {
		if !slices.Contains(formats, d.Format) {
			formats = append(formats, d.Format)
		}
	}
	want := AllDescriptors().OfFormats(formats...)
	if !reflect.DeepEqual(swift.Descriptors, want.Descriptors) {
		t.Error("the Swift descriptors are not the JSON descriptors of their formats")
	}
	for name, a := range want.Artifacts {
		if !reflect.DeepEqual(swift.Artifacts[name], a) {
			t.Errorf("the Swift artifact %s is not the one in descriptors.json", name)
		}
	}
	for name := range swift.Artifacts {
		if _, ok := want.Artifacts[name]; !ok {
			t.Errorf("the Swift artifact %s is not used by a descriptor of its formats", name)
		}
	}
}
