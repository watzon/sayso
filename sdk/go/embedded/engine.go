package embedded

import (
	"context"
	"errors"
	"slices"
	"sync"

	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/speech"
)

// ModelInfo is a model of the catalog of the engine with its state.
type ModelInfo = protocol.ModelInfo

// Models is what an app does with the models of its engine.
type Models interface {
	Catalog() ([]ModelInfo, error)
	// Download downloads a model at once; onProgress runs on this goroutine.
	Download(model string, sizeBytes uint64, onProgress func(engine.DownloadProgress)) error
	Delete(model string) error
	Load(model string) error
	Unload(model string) error
}

// ProgramEngine is the engine program of the app. It starts at the first
// request that needs it, and again after it stopped.
type ProgramEngine struct {
	opts engine.Options

	mu  sync.Mutex
	eng *engine.Engine
	ad  *speech.EngineAdapter

	subMu sync.Mutex
	subs  map[int]chan speech.EngineEvent
	next  int
}

var (
	_ speech.Engine = (*ProgramEngine)(nil)
	_ Models        = (*ProgramEngine)(nil)
)

// NewProgramEngine runs the engine program with opts (Program, ModelsDir,
// StoreDir, Env).
func NewProgramEngine(opts engine.Options) *ProgramEngine {
	return &ProgramEngine{opts: opts, subs: map[int]chan speech.EngineEvent{}}
}

// Stop ends the engine; the next request starts it again.
func (p *ProgramEngine) Stop() {
	p.mu.Lock()
	eng := p.eng
	p.eng, p.ad = nil, nil
	p.mu.Unlock()
	if eng != nil {
		eng.Close()
	}
}

// IsRunning is true while the engine runs.
func (p *ProgramEngine) IsRunning() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.eng == nil {
		return false
	}
	select {
	case <-p.eng.Done():
		return false
	default:
		return true
	}
}

func (p *ProgramEngine) running() (*engine.Engine, *speech.EngineAdapter, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.eng != nil {
		select {
		case <-p.eng.Done():
			p.eng, p.ad = nil, nil
		default:
			return p.eng, p.ad, nil
		}
	}
	eng, err := engine.Start(context.Background(), p.opts)
	if err != nil {
		return nil, nil, err
	}
	ad := speech.FromEngine(eng, "")
	events, _ := ad.Subscribe()
	go func() {
		for ev := range events {
			p.subMu.Lock()
			for _, ch := range p.subs {
				select {
				case ch <- ev:
				default:
				}
			}
			p.subMu.Unlock()
		}
	}()
	p.eng, p.ad = eng, ad
	return eng, ad, nil
}

func (p *ProgramEngine) adapter() (*speech.EngineAdapter, error) {
	_, ad, err := p.running()
	return ad, err
}

func engineErr(err error) error {
	var ee *engine.Error
	if errors.As(err, &ee) {
		return &speech.EngineError{Code: protocol.ErrorCode(ee.Code), Message: ee.Message}
	}
	return err
}

func (p *ProgramEngine) Catalog() ([]ModelInfo, error) {
	eng, ad, err := p.running()
	if err != nil {
		return nil, err
	}
	list, _, states, err := eng.Catalog(context.Background())
	if err != nil {
		return nil, engineErr(err)
	}
	state := map[string]string{}
	for _, s := range states {
		state[s.Model] = s.State
		ad.SetStatus(s.Model, speech.ModelStatus{State: s.State, Message: s.Message, Reason: s.Reason})
	}
	out := make([]ModelInfo, 0, len(list))
	for _, m := range list {
		st := protocol.ModelState(state[m.ID])
		if st == "" {
			st = protocol.StateNotDownloaded
		}
		roles := make([]protocol.Role, len(m.Roles))
		for i, r := range m.Roles {
			roles[i] = protocol.Role(r)
		}
		langs := m.Languages
		if langs == nil {
			langs = []string{}
		}
		out = append(out, ModelInfo{ID: m.ID, Name: m.Name, Roles: roles, State: st, Languages: langs, Timestamps: protocol.TimestampLevel(m.Timestamps), SizeBytes: m.SizeBytes})
	}
	return out, nil
}

