package main

import (
	"encoding/json"
	"io"
	"path/filepath"
	"sync"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/store"
)

// output writes one JSON object for each line on stdout.
type output struct {
	mu sync.Mutex
	w  io.Writer
}

func newOutput(w io.Writer) *output { return &output{w: w} }

func (o *output) write(v any) {
	b, _ := json.Marshal(v)
	o.mu.Lock()
	o.w.Write(append(b, '\n'))
	o.mu.Unlock()
}

// control holds the answers of the user, which the tests set on stdin.
type control struct {
	out *output

	mu       sync.Mutex
	changed  *sync.Cond
	approve  string
	download string
	answers  []bool
}

func newControl(out *output, approve, download string) *control {
	c := &control{out: out, approve: approve, download: download}
	c.changed = sync.NewCond(&c.mu)
	go func() {
		for range time.Tick(50 * time.Millisecond) {
			c.changed.Broadcast()
		}
	}()
	return c
}

func (c *control) setApprove(v string) {
	c.mu.Lock()
	c.approve = v
	c.mu.Unlock()
	c.changed.Broadcast()
}

func (c *control) answerDownload(yes bool) {
	c.mu.Lock()
	c.answers = append(c.answers, yes)
	c.mu.Unlock()
	c.changed.Broadcast()
}

func (c *control) askApproval(q bridge.ApprovalQuestion, withdrawn func() bool) (bool, bool) {
	caps := make([]string, len(q.Capabilities))
	for i, cap := range q.Capabilities {
		caps[i] = string(cap)
	}
	c.out.write(map[string]any{"question": "approval", "app": q.Name, "claimed": q.ClaimedName, "capabilities": caps, "approved": q.Approved})
	c.mu.Lock()
	defer c.mu.Unlock()
	for {
		switch c.approve {
		case "allow":
			return true, true
		case "deny":
			return false, true
		}
		if withdrawn() {
			return false, false
		}
		c.changed.Wait()
	}
}

func (c *control) askDownload(q bridge.DownloadQuestion, withdrawn func() bool) bool {
	c.out.write(map[string]any{"question": "download", "app": q.AppName, "model": q.ModelID, "reason": q.Reason, "size_bytes": q.SizeBytes})
	c.mu.Lock()
	defer c.mu.Unlock()
	for {
		switch {
		case c.download == "yes":
			return true
		case c.download == "no":
			return false
		case len(c.answers) > 0:
			yes := c.answers[0]
			c.answers = c.answers[1:]
			return yes
		}
		if withdrawn() {
			return false
		}
		c.changed.Wait()
	}
}

// providerID is the AI provider of the test host: it runs no model, and its
// answer is the text with the id of the style in front.
const providerID = "testhost"

type services struct {
	bridge.BaseServices
	ctl    *control
	cfg    *config.Config
	styles *enhance.StyleLibrary
	dir    string
	eng    bridge.Engine
	mic    bridge.Microphone
	system *wavSource
}

func newServices(dir string, ctl *control) (*services, error) {
	cfg := config.Default()
	for _, m := range models.LocalModels() {
		if m.FinalPass && m.Task() == models.TaskSpeech {
			cfg.Dictation.Model = m.ID
			break
		}
	}
	cfg.Dictation.Language = "en"
	cfg.Dictation.PreloadModel = false
	model := "echo"
	cfg.AI.Enabled = true
	cfg.AI.Providers = []config.Provider{{ID: providerID, Name: "Test host", Kind: config.KindOpenAICompatible, BaseURL: "http://127.0.0.1:9", Model: &model}}
	id := providerID
	cfg.AI.DefaultProvider = &id
	return &services{ctl: ctl, cfg: &cfg, styles: enhance.LoadStyles(filepath.Join(dir, "styles")), dir: dir}, nil
}

func (s *services) Engine() bridge.Engine {
	if s.eng == nil {
		return nil
	}
	return s.eng
}

func (s *services) Config() *config.Config { return s.cfg }

func (s *services) AskDownload(q bridge.DownloadQuestion, withdrawn func() bool) bool {
	return s.ctl.askDownload(q, withdrawn)
}

func (s *services) AskApproval(q bridge.ApprovalQuestion, withdrawn func() bool) (bool, bool) {
	return s.ctl.askApproval(q, withdrawn)
}

func (s *services) Styles() *enhance.StyleLibrary { return s.styles }

func (s *services) RunStyle(r bridge.StyleRequest) enhance.PipelineOutput {
	return enhance.PipelineOutput{Text: "[" + r.Style.ID + "] " + r.Text, Enhance: store.EnhanceOutcome{Result: store.EnhanceApplied, ProviderID: r.Provider.ID, Model: "echo"}}
}

func (s *services) Microphone() bridge.Microphone {
	if s.mic == nil {
		return nil
	}
	return s.mic
}

func (s *services) SystemAudio() bridge.SystemAudio {
	if s.system == nil {
		return nil
	}
	return systemAudio{s.system}
}

func (s *services) LiveTempDir() string { return filepath.Join(s.dir, "live") }
