package enhance

import (
	"log/slog"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
)

// BuildEnhancer makes the enhancer of a provider (sayso-enhance build_enhancer).
// The Claude CLI and ACP agent kinds are not ported yet: they report "not set up".
func BuildEnhancer(p config.Provider, secrets providers.SecretStore, cfg config.AI, engineModel LanguageModel) Enhancer {
	httpTimeout := time.Duration(cfg.HTTPTimeoutMs) * time.Millisecond
	cliTimeout := time.Duration(cfg.CLITimeoutMs) * time.Millisecond
	switch p.Kind {
	case config.KindOpenAICompatible:
		e := NewOpenAICompatible(p.ID, p.BaseURL, p.ModelName()).WithCloud(p.IsCloud()).WithThinking(p.ThinkingSent())
		e.ZeroDataRetention = p.ZeroDataRetention
		e.DefaultTimeout = httpTimeout
		if p.APIKeyAccount != nil {
			if key := secrets.Get(*p.APIKeyAccount); key != "" {
				e.WithAPIKey(key)
			} else {
				e.WithSetupError("no API key is saved for \"" + *p.APIKeyAccount + "\"")
			}
		}
		return e
	case config.KindChatGPT:
		e := NewChatGPT(p.ID, p.ModelName(), secrets).WithThinking(p.ThinkingSent())
		e.DefaultTimeout = cliTimeout
		return e
	case config.KindAppleIntelligence:
		e := NewEngineModel(p.ID, engineModel)
		e.DefaultTimeout = httpTimeout
		return e
	case config.KindCleanupModel:
		return stubEnhancer{id: p.ID, reason: "cleanup models run in the cleanup runtime"}
	case config.KindClaudeCLI:
		return stubEnhancer{id: p.ID, cloud: true, reason: "the Claude CLI provider is not available in this build"}
	case config.KindACP:
		return stubEnhancer{id: p.ID, cloud: true, reason: "ACP agents are not available in this build"}
	}
	return stubEnhancer{id: p.ID, reason: "unknown provider kind " + string(p.Kind)}
}

// The text pipeline of a dictation (sayso-core/src/pipeline.rs):
// replacements, then the style (a prompt on an enhancer, or a cleanup model).
// Spoken punctuation runs in the dictation controller between the two.

const (
	baseTimeoutWords = 100
	timeoutPerWord   = 50 * time.Millisecond
)

// CleanerRun is a cleanup model run for one dictation.
type CleanerRun struct {
	Cleaner             Cleaner
	ProviderID          string
	Model               *CleanupModelInfo
	Plan                RunPlan
	LanguageSetting     string
	FinalModelLanguages []string
	App, Site           string
}

// CleanupNotice is what the overlay says about a cleanup model.
type CleanupNotice struct {
	Kind     string // cleanup_only, language, unknown_language
	Model    string
	Language string
}

// PipelineOutput is the result of the text steps.
type PipelineOutput struct {
	Text         string
	Replacements []store.AppliedReplacement
	Enhance      store.EnhanceOutcome
	EnhanceError error
	ErrorFrom    string // the provider of EnhanceError
	Timings      *store.EnhanceTimings
	Notice       *CleanupNotice
}

// TextPipeline holds what one dictation's text steps need.
type TextPipeline struct {
	Replacer   *Replacer
	Style      *Style
	BasePrompt string
	Vocabulary []string
	Enhancer   Enhancer    // nil: no provider
	Cleaner    *CleanerRun // set when the provider is a cleanup model
	Timeout    time.Duration
}

// stepTimeout is the style timeout plus 50 ms per word over 100 words.
func (p *TextPipeline) stepTimeout(text string) time.Duration {
	base := p.Timeout
	if p.Style.TimeoutMs != nil {
		base = time.Duration(*p.Style.TimeoutMs) * time.Millisecond
	}
	if base == 0 {
		return 0
	}
	extra := len(strings.Fields(text)) - baseTimeoutWords
	if extra < 0 {
		extra = 0
	}
	return base + timeoutPerWord*time.Duration(extra)
}

