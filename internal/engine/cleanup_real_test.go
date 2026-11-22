package engine

import (
	"bufio"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/llamalib"
)

// The real cleanup runtime (cmd/sayso-cleanup) with the real models. It runs
// only when SAYSO_CLEANUP_REAL_MODEL_DIR names a folder with
// s1-mini-q4_k_m.gguf and cleanup-1-q8_0.gguf:
//
//	SAYSO_CLEANUP_REAL_MODEL_DIR=<folder> go test -run Real -v ./internal/engine
//
// The llama.cpp libraries come from SAYSO_LLAMA_LIB_DIR, else from
// build/llama-libs (downloaded once and checked against the pins).
//
// On Apple silicon (Metal) the answers must be the bytes that the Rust runtime
// (native/cleanup, llama-cpp-2 0.1.160) gave for the same prompts. On the CPU
// the order of float sums depends on the thread count, so there the test only
// checks for a whole, clean answer.

const realS1System = "You are a text normalizer for speech-to-text transcripts. The input begins with a control line " +
	"specifying the styling, structure, and context settings; clean the transcript to match those settings and output only " +
	"the cleaned text."

func realS1Prompt(text string) string {
	return "<|im_start|>system\n" + realS1System + "<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: prose] [Context: general]\n" +
		text + "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
}

var realShort = "um so I think we should uh move the meeting to Thursday no wait Friday afternoon because uh Sarah is out on Thursday you know"
var realMedium = "okay so um the thing I wanted to talk about is the uh the release plan for next week so basically we were gonna ship on Tuesday " +
	"but actually no let's say Wednesday because the QA team needs one more day to finish testing the the Windows build and uh I also " +
	"want to make sure that the new onboarding flow is is working properly before we push it out to everyone so like the plan is we " +
	"freeze the branch Monday night uh run the full regression suite Tuesday and then if everything looks good we tag the release " +
	"Wednesday morning around ten no actually eleven since the standup runs long and then we'll send the announcement email to the " +
	"beta list that afternoon"

type realCase struct {
	name, model, prompt string
	end                 []string
	max                 uint32
	want                string // the answer of the Rust runtime
}

var realCases = []realCase{
	{"s1 short", "s1", realS1Prompt(realShort), []string{"<|im_end|>"}, 96,
		"So I think we should move the meeting to Friday afternoon, because Sarah is out on Thursday."},
	{"s1 medium", "s1", realS1Prompt(realMedium), []string{"<|im_end|>"}, 256,
		"Okay, so the thing I wanted to talk about is the release plan for next week. So basically we were going to ship on Wednesday, because the QA team needs one more day to finish testing the Windows build. And I also want to make sure that the new onboarding flow is working properly before we push it out to everyone.\n\nSo the plan is we freeze the branch Monday night, run the full regression suite Tuesday, and then if everything looks good we tag the release Wednesday morning around 11, since the standup runs long. And then we'll send the announcement email to the beta list that afternoon."},
	{"cleanup-1 short", "kv", "tone: formal\ntext: " + realShort + "\nout:", []string{"\ntext:", "\nout:"}, 96,
		" I think we should move the meeting to Friday afternoon because Sarah is out on Thursday."},
	{"cleanup-1 medium", "kv", "tone: formal\ntext: " + realMedium + "\nout:", []string{"\ntext:", "\nout:"}, 256,
		" The thing I wanted to talk about is the release plan for next week. We were gonna ship on Tuesday, but actually no, let's say Wednesday because the QA team needs one more day to finish testing the Windows build, and I also want to make sure that the new onboarding flow is working properly before we push it out to everyone. The plan is we freeze the branch Monday night, run the full regression suite Tuesday, and then if everything looks good we tag the release Wednesday morning around 11, since the standup runs long, and then we'll send the announcement email to the beta list that afternoon."},
}

// realRuntime builds cmd/sayso-cleanup and points it at the libraries.
func realRuntime(t *testing.T) (exe string, models map[string]string) {
	t.Helper()
	dir := os.Getenv("SAYSO_CLEANUP_REAL_MODEL_DIR")
	if dir == "" {
		t.Skip("set SAYSO_CLEANUP_REAL_MODEL_DIR to run the real cleanup runtime")
	}
	models = map[string]string{"s1": filepath.Join(dir, "s1-mini-q4_k_m.gguf"), "kv": filepath.Join(dir, "cleanup-1-q8_0.gguf")}
	if os.Getenv(llamalib.EnvLibDir) == "" {
		a, ok := llamalib.Current()
		if !ok {
			t.Skip("no llama.cpp build for this system")
		}
		root, _ := filepath.Abs(filepath.Join("..", "..", "build", "llama-libs"))
		libs := filepath.Join(root, a.Name())
		if err := llamalib.Fetch(context.Background(), nil, a, libs, filepath.Join(root, "archives")); err != nil {
			t.Fatal(err)
		}
		t.Setenv(llamalib.EnvLibDir, libs)
	}
	exe = filepath.Join(t.TempDir(), "sayso-cleanup")
	if runtime.GOOS == "windows" {
		exe += ".exe"
	}
	build := exec.Command(filepath.Join(runtime.GOROOT(), "bin", "go"), "build", "-o", exe, "github.com/watzon/sayso/cmd/sayso-cleanup")
	build.Env = append(os.Environ(), "CGO_ENABLED=0")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	return exe, models
}

