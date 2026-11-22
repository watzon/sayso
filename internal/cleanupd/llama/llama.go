// Package llama runs cleanup models with llama.cpp through yzma (purego and
// libffi, no cgo): the Engine and Model of internal/cleanupd.
//
// Only cmd/sayso-cleanup may import this package. yzma's libffi binding loads
// libffi in a package init and panics when that fails, so a process that
// imports it cannot start without libffi (cmd/sayso has a test for this).
package llama

import (
	"errors"
	"fmt"
	"log/slog"
	"runtime"
	"sync"

	"github.com/hybridgroup/yzma/pkg/llama"

	"github.com/watzon/sayso/internal/cleanupd"
	"github.com/watzon/sayso/internal/llamalib"
)

// Engine is the llama.cpp backend. It loads the libraries and starts the
// backend at the first Open.
type Engine struct {
	// LibDir finds the folder with the libraries, at the first Open.
	LibDir func() (string, error)
	// Found is true when the libraries are there without a download.
	Found func() bool
	// Logs keeps the log output of llama.cpp and ggml (on stderr).
	Logs bool

	started bool
	libErr  error
}

// Backend is "metal" on Apple silicon, else "cpu".
func (e *Engine) Backend() string { return backend() }

func backend() string {
	if runtime.GOOS == "darwin" && runtime.GOARCH == "arm64" {
		return "metal"
	}
	return "cpu"
}

// HasLibraries is true when llama.cpp runs or its libraries are there.
func (e *Engine) HasLibraries() bool { return e.started || (e.Found != nil && e.Found()) }

// Version names the llama.cpp release of the pinned libraries.
func (e *Engine) Version() string { return llamalib.Tag + " (" + llamalib.Upstream + ")" }

// The libraries load once per process; a failure stays.
var loadOnce sync.Once

func (e *Engine) start() error {
	if e.started {
		return nil
	}
	loadOnce.Do(func() {
		dir, err := e.LibDir()
		if err != nil {
			e.libErr = err
			return
		}
		if err := llama.Load(dir); err != nil {
			e.libErr = fmt.Errorf("cannot load llama.cpp from %s: %w", dir, err)
			return
		}
		// Before the backend starts: the Metal setup logs about 30 lines.
		if !e.Logs {
			llama.LogSet(llama.LogSilent())
		}
		// The ggml backends (Metal, the CPU builds) come from this folder
		// only. llama_backend_init loads every backend it finds when none is
		// loaded yet, also in the folder of the program and the working
		// folder, where it can find the ggml of the speech engine. So the
		// backends load first, from dir (not llama.Init, which loads them
		// after the init).
		if err := llama.GGMLBackendLoadAllFromPath(dir); err != nil {
			e.libErr = fmt.Errorf("cannot load the ggml backends from %s: %w", dir, err)
			return
		}
		llama.BackendInit()
		slog.Info("llama.cpp started", "dir", dir, "backend", backend())
		e.started = true
	})
	if e.libErr != nil {
		return e.libErr
	}
	if !e.started {
		return errors.New("llama.cpp was stopped")
	}
	return nil
}

// Close frees the backend. A second call does nothing.
func (e *Engine) Close() {
	if e.started {
		llama.BackendFree()
		e.started = false
	}
}

// Open loads the model file with one context of nCtx tokens. One decode takes
// the whole prompt (Complete checks that it fits), so the batch sizes are nCtx.
func (e *Engine) Open(path string, nCtx uint32) (cleanupd.Model, error) {
	if err := e.start(); err != nil {
		return nil, err
	}
	mp := llama.ModelDefaultParams()
	mp.NGpuLayers = 0
	if backend() == "metal" {
		mp.NGpuLayers = 99
	}
	model, err := llama.ModelLoadFromFile(path, mp)
	if err != nil || model == 0 {
		return nil, fmt.Errorf("cannot load %s: %v", path, err)
	}
	cp := llama.ContextDefaultParams()
	cp.NCtx, cp.NBatch, cp.NUbatch = nCtx, nCtx, nCtx
	if backend() == "cpu" {
		// The cores, at most 8, for both the prompt and the answer.
		threads := int32(min(runtime.NumCPU(), 8))
		cp.NThreads, cp.NThreadsBatch = threads, threads
	}
	ctx, err := llama.InitFromModel(model, cp)
	if err != nil || ctx == 0 {
		llama.ModelFree(model)
		return nil, fmt.Errorf("cannot create a context of %d tokens: %v", nCtx, err)
	}
	mem, err := llama.GetMemory(ctx)
	if err != nil {
		llama.Free(ctx)
		llama.ModelFree(model)
		return nil, fmt.Errorf("cannot get the memory of the context: %w", err)
	}
	return &Model{model: model, ctx: ctx, vocab: llama.ModelGetVocab(model), mem: mem}, nil
}