// WillEnhance is true when the style step will run AI.
func (p *TextPipeline) WillEnhance() bool {
	if p.Cleaner != nil {
		return p.Cleaner.Plan.Kind == PlanCleanup || p.Cleaner.Plan.Kind == PlanCleanupOnly
	}
	return p.Style.UsesAI() && p.Enhancer != nil
}

// Replace runs the dictionary on the trimmed transcript.
func (p *TextPipeline) Replace(transcript string) (string, []store.AppliedReplacement) {
	return p.Replacer.Apply(strings.TrimSpace(transcript))
}

// Run runs replacements and the style.
func (p *TextPipeline) Run(transcript string) PipelineOutput {
	replaced, applied := p.Replace(transcript)
	return p.RunStyle(replaced, applied)
}

func notUsed(text string, reps []store.AppliedReplacement, notice *CleanupNotice) PipelineOutput {
	return PipelineOutput{Text: text, Replacements: reps, Enhance: store.EnhanceOutcome{Result: store.EnhanceNotUsed}, Notice: notice}
}

func isRunaway(text, reply string) bool {
	return utf8.RuneCountInString(reply) > utf8.RuneCountInString(text)*3+200
}

// RunStyle runs the style on text after replacements. On failure the text
// after replacements comes back, with the error.
func (p *TextPipeline) RunStyle(replaced string, reps []store.AppliedReplacement) PipelineOutput {
	if p.Cleaner != nil {
		return p.runCleanup(*p.Cleaner, replaced, reps)
	}
	if p.Enhancer == nil || !p.Style.UsesAI() || strings.TrimSpace(replaced) == "" {
		return notUsed(replaced, reps, nil)
	}
	spoken := Spelled(replaced)
	timeout := p.stepTimeout(spoken)
	req := Request{SystemPrompt: SystemPrompt(p.Style, p.BasePrompt, p.Vocabulary, spoken), Transcript: spoken, Temperature: p.Style.Temperature, Timeout: timeout}
	if p.Style.Model != nil {
		req.Model = *p.Style.Model
	}
	var deadline time.Time
	if timeout > 0 {
		deadline = time.Now().Add(timeout)
	}
	attempts := []store.EnhanceAttempt{}
	attempt := func(r Request) (Response, error) {
		started := time.Now()
		resp, err := p.Enhancer.Enhance(r)
		if err == nil && isRunaway(replaced, resp.Text) {
			err = invalidOutput("the reply is much longer than the text")
		}
		a := store.EnhanceAttempt{ElapsedMs: uint64(time.Since(started).Milliseconds())}
		if err != nil {
			s := err.Error()
			a.Error = &s
		} else {
			m := resp.Mode
			a.Mode = &m
			a.Report = resp.Report
		}
		attempts = append(attempts, a)
		return resp, err
	}
	resp, err := attempt(req)
	if IsKind(err, ErrInvalidOutput) {
		left := time.Duration(-1)
		if !deadline.IsZero() {
			left = max(time.Until(deadline), 0)
		}
		if left != 0 {
			if left > 0 {
				req.Timeout = left
			}
			resp, err = attempt(req)
			if !deadline.IsZero() && IsKind(err, ErrTimeout) {
				err = &Error{Kind: ErrTimeout, Ms: uint64(timeout.Milliseconds())}
			}
		}
	}
	var elapsed uint64
	for _, a := range attempts {
		elapsed += a.ElapsedMs
	}
	timings := &store.EnhanceTimings{TimeoutMs: uint64(timeout.Milliseconds()),
		PromptChars: utf8.RuneCountInString(req.SystemPrompt) + utf8.RuneCountInString(req.Transcript), Attempts: attempts}
	if err != nil {
		slog.Warn("style failed", "style", p.Style.ID, "provider", p.Enhancer.ID(), "error", err)
		return PipelineOutput{Text: replaced, Replacements: reps,
			Enhance:      store.EnhanceOutcome{Result: store.EnhanceFailed, ProviderID: p.Enhancer.ID(), Reason: err.Error()},
			EnhanceError: err, ErrorFrom: p.Enhancer.ID(), Timings: timings}
	}
	return PipelineOutput{Text: strings.TrimSpace(resp.Text), Replacements: reps,
		Enhance: store.EnhanceOutcome{Result: store.EnhanceApplied, ProviderID: resp.ProviderID, Model: resp.Model, ElapsedMs: elapsed},
		Timings: timings}
}

