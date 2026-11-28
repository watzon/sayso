// Command sayso-testhost is the Host API of Sayso without the app, for the
// integration tests of the SDKs. It serves the real bridge.StartHost on a
// socket or named pipe, with the real identity check and apps.toml in a
// folder of its own, the real engine client over an engine program, and
// microphone and system audio from WAV files.
//
//	sayso-testhost --socket PATH [--engine PROGRAM] [--mic-wav FILE]
//	               [--system-wav FILE] [--approve allow|deny|ignore]
//	               [--download yes|no|wait] [--data DIR] [--engine-env K=V]
//
// It prints {"ready":"<socket>"} on stdout when apps can connect, and
// {"question":"approval"|"download",...} when the host asks the user. Each
// line on stdin is a command:
//
//	{"approve":"allow"}              the answer to approval questions from now on
//	{"download_answer":true}         the answer to the download question that waits
//	{"dictation_done":{"text":...}}  a dictation that ended
//	{"close":true}                   quit like Sayso: host_closing, then exit
//	{"kill":true}                    exit at once, without host_closing
//
// The end of stdin closes the host like {"close":true}.
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/engine/bridgeengine"
)

func main() {
	socket := flag.String("socket", os.Getenv("SAYSO_HOST_SOCKET"), "the socket or pipe to serve")
	enginePath := flag.String("engine", "", "the engine program; none serves no engine")
	micWAV := flag.String("mic-wav", "", "a WAV file that plays as the microphone")
	systemWAV := flag.String("system-wav", "", "a WAV file that plays as the system audio")
	approve := flag.String("approve", "allow", "the answer to approval questions: allow, deny, or ignore")
	download := flag.String("download", "yes", "the answer to download questions: yes, no, or wait")
	dataDir := flag.String("data", "", "the folder of apps.toml, models, and recordings; empty is a new temporary folder")
	speed := flag.Float64("speed", 1, "how much faster than real time the WAV files play")
	cooldown := flag.Duration("cooldown", 200*time.Millisecond, "the wait after a question that got no answer")
	var engineEnv envList
	flag.Var(&engineEnv, "engine-env", "more environment of the engine (K=V); can repeat")
	flag.Parse()
	if *socket == "" {
		log.Fatal("sayso-testhost: no --socket and no SAYSO_HOST_SOCKET")
	}
	if !validAnswer(*approve, "allow", "deny", "ignore") || !validAnswer(*download, "yes", "no", "wait") {
		log.Fatal("sayso-testhost: --approve is allow, deny, or ignore; --download is yes, no, or wait")
	}
	dir := *dataDir
	if dir == "" {
		d, err := os.MkdirTemp("", "sayso-testhost")
		if err != nil {
			log.Fatal(err)
		}
		defer os.RemoveAll(d)
		dir = d
	}
	out := newOutput(os.Stdout)
	ctl := newControl(out, *approve, *download)
	svc, err := newServices(dir, ctl)
	if err != nil {
		log.Fatal(err)
	}
	if *enginePath != "" {
		models, cache := filepath.Join(dir, "models"), filepath.Join(dir, "cache")
		if err := os.MkdirAll(cache, 0o700); err != nil {
			log.Fatal(err)
		}
		eng, err := bridgeengine.Start(engine.Options{EnginePath: *enginePath, ModelsDir: models, StoreDir: filepath.Join(models, "v1"), CacheDir: cache, Env: engineEnv}, nil, nil)
		if err != nil {
			log.Fatalf("sayso-testhost: engine: %v", err)
		}
		defer eng.Close()
		svc.eng = eng
	}
	if *micWAV != "" {
		svc.mic = &wavSource{path: *micWAV, speed: *speed}
	}
	if *systemWAV != "" {
		svc.system = &wavSource{path: *systemWAV, speed: *speed}
	}
	cfg := bridge.NewHostConfig(*socket, filepath.Join(dir, "apps.toml"))
	cfg.ApprovalCooldown = *cooldown
	host, err := bridge.StartHost(cfg, svc)
	if err != nil {
		log.Fatalf("sayso-testhost: %v", err)
	}
	out.write(map[string]any{"ready": *socket})
	os.Exit(ctl.run(os.Stdin, host))
}

type envList []string

func (e *envList) String() string     { return strings.Join(*e, " ") }
func (e *envList) Set(v string) error { *e = append(*e, v); return nil }

func validAnswer(v string, allowed ...string) bool {
	for _, a := range allowed {
		if v == a {
			return true
		}
	}
	return false
}

type command struct {
	Approve        *string        `json:"approve"`
	DownloadAnswer *bool          `json:"download_answer"`
	DictationDone  *dictationDone `json:"dictation_done"`
	Close          bool           `json:"close"`
	Kill           bool           `json:"kill"`
}

type dictationDone struct {
	Text       string  `json:"text"`
	Transcript string  `json:"transcript"`
	App        *string `json:"app"`
	Site       *string `json:"site"`
	Style      *string `json:"style"`
	Time       string  `json:"time"`
	Incognito  bool    `json:"incognito"`
}

// run reads the commands on stdin until close, kill, or the end. It gives
// the exit code.
func (c *control) run(stdin *os.File, host *bridge.Host) int {
	s := bufio.NewScanner(stdin)
	s.Buffer(make([]byte, 64*1024), 1<<20)
	for s.Scan() {
		line := strings.TrimSpace(s.Text())
		if line == "" {
			continue
		}
		var cmd command
		if err := json.Unmarshal([]byte(line), &cmd); err != nil {
			fmt.Fprintf(os.Stderr, "sayso-testhost: bad command %q: %v\n", line, err)
			continue
		}
		switch {
		case cmd.Kill:
			os.Exit(3)
		case cmd.Close:
			host.Close()
			return 0
		case cmd.Approve != nil:
			if !validAnswer(*cmd.Approve, "allow", "deny", "ignore") {
				fmt.Fprintf(os.Stderr, "sayso-testhost: approve is allow, deny, or ignore, not %q\n", *cmd.Approve)
				continue
			}
			c.setApprove(*cmd.Approve)
		case cmd.DownloadAnswer != nil:
			c.answerDownload(*cmd.DownloadAnswer)
		case cmd.DictationDone != nil:
			d := cmd.DictationDone
			when := d.Time
			if when == "" {
				when = time.Now().UTC().Format(time.RFC3339)
			}
			host.DictationDone(bridge.DictationDone{Text: d.Text, Transcript: d.Transcript, App: d.App, Site: d.Site, Style: d.Style, Time: when}, d.Incognito, false)
			c.out.write(map[string]any{"sent": "dictation_done"})
		}
	}
	host.Close()
	return 0
}
