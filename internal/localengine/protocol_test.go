package localengine

import (
	"bytes"
	"encoding/json"
	"reflect"
	"strings"
	"sync"
	"testing"
)

// captured is a sink that keeps everything written to it.
type captured struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (c *captured) Write(p []byte) (int, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.buf.Write(p)
}

// messages is every message written so far, parsed.
func (c *captured) messages(t testing.TB) []map[string]any {
	t.Helper()
	c.mu.Lock()
	defer c.mu.Unlock()
	var out []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(c.buf.String()), "\n") {
		if line == "" {
			continue
		}
		var m map[string]any
		if err := json.Unmarshal([]byte(line), &m); err != nil {
			t.Fatalf("bad line %q: %v", line, err)
		}
		out = append(out, m)
	}
	return out
}

func (c *captured) clear() {
	c.mu.Lock()
	c.buf.Reset()
	c.mu.Unlock()
}

func strp(s string) *string { return &s }

func TestMessagesHaveVersionTypeAndID(t *testing.T) {
	var c captured
	out := NewOutput(&c)
	out.OK(strp("r1"), fields{"session": 3})
	out.Error("boom", nil)
	out.Log("info", "hi")
	m := c.messages(t)
	want := []map[string]any{
		{"v": 1.0, "type": "ok", "id": "r1", "session": 3.0},
		{"v": 1.0, "type": "error", "message": "boom"},
		{"v": 1.0, "type": "log", "level": "info", "message": "hi"},
	}
	if !reflect.DeepEqual(m, want) {
		t.Fatalf("%v", m)
	}
}

func TestRequestParsing(t *testing.T) {
	r, err := ParseRequest(`{"v":1,"type":"load","id":"r2","model":"m","session":7}`)
	if err != nil || r.Kind != "load" || r.ID == nil || *r.ID != "r2" {
		t.Fatalf("%+v %v", r, err)
	}
	if s, err := r.String("model"); err != nil || s != "m" {
		t.Fatalf("%q %v", s, err)
	}
	if n, err := r.Uint("session"); err != nil || n != 7 {
		t.Fatalf("%d %v", n, err)
	}
	if _, err := r.String("path"); err == nil {
		t.Fatal("missing path read")
	}
	if _, ok := r.OptionalUint("size_bytes"); ok {
		t.Fatal("missing size read")
	}
	if _, err := ParseRequest("nope"); err == nil || err.Error() != "invalid JSON line" {
		t.Fatalf("%v", err)
	}
	if _, err := ParseRequest(`{"v":1}`); err == nil || err.Error() != "missing type" {
		t.Fatalf("%v", err)
	}
	if _, err := ParseRequest("[1]"); err == nil {
		t.Fatal("array parsed")
	}
}
