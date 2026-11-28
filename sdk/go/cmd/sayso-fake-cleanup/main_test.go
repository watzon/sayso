package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/watzon/sayso/sdk/go/protocol"
)

func run(t *testing.T, s script, lines ...string) []map[string]any {
	t.Helper()
	var out bytes.Buffer
	serve(&runtime{script: s}, strings.NewReader(strings.Join(lines, "\n")+"\n"), &out)
	var replies []map[string]any
	for _, l := range strings.Split(strings.TrimSpace(out.String()), "\n") {
		var m map[string]any
		if err := json.Unmarshal([]byte(l), &m); err != nil {
			t.Fatalf("%q: %v", l, err)
		}
		replies = append(replies, m)
	}
	return replies
}

func TestTheFakeAnswersEveryRequest(t *testing.T) {
	model := filepath.Join(t.TempDir(), "m.gguf")
	os.WriteFile(model, []byte("gguf"), 0o600)
	r := run(t, script{},
		`{"v":1,"id":"1","type":"hello"}`,
		`{"v":1,"id":"2","type":"load","model_id":"s1-mini","path":"`+filepath.ToSlash(model)+`","n_ctx":64}`,
		`{"v":1,"id":"3","type":"count","model_id":"s1-mini","text":"um so hello"}`,
		`{"v":1,"id":"4","type":"complete","model_id":"s1-mini","prompt":"text: um so hello there\nout:","end":["\ntext:"],"max_tokens":20}`,
		`{"v":1,"id":"5","type":"complete","model_id":"s1-mini","prompt":"text: one two three four\nout:","end":[],"max_tokens":2}`,
		`{"v":1,"id":"6","type":"complete","model_id":"s1-mini","prompt":"text: a\nout:","end":[],"max_tokens":63}`,
		`{"v":1,"id":"7","type":"unload"}`,
	)
	want := []string{"hello", "loaded", "count", "completion", "completion", "completion", "unloaded"}
	for i, w := range want {
		if r[i]["type"] != w || r[i]["id"] != fmt.Sprint(i+1) {
			t.Fatalf("%d: %v", i, r[i])
		}
	}
	if r[0]["libraries"] != "found" || r[2]["tokens"] != 3.0 {
		t.Fatal(r[0], r[2])
	}
	if r[3]["stop"] != "end" || r[3]["text"] != " So hello there." {
		t.Fatal(r[3])
	}
	if r[4]["stop"] != "limit" || r[4]["text"] != " One two" {
		t.Fatal(r[4])
	}
	if r[5]["stop"] != "context" || r[5]["text"] != "" {
		t.Fatal(r[5])
	}
	for _, x := range r[3:6] {
		if !slices.Contains(protocol.CleanupStops, x["stop"].(string)) {
			t.Fatal(x)
		}
	}
}

func TestTheFakeGivesEveryErrorCode(t *testing.T) {
	model := filepath.Join(t.TempDir(), "m.gguf")
	os.WriteFile(model, []byte("gguf"), 0o600)
	r := run(t, script{Fail: map[string]failure{"unload": {"decode_failed", "panic: x"}}},
		`{"v":1,"id":"1","type":"load","model_id":"m"}`,
		`{"v":1,"id":"2","type":"count","model_id":"m","text":"a"}`,
		`{"v":1,"id":"3","type":"load","model_id":"m","path":"/nowhere/m.gguf","n_ctx":64}`,
		`{"v":1,"id":"4","type":"load","model_id":"m","path":"`+filepath.ToSlash(model)+`","n_ctx":64}`,
		`{"v":1,"id":"5","type":"count","model_id":"other","text":"a"}`,
		`{"v":1,"id":"6","type":"unload"}`,
	)
	var codes []string
	for _, x := range r {
		if x["type"] == "error" {
			codes = append(codes, x["code"].(string))
		}
	}
	if strings.Join(codes, ",") != "bad_request,not_loaded,load_failed,wrong_model,decode_failed" {
		t.Fatal(codes)
	}
	for _, c := range codes {
		if !slices.Contains(protocol.CleanupErrorCodes, c) {
			t.Fatal(c)
		}
	}
}

func TestTheFakeCrashesWhereTheScriptSays(t *testing.T) {
	var out bytes.Buffer
	code := serve(&runtime{script: script{CrashAt: 2}}, strings.NewReader("{\"v\":1,\"id\":\"1\",\"type\":\"hello\"}\n{\"v\":1,\"id\":\"2\",\"type\":\"hello\"}\n"), &out)
	if code != 1 || strings.Count(out.String(), "\n") != 1 {
		t.Fatal(code, out.String())
	}
}