func checkAnswer(t *testing.T, c realCase, got Completion) {
	t.Helper()
	if got.Stop != StopEnd {
		t.Errorf("%s: stop %q", c.name, got.Stop)
	}
	metal := runtime.GOOS == "darwin" && runtime.GOARCH == "arm64"
	if metal && got.Text != c.want {
		t.Errorf("%s: not the bytes of the Rust runtime:\n got %q\nwant %q", c.name, got.Text, c.want)
	}
	for _, bad := range []string{"<|im_end|>", "<|im_start|>", "<think>", "\ntext:", "\nout:"} {
		if strings.Contains(got.Text, bad) {
			t.Errorf("%s: control text %q in %q", c.name, bad, got.Text)
		}
	}
	if !metal && len(strings.TrimSpace(got.Text)) < len(strings.TrimSpace(c.want))/2 {
		t.Errorf("%s: a short answer %q", c.name, got.Text)
	}
}

func TestRealCleanupRuntimeMatchesTheRustRuntime(t *testing.T) {
	exe, models := realRuntime(t)
	c := NewCleanupClient(exe)
	defer c.Stop()
	c.idleLimit = 2 * time.Second
	run := func(cases []realCase) {
		for _, rc := range cases {
			err := c.With(rc.model, models[rc.model], 2048, func(s *CleanupSession) error {
				started := time.Now()
				got, err := s.Complete(rc.prompt, rc.end, rc.max, 30*time.Second)
				if err != nil {
					return err
				}
				t.Logf("%s: %d ms, %d tokens in, %d out", rc.name, time.Since(started).Milliseconds(), got.TokensIn, got.TokensOut)
				checkAnswer(t, rc, got)
				return nil
			})
			if err != nil {
				t.Fatalf("%s: %v", rc.name, err)
			}
		}
	}
	run(realCases)
	if h, ok := c.Hello(); !ok || h.Libraries != "found" || !strings.Contains(h.LlamaCpp, llamalib.Upstream) {
		t.Fatalf("hello %+v", h)
	}
	// Idle: the runtime stops (gracefully) and the next request starts and
	// loads again, with the same answer.
	deadline := time.Now().Add(15 * time.Second)
	for c.IsRunning() && time.Now().Before(deadline) {
		time.Sleep(100 * time.Millisecond)
	}
	if c.IsRunning() {
		t.Fatal("the runtime did not stop when idle")
	}
	run(realCases[:1])
	if !c.IsRunning() {
		t.Fatal("the runtime did not start again")
	}
}

// After a cleanup, the end of stdin frees the context, the model, and the
// backend, and the process exits with 0 and no ggml assert (ggml-metal asserts
// at exit when the backend still holds resources).
func TestRealCleanupRuntimeExitsCleanly(t *testing.T) {
	exe, models := realRuntime(t)
	for _, unloadFirst := range []bool{false, true} {
		cmd := exec.Command(exe)
		cmd.Env = append(os.Environ(), "SAYSO_CLEANUP_LOG=debug")
		stdin, _ := cmd.StdinPipe()
		stdout, _ := cmd.StdoutPipe()
		var stderr strings.Builder
		cmd.Stderr = &stderr
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		lines := bufio.NewScanner(stdout)
		lines.Buffer(make([]byte, 1<<20), 1<<20)
		send := func(m map[string]any) map[string]any {
			m["v"] = 1
			b, _ := json.Marshal(m)
			stdin.Write(append(b, '\n'))
			if !lines.Scan() {
				t.Fatalf("no reply; stderr:\n%s", stderr.String())
			}
			var r map[string]any
			json.Unmarshal(lines.Bytes(), &r)
			return r
		}
		if r := send(map[string]any{"id": "1", "type": "load", "model_id": "s1", "path": models["s1"], "n_ctx": 2048}); r["type"] != "loaded" {
			t.Fatal(r)
		}
		rc := realCases[0]
		if r := send(map[string]any{"id": "2", "type": "complete", "model_id": "s1", "prompt": rc.prompt, "end": rc.end, "max_tokens": rc.max}); r["stop"] != "end" {
			t.Fatal(r)
		}
		if unloadFirst {
			if r := send(map[string]any{"id": "3", "type": "unload"}); r["type"] != "unloaded" {
				t.Fatal(r)
			}
		}
		stdin.Close()
		err := cmd.Wait()
		log := stderr.String()
		if err != nil || strings.Contains(log, "GGML_ASSERT") || strings.Contains(strings.ToLower(log), "assertion") {
			t.Fatalf("unload first %v: exit %v\n%s", unloadFirst, err, log)
		}
	}
}
