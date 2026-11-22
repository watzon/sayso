package ui

import (
	"fmt"
	"io"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
)

// Surface is one view of the app at one state, for the smoke run and the
// tests.
type Surface struct {
	Name   string
	W, H   int
	Prep   func(a *App)
	View   func(a *App) func(c *mu.Context)
	Expect string // a text the surface must show
	// Empty is true for a surface that draws nothing: the notch at rest,
	// whose window hides.
	Empty bool
}

// Surfaces are every page, section, overlay state, popover and window.
func Surfaces() []Surface {
	var out []Surface
	hub := func(a *App) func(c *mu.Context) { return a.HubView }
	for _, p := range Pages[:5] {
		p := p
		out = append(out, Surface{Name: "hub/" + p.key(), W: HubWidth, H: HubHeight, Prep: func(a *App) { a.SetPage(p) }, View: hub})
	}
	for _, s := range Sections {
		s := s
		out = append(out, Surface{Name: "settings/" + s.key(), W: HubWidth, H: HubHeight, Prep: func(a *App) { a.OpenSettings(s) }, View: hub})
	}
	out = append(out, Surface{Name: "hub/narrow", W: HubMinWidth, H: HubMinHeight, Prep: func(a *App) { a.SetPage(PageHome) }, View: hub})
	overlay := func(a *App) func(c *mu.Context) { return a.OverlayView }
	states := []struct {
		name string
		s    dictation.State
	}{
		{"idle", dictation.State{Kind: dictation.StateIdle}},
		{"recording", dictation.State{Kind: dictation.StateRecording}},
		{"processing", dictation.State{Kind: dictation.StateProcessing}},
		{"done", dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeInserted}}},
		{"cancelled", dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeCancelled}}},
		{"failed", dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeError, Message: "boom"}}},
	}
	for _, style := range []config.OverlayStyle{config.OverlayPill, config.OverlayNotch} {
		for _, st := range states {
			style, st := style, st
			out = append(out, Surface{Name: fmt.Sprintf("overlay/%s/%s", style, st.name), W: PillWindowW, H: PillWindowH,
				Empty: style == config.OverlayNotch && st.s.Kind == dictation.StateIdle,
				Prep: func(a *App) {
					a.cfg.Overlay.Style, a.cfg.Overlay.IdlePill = style, true
					a.state = st.s
				}, View: overlay})
		}
	}
	out = append(out,
		Surface{Name: "popover", W: PopoverW, H: PopoverH, View: func(a *App) func(c *mu.Context) { return a.PopoverView }},
		Surface{Name: "whats_new", W: WhatsNewW, H: WhatsNewH, View: func(a *App) func(c *mu.Context) { return a.WhatsNewView }},
		Surface{Name: "update", W: UpdateW, H: UpdateH, View: func(a *App) func(c *mu.Context) { return a.UpdateView }},
		Surface{Name: "permissions", W: PermissionsW, H: PermissionsH, View: func(a *App) func(c *mu.Context) { return a.PermissionsView }},
		Surface{Name: "approval", W: ApprovalW, H: ApprovalH, View: func(a *App) func(c *mu.Context) { return a.ApprovalView }},
	)
	for i := range OnboardingSteps {
		i := i
		out = append(out, Surface{Name: fmt.Sprintf("onboarding/%d", i+1), W: OnboardingW, H: OnboardingH,
			Prep: func(a *App) { a.onb.step = i }, View: func(a *App) func(c *mu.Context) { return a.OnboardingView }})
	}
	return out
}

// Smoke renders every surface in light and dark without a window and
// writes one line for each. It restores the state it changed.
func Smoke(a *App, w io.Writer) (err error) {
	RegisterFonts()
	shell, page, inSettings, section, state := a.d.Shell, a.page, a.inSettings, a.section, a.state
	ov, step := a.cfg.Overlay, a.onb.step
	path := a.d.ConfigPath
	a.d.Shell, a.d.ConfigPath = nil, "" // render only: no windows, no saves
	defer func() {
		a.d.Shell, a.page, a.inSettings, a.section, a.state = shell, page, inSettings, section, state
		a.cfg.Overlay, a.onb.step, a.d.ConfigPath = ov, step, path
	}()
	n := 0
	for _, s := range Surfaces() {
		for _, dark := range []bool{false, true} {
			if s.Prep != nil {
				s.Prep(a)
			}
			if rerr := render(s, a, dark); rerr != nil {
				return fmt.Errorf("%s (dark %v): %w", s.Name, dark, rerr)
			}
			n++
		}
		fmt.Fprintf(w, "smoke: %s ok\n", s.Name)
	}
	fmt.Fprintf(w, "smoke: %d renders ok\n", n)
	return nil
}

func render(s Surface, a *App, dark bool) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("panic: %v", r)
		}
	}()
	tt := mu.NewTesterWith(s.View(a), s.W, s.H, mu.TesterOptions{Dark: dark})
	tt.Frame()
	// The overlay eases to its state; let it settle.
	tt.Advance(time.Second)
	if len(tt.Texts()) == 0 && !s.Empty {
		return fmt.Errorf("nothing drawn")
	}
	return nil
}
