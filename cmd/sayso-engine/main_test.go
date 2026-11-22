//go:build !darwin

package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"
)

// The test binary runs itself as the engine when this is set.
const childEnv = "SAYSO_ENGINE_TEST_CHILD"

func TestMain(m *testing.M) {
	if os.Getenv(childEnv) == "1" {
		main()
	}
	os.Exit(m.Run())
}

func runEngine(t *testing.T, env []string, input string) (stdout, stderr string, code int) {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(exe, "-test.run=^$")
	cmd.Env = append(append(os.Environ(), childEnv+"=1"), env...)
	cmd.Stdin = strings.NewReader(input)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	err = cmd.Run()
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		code = exit.ExitCode()
	} else if err != nil {
		t.Fatal(err)
	}
	return out.String(), errb.String(), code
}

func TestTheEngineSpeaksTheProtocolOnStdoutOnly(t *testing.T) {
	stdout, stderr, code := runEngine(t, []string{"SAYSO_MODELS_DIR=" + t.TempDir(), "SAYSO_MODEL_STORE="},
		`{"v":1,"type":"hello","id":"h"}`+"\n"+`{"v":1,"type":"shutdown","id":"s"}`+"\n")
	if code != 0 {
		t.Fatalf("exit %d, stderr %s", code, stderr)
	}
	types := map[string]bool{}
	for _, line := range strings.Split(strings.TrimSpace(stdout), "\n") {
		var m map[string]any
		if err := json.Unmarshal([]byte(line), &m); err != nil || m["v"] != 1.0 {
			t.Fatalf("not a protocol line: %q", line)
		}
		types[fmt.Sprint(m["type"])] = true
	}
	if !types["log"] || !types["ok"] {
		t.Fatalf("stdout: %s", stdout)
	}
}

func TestAMissingModelsDirIsAnErrorWithExitCode2(t *testing.T) {
	stdout, _, code := runEngine(t, []string{"SAYSO_MODELS_DIR="}, "")
	if code != 2 || !strings.Contains(stdout, `"message":"SAYSO_MODELS_DIR is not set"`) {
		t.Fatalf("exit %d, stdout %s", code, stdout)
	}
}
