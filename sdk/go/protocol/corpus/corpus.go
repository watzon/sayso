// Package corpus reads the wire corpus of sdk/protocol/testdata, for the
// tests of the SDKs and of the host.
package corpus

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strings"
)

// Case is one message of the corpus.
type Case struct {
	File string
	Line int
	Name string
	// DecodeOnly cases need not encode to the same JSON.
	DecodeOnly bool
	Message    string
}

func (c Case) String() string { return fmt.Sprintf("%s:%d %s", c.File, c.Line, c.Name) }

// Dir is sdk/protocol/testdata of this checkout.
func Dir() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "..", "..", "protocol", "testdata")
}

// Read reads the cases of one protocol ("host", "engine", "cleanup") whose
// file names start with prefix ("" for all).
func Read(protocol, prefix string) ([]Case, error) {
	files, err := filepath.Glob(filepath.Join(Dir(), protocol, prefix+"*.ndjson"))
	if err != nil {
		return nil, err
	}
	if len(files) == 0 {
		return nil, fmt.Errorf("no corpus files for %s/%s*", protocol, prefix)
	}
	sort.Strings(files)
	var out []Case
	for _, f := range files {
		cases, err := readFile(f)
		if err != nil {
			return nil, err
		}
		out = append(out, cases...)
	}
	return out, nil
}

func readFile(path string) ([]Case, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	name := filepath.Join(filepath.Base(filepath.Dir(path)), filepath.Base(path))
	var out []Case
	var pending string
	scanner := bufio.NewScanner(bytes.NewReader(data))
	scanner.Buffer(make([]byte, 1<<20), 1<<20)
	for n := 1; scanner.Scan(); n++ {
		line := strings.TrimSpace(scanner.Text())
		switch {
		case line == "":
		case strings.HasPrefix(line, "#"):
			pending = strings.TrimSpace(strings.TrimPrefix(line, "#"))
		default:
			if pending == "" {
				return nil, fmt.Errorf("%s:%d: a message without a comment line", name, n)
			}
			c := Case{File: name, Line: n, Name: pending, Message: line}
			if rest, ok := strings.CutPrefix(pending, "decode:"); ok {
				c.Name, c.DecodeOnly = strings.TrimSpace(rest), true
			}
			out = append(out, c)
			pending = ""
		}
	}
	return out, scanner.Err()
}

// Same compares two JSON objects as values. A null field counts as missing.
func Same(a, b string) (bool, error) {
	var va, vb any
	if err := json.Unmarshal([]byte(a), &va); err != nil {
		return false, err
	}
	if err := json.Unmarshal([]byte(b), &vb); err != nil {
		return false, err
	}
	return reflect.DeepEqual(dropNulls(va), dropNulls(vb)), nil
}

func dropNulls(v any) any {
	switch v := v.(type) {
	case map[string]any:
		out := map[string]any{}
		for k, x := range v {
			if x != nil {
				out[k] = dropNulls(x)
			}
		}
		return out
	case []any:
		out := make([]any, len(v))
		for i, x := range v {
			out[i] = dropNulls(x)
		}
		return out
	}
	return v
}