func (p *ProgramEngine) Download(model string, sizeBytes uint64, onProgress func(engine.DownloadProgress)) error {
	eng, _, err := p.running()
	if err != nil {
		return err
	}
	return engineErr(eng.Download(context.Background(), model, nil, sizeBytes, onProgress, nil))
}

func (p *ProgramEngine) Delete(model string) error {
	eng, _, err := p.running()
	if err != nil {
		return err
	}
	return engineErr(eng.Delete(context.Background(), model, nil))
}

func (p *ProgramEngine) Unload(model string) error {
	eng, ad, err := p.running()
	if err != nil {
		return err
	}
	if !slices.Contains(eng.Hello().Features, "unload") {
		return nil
	}
	if err := eng.Unload(context.Background(), model, false); err != nil {
		return engineErr(err)
	}
	ad.SetStatus(model, speech.ModelStatus{State: speech.StateDownloaded})
	return nil
}

func (p *ProgramEngine) Features() []string {
	eng, _, err := p.running()
	if err != nil {
		return nil
	}
	return eng.Hello().Features
}

func (p *ProgramEngine) Status(model string) speech.ModelStatus {
	p.mu.Lock()
	ad := p.ad
	p.mu.Unlock()
	if ad == nil {
		return speech.ModelStatus{}
	}
	return ad.Status(model)
}

func (p *ProgramEngine) Load(model string) error {
	ad, err := p.adapter()
	if err != nil {
		return err
	}
	return ad.Load(model)
}

func (p *ProgramEngine) Transcribe(samples []float32, model, language string) (string, error) {
	ad, err := p.adapter()
	if err != nil {
		return "", err
	}
	return ad.Transcribe(samples, model, language)
}

func (p *ProgramEngine) TranscribeSegments(samples []float32, model, language string) ([]speech.Segment, error) {
	ad, err := p.adapter()
	if err != nil {
		return nil, err
	}
	return ad.TranscribeSegments(samples, model, language)
}

func (p *ProgramEngine) DetectSpeech(model string, samples []float32) ([]speech.SpeechPart, error) {
	ad, err := p.adapter()
	if err != nil {
		return nil, err
	}
	return ad.DetectSpeech(model, samples)
}

func (p *ProgramEngine) Diarize(model string, samples []float32, speakers *uint32) ([]speech.RawTurn, error) {
	ad, err := p.adapter()
	if err != nil {
		return nil, err
	}
	return ad.Diarize(model, samples, speakers)
}

func (p *ProgramEngine) StartStream(session uint64, model, language string) error {
	ad, err := p.adapter()
	if err != nil {
		return err
	}
	return ad.StartStream(session, model, language)
}

func (p *ProgramEngine) StartVad(session uint64, model string) error {
	ad, err := p.adapter()
	if err != nil {
		return err
	}
	return ad.StartVad(session, model)
}

func (p *ProgramEngine) PushAudio(session uint64, samples []float32) error {
	ad, err := p.adapter()
	if err != nil {
		return err
	}
	return ad.PushAudio(session, samples)
}

func (p *ProgramEngine) EndStream(session uint64) {
	if ad, err := p.adapter(); err == nil {
		ad.EndStream(session)
	}
}

func (p *ProgramEngine) EndVad(session uint64) error {
	ad, err := p.adapter()
	if err != nil {
		return err
	}
	return ad.EndVad(session)
}

// Subscribe gives the events of each engine that runs from now on.
func (p *ProgramEngine) Subscribe() (<-chan speech.EngineEvent, func()) {
	ch := make(chan speech.EngineEvent, 4096)
	p.subMu.Lock()
	id := p.next
	p.next++
	p.subs[id] = ch
	p.subMu.Unlock()
	var once sync.Once
	return ch, func() {
		once.Do(func() {
			p.subMu.Lock()
			delete(p.subs, id)
			p.subMu.Unlock()
			close(ch)
		})
	}
}

// FromProgram is the engine program at program, with the shared model store
// of this user, and modelsDir for an engine without the store.
func FromProgram(program, modelsDir string) *ProgramEngine {
	return NewProgramEngine(engine.Options{Program: program, ModelsDir: modelsDir, ModelStore: protocol.ModelStoreDir(protocol.ModelsDir(protocol.DataDir()))})
}
