// Package conformance checks that an engine program follows the engine
// protocol v1 (docs/engine-protocol.md). Each check has an id and a level:
//
//   - core: hello, errors, shutdown. Needs no model.
//   - model: the catalog, download, load, transcribe, segments, streams,
//     VAD, unload, delete, on one model. Downloads that model.
//   - store: the model store v1 layout after a download (feature store_v1).
package conformance

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/engine"
)

// Level is a group of checks.
type Level string

const (
	Core  Level = "core"
	Model Level = "model"
	Store Level = "store"
)

// Options say what to check.
type Options struct {
	Program string
	Env     []string
	// ModelsDir is the models folder of the engine; "" is a new temporary
	// folder.
	ModelsDir string
	Levels    []Level
	// Model is the model of the model and store levels; "" is the smallest
	// final model of the catalog.
	Model string
	// Timeout is the time of each request; 0 is 10 minutes.
	Timeout time.Duration
}

// Result is one check.
type Result struct {
	ID     string `json:"id"`
	Level  Level  `json:"level"`
	Pass   bool   `json:"pass"`
	Skip   bool   `json:"skip,omitempty"`
	Detail string `json:"detail,omitempty"`
}

// Report is all checks.
type Report struct {
	Engine  engine.Hello `json:"engine"`
	Results []Result     `json:"results"`
}

// Failed are the checks that did not pass and were not skipped.
func (r Report) Failed() []Result {
	var out []Result
	for _, x := range r.Results {
		if !x.Pass && !x.Skip {
			out = append(out, x)
		}
	}
	return out
}

type checker struct {
	opts   Options
	eng    *engine.Engine
	report *Report
	level  Level
	models string
}

func (c *checker) ctx() (context.Context, context.CancelFunc) {
	t := c.opts.Timeout
	if t == 0 {
		t = 10 * time.Minute
	}
	return context.WithTimeout(context.Background(), t)
}

func (c *checker) check(id string, f func() error) bool {
	err := f()
	r := Result{ID: id, Level: c.level, Pass: err == nil}
	var skip skipped
	if errors.As(err, &skip) {
		r.Pass, r.Skip, r.Detail = false, true, string(skip)
	} else if err != nil {
		r.Detail = err.Error()
	}
	c.report.Results = append(c.report.Results, r)
	return r.Pass
}

type skipped string

func (s skipped) Error() string { return string(s) }

// Check starts the engine and runs the checks of the levels (core when none
// is given).
func Check(ctx context.Context, opts Options) (Report, error) {
	levels := opts.Levels
	if len(levels) == 0 {
		levels = []Level{Core}
	}
	models := opts.ModelsDir
	if models == "" {
		d, err := os.MkdirTemp("", "sayso-engine-check")
		if err != nil {
			return Report{}, err
		}
		defer os.RemoveAll(d)
		models = d
	}
	eng, err := engine.Start(ctx, engine.Options{Program: opts.Program, ModelsDir: models, Env: opts.Env})
	if err != nil {
		return Report{}, err
	}
	defer eng.Close()
	report := &Report{Engine: eng.Hello()}
	c := &checker{opts: opts, eng: eng, report: report, models: models}
	c.level = Core
	c.core()
	var model string
	if slices.Contains(levels, Model) || slices.Contains(levels, Store) {
		c.level = Model
		model = c.model(slices.Contains(levels, Model))
	}
	if slices.Contains(levels, Store) {
		c.level = Store
		c.store(model)
	}
	if slices.Contains(levels, Core) {
		c.level = Core
		c.shutdown()
	}
	return *report, nil
}

func (c *checker) core() {
	h := c.eng.Hello()
	c.check("core.hello.protocol", func() error {
		if h.Protocol != 1 {
			return fmt.Errorf("protocol %d", h.Protocol)
		}
		if h.Version == "" {
			return errors.New("no version")
		}
		return nil
	})
	c.check("core.hello.features", func() error {
		for _, f := range h.Features {
			if !slices.Contains(engine.KnownFeatures, f) {
				return fmt.Errorf("an unknown feature %q", f)
			}
		}
		return nil
	})
	c.check("core.error.unknown_type", func() error {
		ctx, cancel := c.ctx()
		defer cancel()
		_, err := c.eng.Call(ctx, "conformance_unknown", nil, nil)
		var ee *engine.Error
		if !errors.As(err, &ee) || ee.Message == "" {
			return fmt.Errorf("want an error with a message, got %v", err)
		}
		return nil
	})
	c.check("core.error.missing_field", func() error {
		ctx, cancel := c.ctx()
		defer cancel()
		_, err := c.eng.Call(ctx, "load", nil, nil)
		var ee *engine.Error
		if !errors.As(err, &ee) {
			return fmt.Errorf("load without model: want an error, got %v", err)
		}
		return nil
	})
	c.check("core.error.stream_not_open", func() error {
		ctx, cancel := c.ctx()
		defer cancel()
		_, err := c.eng.StreamEnd(ctx, 987654321)
		var ee *engine.Error
		if !errors.As(err, &ee) {
			return fmt.Errorf("stream_end of no session: want an error, got %v", err)
		}
		return nil
	})
}

func (c *checker) shutdown() {
	c.check("core.shutdown", func() error {
		ctx, cancel := c.ctx()
		defer cancel()
		if err := c.eng.Shutdown(ctx); err != nil {
			return err
		}
		select {
		case <-c.eng.Done():
			return nil
		case <-time.After(10 * time.Second):
			return errors.New("the engine did not exit after shutdown")
		}
	})
}