func (p *TextPipeline) runCleanup(run CleanerRun, replaced string, reps []store.AppliedReplacement) PipelineOutput {
	var settings Settings
	switch run.Plan.Kind {
	case PlanCleanup:
		settings = run.Plan.Settings
	case PlanCleanupOnly:
	default:
		return notUsed(replaced, reps, nil)
	}
	if strings.TrimSpace(replaced) == "" {
		return notUsed(replaced, reps, nil)
	}
	model := run.Model.ID
	lang, ok := DictationLanguage(run.LanguageSetting, run.FinalModelLanguages, replaced, DetectLanguage)
	if !ok {
		return notUsed(replaced, reps, &CleanupNotice{Kind: "unknown_language", Model: model})
	}
	if !run.Model.SupportsLanguage(lang) {
		return notUsed(replaced, reps, &CleanupNotice{Kind: "language", Model: model, Language: lang})
	}
	spoken := Spelled(replaced)
	timeout := p.stepTimeout(spoken)
	started := time.Now()
	resp, cerr := run.Cleaner.Clean(CleanupRequest{Model: model, Text: spoken, Settings: settings, App: run.App, Site: run.Site, Timeout: timeout})
	var err error
	if cerr != nil {
		if ce, ok := cerr.(*CleanError); ok {
			err = ce.ToEnhanceError()
		} else {
			err = &Error{Kind: ErrCLI, Message: cerr.Error()}
		}
	}
	elapsed := uint64(time.Since(started).Milliseconds())
	cleanupOnly := run.Plan.Kind == PlanCleanupOnly
	mode := "cleanup"
	a := store.EnhanceAttempt{ElapsedMs: elapsed, Mode: &mode}
	if err != nil {
		s := err.Error()
		a.Error = &s
	}
	l := lang
	ct := &store.CleanupTimings{Language: &l, CleanupOnly: cleanupOnly}
	timings := &store.EnhanceTimings{TimeoutMs: uint64(timeout.Milliseconds()), PromptChars: utf8.RuneCountInString(spoken),
		Attempts: []store.EnhanceAttempt{a}, Cleanup: ct}
	if err != nil {
		slog.Warn("style failed with cleanup model", "style", p.Style.ID, "model", model, "error", err)
		return PipelineOutput{Text: replaced, Replacements: reps,
			Enhance:      store.EnhanceOutcome{Result: store.EnhanceFailed, ProviderID: run.ProviderID, Reason: err.Error()},
			EnhanceError: err, ErrorFrom: run.ProviderID, Timings: timings}
	}
	ct.LoadMs, ct.Parts, ct.TokensIn, ct.TokensOut = resp.LoadMs, resp.Parts, resp.TokensIn, resp.TokensOut
	terms := append([]string{}, p.Vocabulary...)
	for _, r := range reps {
		terms = append(terms, r.To)
	}
	out := PipelineOutput{Text: RestoreSpelling(spoken, resp.Text, terms), Replacements: reps,
		Enhance: store.EnhanceOutcome{Result: store.EnhanceApplied, ProviderID: run.ProviderID, Model: resp.Model, ElapsedMs: elapsed},
		Timings: timings}
	if cleanupOnly {
		out.Notice = &CleanupNotice{Kind: "cleanup_only", Model: model}
	}
	return out
}
