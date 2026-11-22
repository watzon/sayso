package config

import (
	"encoding/json"
	"os"
	"testing"
)

// The expected text comes from toml-rs 0.9 (toml_writer 1.1.2), serializing a
// one-entry map whose key and value are both the case string.
func TestStringsAndKeysMatchTomlRs(t *testing.T) {
	data, err := os.ReadFile("testdata/toml_strings_golden.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct{ In, TOML string }
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		got := newTable().Set(c.In, c.In).Encode()
		if got != c.TOML {
			t.Errorf("%q:\n got %q\nwant %q", c.In, got, c.TOML)
		}
		// What we write reads back as the same string.
		doc, err := parseDocument(got)
		if err != nil || doc[c.In] != c.In {
			t.Errorf("%q does not round-trip: %v %v", c.In, doc, err)
		}
	}
}