func toneFile(dir string) (string, error) {
	samples := make([]int16, audio.SampleRate/2)
	for i := 0; i < 2*audio.SampleRate; i++ {
		samples = append(samples, int16(8000*math.Sin(2*math.Pi*220*float64(i)/audio.SampleRate)))
	}
	samples = append(samples, make([]int16, audio.SampleRate/2)...)
	path := filepath.Join(dir, "conformance-tone.wav")
	return path, audio.WriteWAV(path, samples)
}

// model runs the model checks, and gives the model that it downloaded.
func (c *checker) model(all bool) string {
	model := c.opts.Model
	var roles []string
	if c.eng.Has("catalog") {
		var list []engine.CatalogModel
		c.check("model.catalog", func() error {
			ctx, cancel := c.ctx()
			defer cancel()
			var err error
			list, _, _, err = c.eng.Catalog(ctx)
			if err != nil {
				return err
			}
			if len(list) == 0 {
				return errors.New("an empty catalog")
			}
			for _, m := range list {
				if m.ID == "" || len(m.Roles) == 0 {
					return fmt.Errorf("a model without id or roles: %+v", m)
				}
			}
			return nil
		})
		best := uint64(math.MaxUint64)
		for _, m := range list {
			if model == "" && slices.Contains(m.Roles, "final") && m.SizeBytes < best {
				best = m.SizeBytes
				model, roles = m.ID, m.Roles
			}
			if m.ID == model && roles == nil {
				roles = m.Roles
			}
		}
	}
	if model == "" {
		c.check("model.pick", func() error { return errors.New("no model: give one, or an engine with the feature catalog") })
		return ""
	}
	if !c.check("model.download", func() error {
		ctx, cancel := c.ctx()
		defer cancel()
		var last string
		err := c.eng.Download(ctx, model, nil, 0, nil, func(s engine.ModelState) { last = s.State })
		if err == nil && last != "downloaded" {
			return fmt.Errorf("the last model_state is %q, not downloaded", last)
		}
		return err
	}) || !all {
		return model
	}
	c.check("model.load", func() error {
		ctx, cancel := c.ctx()
		defer cancel()
		_, err := c.eng.Load(ctx, model, nil)
		return err
	})
	tone, err := toneFile(c.models)
	if err != nil {
		c.check("model.transcribe", func() error { return err })
		return model
	}
	c.check("model.transcribe", func() error {
		ctx, cancel := c.ctx()
		defer cancel()
		_, err := c.eng.Transcribe(ctx, engine.TranscribeRequest{Model: model, Path: tone, Language: "auto"})
		return err
	})
	c.check("model.segments", func() error {
		if !c.eng.Has("segments") {
			return skipped("no feature segments")
		}
		ctx, cancel := c.ctx()
		defer cancel()
		f, err := c.eng.Transcribe(ctx, engine.TranscribeRequest{Model: model, Path: tone, Language: "auto", Segments: true})
		if err != nil {
			return err
		}
		var end uint64
		for _, s := range f.Segments {
			if s.StartMs < end || s.EndMs < s.StartMs || strings.TrimSpace(s.Text) == "" {
				return fmt.Errorf("segments out of order or empty: %+v", f.Segments)
			}
			end = s.EndMs
		}
		return nil
	})
	c.check("model.stream", func() error {
		if !slices.Contains(roles, "streaming") {
			return skipped("the model has no live preview")
		}
		ctx, cancel := c.ctx()
		defer cancel()
		const session = 1 << 40
		if err := c.eng.StreamStart(ctx, session, model, ""); err != nil {
			return err
		}
		a, err := audio.ReadFile(tone)
		if err != nil {
			return err
		}
		for s := a.Samples(); len(s) > 0; s = s[min(2560, len(s)):] {
			if err := c.eng.StreamAudio(session, s[:min(2560, len(s))]); err != nil {
				return err
			}
		}
		_, err = c.eng.StreamEnd(ctx, session)
		return err
	})
	c.check("model.unload", func() error {
		if !c.eng.Has("unload") {
			return skipped("no feature unload")
		}
		ctx, cancel := c.ctx()
		defer cancel()
		return c.eng.Unload(ctx, model, false)
	})
	return model
}

func (c *checker) store(model string) {
	c.check("store.layout", func() error {
		if !c.eng.Has("store_v1") {
			return skipped("no feature store_v1")
		}
		if model == "" {
			return errors.New("no model was downloaded")
		}
		store := filepath.Join(c.models, "v1")
		for _, d := range []string{"artifacts", "models", "locks"} {
			if fi, err := os.Stat(filepath.Join(store, d)); err != nil || !fi.IsDir() {
				return fmt.Errorf("no folder %s in the store", d)
			}
		}
		manifests, _ := filepath.Glob(filepath.Join(store, "models", model, "*.json"))
		if len(manifests) == 0 {
			return fmt.Errorf("no manifest of %s", model)
		}
		b, err := os.ReadFile(manifests[0])
		if err != nil {
			return err
		}
		if !json.Valid(b) {
			return fmt.Errorf("the manifest %s is not JSON", manifests[0])
		}
		artifacts, _ := os.ReadDir(filepath.Join(store, "artifacts"))
		if len(artifacts) == 0 {
			return errors.New("no artifact")
		}
		return nil
	})
	c.check("store.delete", func() error {
		if !c.eng.Has("store_v1") || model == "" {
			return skipped("no store")
		}
		ctx, cancel := c.ctx()
		defer cancel()
		if err := c.eng.Delete(ctx, model, nil); err != nil {
			return err
		}
		manifests, _ := filepath.Glob(filepath.Join(c.models, "v1", "models", model, "*.json"))
		if len(manifests) > 0 {
			return fmt.Errorf("the manifest stays after delete: %v", manifests)
		}
		return nil
	})
}

// LookPath is the program in PATH or the path itself.
func LookPath(program string) (string, error) {
	if strings.ContainsAny(program, `/\`) {
		return program, nil
	}
	return exec.LookPath(program)
}
