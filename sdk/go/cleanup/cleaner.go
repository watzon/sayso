package cleanup

import (
	"context"
	"errors"
	"fmt"
	"time"
)

// TimePerToken is the time a cleanup gets per text token, on top of its
// timeout.
const TimePerToken = 15 * time.Millisecond

// Request is one cleanup of a whole text with a cleanup model.
type Request struct {
	Model     string
	Text      string
	Settings  Settings
	App, Site string
	// Timeout is one deadline for all parts; zero means none.
	Timeout time.Duration
}

// Response is the cleaned text.
type Response struct {
	Text                string
	Model               string
	ElapsedMs, LoadMs   uint64
	Parts               uint32
	TokensIn, TokensOut uint32
}

// Error is why a cleanup gave no text.
type Error struct {
	// Kind is "timeout", "not_configured", "runtime", or "invalid_output".
	Kind    string
	Ms      uint64
	Message string
	// NoLibraries: the runtime has no llama.cpp libraries.
	NoLibraries bool
}

func (e *Error) Error() string {
	switch e.Kind {
	case "timeout":
		return fmt.Sprintf("timed out after %d ms", e.Ms)
	case "not_configured":
		return "the cleanup model is not set up: " + e.Message
	case "invalid_output":
		return "the answer is not usable: " + e.Message
	}
	return e.Message
}

// Cleaner runs the cleanup models of ModelsDir on the runtime.
type Cleaner struct {
	Runtime   *Client
	ModelsDir string
}

// Warm loads model ahead and waits for the load.
func (c *Cleaner) Warm(model string) error {
	info, ok := Find(model)
	if !ok {
		return fmt.Errorf("no cleanup model %s", model)
	}
	path, ok := ModelPath(c.ModelsDir, info)
	if !ok {
		return fmt.Errorf("%s is not downloaded", info.Name)
	}
	return c.Runtime.EnsureLoaded(info.ID, path, NCtx)
}

// Clean splits the text, cleans each part, checks each answer, and joins
// them. ctx ends the cleanup early (the runtime is stopped).
func (c *Cleaner) Clean(ctx context.Context, r Request) (Response, error) {
	started := time.Now()
	info, ok := Find(r.Model)
	if !ok {
		return Response{}, &Error{Kind: "not_configured", Message: "no cleanup model " + r.Model}
	}
	path, ok := ModelPath(c.ModelsDir, info)
	if !ok {
		return Response{}, &Error{Kind: "not_configured", Message: info.Name + " is not downloaded"}
	}
	var resp Response
	var check error
	called := time.Now()
	err := c.Runtime.WithContext(ctx, info.ID, path, NCtx, func(s *Session) error {
		resp.LoadMs = uint64(time.Since(called).Milliseconds())
		begun := time.Now()
		left := func(until time.Time) (time.Duration, error) {
			if until.IsZero() {
				return 0, nil
			}
			d := time.Until(until)
			if d <= 0 {
				return 0, ErrTimeout
			}
			return d, nil
		}
		var planUntil time.Time
		if r.Timeout > 0 {
			planUntil = begun.Add(r.Timeout)
		}
		count := func(text string) (uint32, error) {
			d, err := left(planUntil)
			if err != nil {
				return 0, err
			}
			return s.Count(text, d)
		}
		parts, err := PlanParts(r.Text, MaxPartTokens, count)
		if err != nil {
			return err
		}
		counts := make([]uint32, len(parts))
		var total uint32
		for i, p := range parts {
			if counts[i], err = count(p); err != nil {
				return err
			}
			total += counts[i]
		}
		var deadline time.Time
		if r.Timeout > 0 {
			deadline = begun.Add(r.Timeout + TimePerToken*time.Duration(total))
		}
		answers := make([]string, 0, len(parts))
		for i, p := range parts {
			prompt := Render(info, p, r.Settings, r.App, r.Site, counts[i])
			d, err := left(deadline)
			if err != nil {
				return err
			}
			done, err := s.Complete(prompt.Text, prompt.End, prompt.MaxTokens, d)
			if err != nil {
				return err
			}
			resp.TokensIn += done.TokensIn
			resp.TokensOut += done.TokensOut
			answer, err := CheckAnswer(p, done.Stop, done.Text)
			if err != nil {
				check = err
				return nil
			}
			answers = append(answers, answer)
		}
		resp.Text = JoinAnswers(parts, answers)
		resp.Parts = uint32(len(parts))
		return nil
	})
	if err != nil {
		return Response{}, toError(err, r)
	}
	if check != nil {
		return Response{}, &Error{Kind: "invalid_output", Message: check.Error()}
	}
	resp.Model = info.ID
	resp.ElapsedMs = uint64(time.Since(started).Milliseconds())
	return resp, nil
}

func toError(err error, r Request) error {
	switch {
	case errors.Is(err, ErrTimeout):
		return &Error{Kind: "timeout", Ms: uint64(r.Timeout.Milliseconds())}
	case errors.Is(err, ErrNoLibraries):
		return &Error{Kind: "not_configured", Message: err.Error(), NoLibraries: true}
	case errors.Is(err, ErrDisabled), errors.Is(err, ErrStopped), errors.Is(err, ErrNotInstalled):
		return &Error{Kind: "not_configured", Message: err.Error()}
	}
	return &Error{Kind: "runtime", Message: err.Error()}
}