// Model is a loaded model with its one context.
type Model struct {
	model llama.Model
	ctx   llama.Context
	vocab llama.Vocab
	mem   llama.Memory
}

// Close frees the context, then the model.
func (m *Model) Close() {
	if m.ctx != 0 {
		llama.Free(m.ctx)
		m.ctx = 0
	}
	if m.model != 0 {
		llama.ModelFree(m.model)
		m.model = 0
	}
}

// Tokens counts text with add_special false and parse_special true.
func (m *Model) Tokens(text string) (int, error) {
	return len(llama.Tokenize(m.vocab, text, false, true)), nil
}

// piece is the bytes of one token (special tokens not rendered).
func (m *Model) piece(tok llama.Token, buf []byte) []byte {
	n := llama.TokenToPiece(m.vocab, tok, buf, 0, false)
	if n < 0 {
		buf = make([]byte, -n)
		n = llama.TokenToPiece(m.vocab, tok, buf, 0, false)
	}
	return buf[:max(n, 0)]
}

// Complete is greedy decoding, line for line the complete() of
// native/cleanup/src/runtime.rs.
func (m *Model) Complete(prompt string, end []string, maxTokens, nCtx uint32) (cleanupd.Completion, error) {
	toks := llama.Tokenize(m.vocab, prompt, llama.VocabGetAddBOS(m.vocab), true)
	c := cleanupd.Completion{TokensIn: uint32(len(toks))}
	if len(toks) == 0 {
		return c, &cleanupd.Error{Code: cleanupd.BadRequest, Message: "the prompt has no tokens"}
	}
	// The whole answer must fit. The caller sizes maxTokens, so this is a
	// "context" stop, not an error.
	if uint64(len(toks))+uint64(maxTokens) > uint64(nCtx) {
		c.Stop = cleanupd.StopContext
		return c, nil
	}
	if maxTokens == 0 {
		c.Stop = cleanupd.StopLimit
		return c, nil
	}
	longest := cleanupd.LongestEnd(end)
	// A new prompt starts from an empty cache. The context stays.
	if err := llama.MemoryClear(m.mem, true); err != nil {
		return c, decodeFailed("cannot clear the cache: %v", err)
	}
	batch := llama.BatchInit(int32(len(toks)), 0, 1)
	defer llama.BatchFree(batch)
	for i, t := range toks {
		if err := batch.Add(t, llama.Pos(i), []llama.SeqId{0}, i == len(toks)-1); err != nil {
			return c, decodeFailed("cannot build the batch: %v", err)
		}
	}
	if rc, err := llama.Decode(m.ctx, batch); err != nil || rc != 0 {
		return c, decodeFailed("decode of the prompt failed: %d %v", rc, err)
	}
	sampler := llama.SamplerChainInit(llama.SamplerChainDefaultParams())
	llama.SamplerChainAdd(sampler, llama.SamplerInitGreedy())
	defer llama.SamplerFree(sampler)
	buf := make([]byte, 256)
	pos := uint32(len(toks))
	idx := int32(len(toks) - 1)
	for {
		tok := llama.SamplerSample(sampler, m.ctx, idx)
		llama.SamplerAccept(sampler, tok)
		if llama.VocabIsEOG(m.vocab, tok) {
			c.Stop = cleanupd.StopEnd
			break
		}
		from := max(0, len(c.Text)-max(longest-1, 0))
		c.Text = append(c.Text, m.piece(tok, buf)...)
		c.TokensOut++
		if at := cleanupd.FindEnd(c.Text, end, from); at >= 0 {
			c.Text = c.Text[:at]
			c.Stop = cleanupd.StopEnd
			break
		}
		if c.TokensOut >= maxTokens {
			c.Stop = cleanupd.StopLimit
			break
		}
		if pos >= nCtx {
			c.Stop = cleanupd.StopContext
			break
		}
		one := llama.BatchGetOne([]llama.Token{tok})
		if rc, err := llama.Decode(m.ctx, one); err != nil || rc != 0 {
			return c, decodeFailed("decode failed after %d tokens: %d %v", c.TokensOut, rc, err)
		}
		// BatchGetOne has one token with logits; sample from the last.
		idx = -1
		pos++
	}
	return c, nil
}

func decodeFailed(format string, args ...any) error {
	return &cleanupd.Error{Code: cleanupd.DecodeFailed, Message: fmt.Sprintf(format, args...)}
}
