package cleanupd

import (
	"bytes"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// fakeEngine records the order of the frees.
type fakeEngine struct {
	log   *[]string
	libs  bool
	fail  bool
	opens int
}

func (e *fakeEngine) Backend() string    { return "cpu" }
func (e *fakeEngine) Version() string    { return "test" }
func (e *fakeEngine) HasLibraries() bool { return e.libs }
func (e *fakeEngine) Close()             { *e.log = append(*e.log, "backend") }
func (e *fakeEngine) Open(path string, nCtx uint32) (Model, error) {
	if e.fail {
		return nil, errors.New("not a model")
	}
	e.opens++
	*e.log = append(*e.log, "open "+filepath.Base(path))
	return &fakeModel{log: e.log, name: filepath.Base(path)}, nil
}

type fakeModel struct {
	log  *[]string
	name string
}

func (m *fakeModel) Tokens(text string) (int, error) { return len(strings.Fields(text)), nil }
func (m *fakeModel) Close() {
	*m.log = append(*m.log, "context "+m.name, "model "+m.name)
}
func (m *fakeModel) Complete(prompt string, end []string, maxTokens, nCtx uint32) (Completion, error) {
	if prompt == "panic" {
		panic("boom")
	}
	return Completion{Text: []byte(" " + prompt + "\xff"), Stop: StopEnd, TokensIn: 2, TokensOut: 1}, nil
}

func modelFile(t *testing.T, name string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(p, []byte("gguf"), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

func call(t *testing.T, r *Runtime, line string) map[string]any {
	t.Helper()
	rep := r.handleLine(line)
	b, _ := json.Marshal(rep)
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatal(err)
	}
	return m
}

func TestBadLinesAreBadRequestsWithTheIDWhenReadable(t *testing.T) {
	var log []string
	r := NewRuntime(&fakeEngine{log: &log})
	for line, id := range map[string]string{
		"nope":                                "",
		`{"v":1,"id":"7","type":"fly"}`:       "7",
		"{}":                                  "",
		"[1]":                                 "",
		`{"v":1,"id":"8","type":"load"}`:      "8",
		`{"v":-1,"id":"9","type":"hello"}`:    "9",
		`{"v":1,"id":"10","type":"complete"}`: "10",
	} {
		m := call(t, r, line)
		if m["type"] != "error" || m["code"] != BadRequest || m["id"] != id || m["v"] != float64(1) {
			t.Errorf("%s: %v", line, m)
		}
	}
}

func TestAForeignVersionIsRefusedExceptForHello(t *testing.T) {
	var log []string
	r := NewRuntime(&fakeEngine{log: &log, libs: true})
	if m := call(t, r, `{"v":2,"id":"1","type":"unload"}`); m["code"] != BadRequest {
		t.Fatal(m)
	}
	m := call(t, r, `{"v":2,"id":"2","type":"hello"}`)
	if m["type"] != "hello" || m["version"] != float64(1) || m["backend"] != "cpu" || m["libraries"] != "found" {
		t.Fatal(m)
	}
}

func TestModelRequestsAndTheFreeOrder(t *testing.T) {
	var log []string
	e := &fakeEngine{log: &log}
	r := NewRuntime(e)
	a, b := modelFile(t, "a.gguf"), modelFile(t, "b.gguf")
	if m := call(t, r, `{"v":1,"id":"1","type":"count","model_id":"a","text":"x"}`); m["code"] != NotLoaded {
		t.Fatal(m)
	}
	if m := call(t, r, `{"v":1,"id":"2","type":"load","model_id":"a","path":"/no/such.gguf","n_ctx":2048}`); m["code"] != LoadFailed {
		t.Fatal(m)
	}
	if m := call(t, r, `{"v":1,"id":"3","type":"load","model_id":"a","path":`+jsonString(a)+`,"n_ctx":0}`); m["code"] != BadRequest {
		t.Fatal(m)
	}
	if m := call(t, r, `{"v":1,"id":"4","type":"load","model_id":"a","path":`+jsonString(a)+`,"n_ctx":2048}`); m["type"] != "loaded" || m["model_id"] != "a" {
		t.Fatal(m)
	}
	// The same load again is a no-op.
	if m := call(t, r, `{"v":1,"id":"5","type":"load","model_id":"a","path":`+jsonString(a)+`,"n_ctx":2048}`); m["load_ms"] != float64(0) || e.opens != 1 {
		t.Fatal(m, e.opens)
	}
	if m := call(t, r, `{"v":1,"id":"6","type":"count","model_id":"b","text":"x"}`); m["code"] != WrongModel {
		t.Fatal(m)
	}
	if m := call(t, r, `{"v":1,"id":"7","type":"count","model_id":"a","text":"three small words"}`); m["tokens"] != float64(3) {
		t.Fatal(m)
	}
	m := call(t, r, `{"v":1,"id":"8","type":"complete","model_id":"a","prompt":"hi","end":[],"max_tokens":4}`)
	if m["type"] != "completion" || m["text"] != " hi�" || m["stop"] != StopEnd || m["tokens_in"] != float64(2) {
		t.Fatal(m)
	}
	// A panic is decode_failed, and the runtime keeps working.
	if m := call(t, r, `{"v":1,"id":"9","type":"complete","model_id":"a","prompt":"panic","end":[],"max_tokens":4}`); m["code"] != DecodeFailed || !strings.Contains(m["message"].(string), "boom") {
		t.Fatal(m)
	}
	// A swap frees the old model first.
	if m := call(t, r, `{"v":1,"id":"10","type":"load","model_id":"b","path":`+jsonString(b)+`,"n_ctx":2048}`); m["type"] != "loaded" {
		t.Fatal(m)
	}
	r.Close()
	r.Close()
	want := []string{"open a.gguf", "context a.gguf", "model a.gguf", "open b.gguf", "context b.gguf", "model b.gguf", "backend"}
	if !reflect.DeepEqual(log, want) {
		t.Fatalf("frees %v, want %v", log, want)
	}
}

func TestServeSkipsBlankLinesAndEndsAtEOF(t *testing.T) {
	var log []string
	r := NewRuntime(&fakeEngine{log: &log})
	in := strings.NewReader("\n  \n{\"v\":1,\"id\":\"1\",\"type\":\"unload\"}\n\xff\xfe\n{\"v\":1,\"id\":\"2\",\"type\":\"hello\"}")
	var out bytes.Buffer
	if err := Serve(r, in, &out); err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(out.String()), "\n")
	if len(lines) != 3 || !strings.Contains(lines[0], `"unloaded"`) || !strings.Contains(lines[1], "UTF-8") || !strings.Contains(lines[2], `"hello"`) {
		t.Fatal(out.String())
	}
}

func TestFindEnd(t *testing.T) {
	ends := []string{"<|im_end|>", "\n\n"}
	for _, c := range []struct {
		text string
		from int
		want int
	}{
		{"Hi.<|im_end|>x", 0, 3},
		{"a\n\nb<|im_end|>", 0, 1},
		{"a\n\nb", 2, -1},
		{"abc", 3, -1},
	} {
		if got := FindEnd([]byte(c.text), ends, c.from); got != c.want {
			t.Errorf("%q from %d: %d, want %d", c.text, c.from, got, c.want)
		}
	}
	if FindEnd([]byte("abc"), nil, 0) != -1 || FindEnd([]byte("abc"), []string{""}, 0) != -1 {
		t.Fatal("no ends must not match")
	}
}

func jsonString(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}
