// Command sayso-fake-engine is a deterministic engine of the Sayso engine
// protocol v1 (docs/engine-protocol.md), for the tests of the SDKs and of
// the host. It speaks every feature, writes real model store v1 entries, and
// transcribes by the level of the audio, so the same audio always gives the
// same words.
//
// Its catalog has fake-final (final), fake-stream (final and streaming),
// fake-vad, fake-diar, and the retired fake-old. A request with an engine
// object runs any id. The ids fake-crash and fake-fail crash or fail at load.
//
// SAYSO_FAKE_SCRIPT is a JSON object that injects faults:
//
//	{"stderr": ["line", ...],         lines on stderr at the start
//	 "exit_at_start": 3,              exit with this code before reading
//	 "crash_at": 4,                   exit 1 when the 4th request comes
//	 "delay_ms": {"transcribe": 300}, wait before the reply of a type
//	 "fail": {"load": {"message": "...", "code": "...", "reason": "..."}},
//	 "features": [],                  the features of hello
//	 "version": "0.1.0",
//	 "max_streams": 4,
//	 "accept_any": true}                any id without an engine object runs,
//	                                    with roles from its name (vad,
//	                                    diar), as a stand-in for a real engine
//
// SAYSO_FAKE_TRANSCRIPTS is a JSON file {"<sha256 of the WAV file>": "text"}
// with fixed transcripts.
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
)

func main() {
	script, err := loadScript(os.Getenv("SAYSO_FAKE_SCRIPT"))
	if err != nil {
		fmt.Fprintln(os.Stderr, "sayso-fake-engine:", err)
		os.Exit(2)
	}
	for _, line := range script.Stderr {
		fmt.Fprintln(os.Stderr, line)
	}
	if script.ExitAtStart != 0 {
		os.Exit(script.ExitAtStart)
	}
	e, err := newEngine(script, os.Getenv("SAYSO_MODELS_DIR"), os.Getenv("SAYSO_MODEL_STORE"), os.Getenv("SAYSO_FAKE_TRANSCRIPTS"), os.Stdout)
	if err != nil {
		fmt.Fprintln(os.Stderr, "sayso-fake-engine:", err)
		os.Exit(2)
	}
	os.Exit(e.serve(os.Stdin))
}

type failure struct {
	Message string `json:"message"`
	Code    string `json:"code,omitempty"`
	Reason  string `json:"reason,omitempty"`
}

type script struct {
	Stderr      []string           `json:"stderr"`
	ExitAtStart int                `json:"exit_at_start"`
	CrashAt     int                `json:"crash_at"`
	DelayMs     map[string]int     `json:"delay_ms"`
	Fail        map[string]failure `json:"fail"`
	Features    *[]string          `json:"features"`
	Version     string             `json:"version"`
	MaxStreams  int                `json:"max_streams"`
	AcceptAny   bool               `json:"accept_any"`
}

func loadScript(text string) (script, error) {
	var s script
	if text == "" {
		return s, nil
	}
	if err := json.Unmarshal([]byte(text), &s); err != nil {
		return s, fmt.Errorf("SAYSO_FAKE_SCRIPT: %v", err)
	}
	return s, nil
}

// serve reads requests until stdin closes or shutdown. It returns the exit
// code.
func (e *engine) serve(in io.Reader) int {
	r := bufio.NewReaderSize(in, 1<<20)
	for {
		line, err := r.ReadBytes('\n')
		if len(line) > 0 {
			if done, code := e.handleLine(line); done {
				return code
			}
		}
		if err != nil {
			return 0
		}
	}
}
