package ui

import (
	"fmt"
	"math"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
)

// Onboarding: seven steps in one window, as the Paper page "Onboarding"
// draws them and crates/sayso-app/src/onboarding (088f25f) runs them.
// Every move is saved, so Sayso resumes at the same step after a quit.

// The onboarding window, in DIPs: the Paper artboards and the Rust window.
const (
	OnboardingW, OnboardingH = 960, 640
	// OnboardingSteps is the number of onboarding steps.
	OnboardingSteps = 7
)

// The steps, in order.
const (
	stepWelcome = iota
	stepModel
	stepAI
	stepPermissions
	stepHotkeys
	stepPractice
	stepDone
)

// onboardingState is what the onboarding window keeps between frames.
type onboardingState struct {
	step int
	// dir is 1 after a step forward and -1 after a step back: the way the
	// next page slides in.
	dir float32
	// started is when the window opened: the clock of the ink animations.
	started time.Time

	// The model step: the list of all models, and the download samples of
	// the time estimate.
	allModels bool
	allQuery  string
	allField  mu.Handle
	dl        downloadClock

	ai     aiForm
	access accessState
	keys   hotkeyRecorder

	practiceText  string
	practiceField mu.Handle
	practiceSince time.Time

	pindropChecked, pindrop bool
}

// NeedsOnboarding is true until the user finished onboarding.
func (a *App) NeedsOnboarding() bool { return !a.cfg.Onboarding.Completed }

// OnboardingStep is the step shown, from 0.
func (a *App) OnboardingStep() int { return a.onb.step }

// clampStep keeps a saved step inside the steps.
func clampStep(s int) int { return min(max(s, 0), OnboardingSteps-1) }

// goToStep shows a step and saves it, so a quit resumes there.
func (a *App) goToStep(s int) {
	s = clampStep(s)
	if s < a.onb.step {
		a.onb.dir = -1
	} else {
		a.onb.dir = 1
	}
	a.cancelRecording()
	a.onb.step = s
	a.cfg.Onboarding.Step = uint8(s)
	a.saveConfig()
	a.enteredStep(s)
	a.invalidate()
}

// setOnboardingStep is goToStep, kept for the commands and tests.
func (a *App) setOnboardingStep(s int) { a.goToStep(s) }

// enteredStep is the work to do when a step appears.
func (a *App) enteredStep(s int) {
	if s != stepPermissions {
		a.stopMeter()
	}
	switch s {
	case stepAI:
		a.detectProviders(false)
	case stepPractice:
		a.onb.practiceSince = a.now()
		a.onb.practiceText = ""
	}
}

// onboardingNext is the primary button: it checks what the step needs,
// then goes on.
func (a *App) onboardingNext() {
	switch a.onb.step {
	case stepModel:
		if !a.modelStatus(a.FinalModel()).IsOnDisk() {
			// Start (or retry) the download and stay until it is done.
			a.ensureDownload()
			a.invalidate()
			return
		}
	case stepAI:
		if !a.saveAI() {
			a.invalidate()
			return
		}
	case stepDone:
		a.FinishOnboarding()
		return
	}
	a.goToStep(a.onb.step + 1)
}

// onboardingSkip leaves a step without what it asks for.
func (a *App) onboardingSkip() {
	if a.onb.step == stepAI {
		a.skipAI()
	}
	a.goToStep(a.onb.step + 1)
}

// FinishOnboarding closes onboarding for good and opens the hub.
func (a *App) FinishOnboarding() {
	a.stopMeter()
	a.cancelRecording()
	a.cfg.Onboarding.Completed = true
	a.saveConfig()
	a.finishSecondary()
}

// footer is what the footer shows for a step.
type footer struct {
	label   string
	enabled bool
	// skip is a ghost action left of the primary button ("Skip for now").
	skip string
}

func (a *App) footerFor() footer {
	switch a.onb.step {
	case stepWelcome:
		return footer{label: i18n.T("onboarding.footer.get_started"), enabled: true}
	case stepModel:
		// A downloaded model is required: dictation cannot work without one.
		st := a.modelStatus(a.FinalModel())
		switch {
		case st.IsOnDisk():
			return footer{label: i18n.T("onboarding.footer.continue"), enabled: true}
		case st.State == engine.StateDownloading:
			return footer{label: i18n.T("onboarding.footer.downloading", "percent", fmt.Sprintf("%.0f", st.Fraction*100)), enabled: false}
		}
		return footer{label: i18n.T("onboarding.footer.download"), enabled: a.d.Engine != nil}
	case stepAI:
		// Continue saves the choice (a cleanup model starts its download).
		// Skip leaves AI off.
		label := i18n.T("onboarding.footer.continue")
		if a.aiDownloads() {
			label = i18n.T("onboarding.ai.download_and_continue")
		}
		_, chosen := a.aiChoice()
		return footer{label: label, enabled: chosen, skip: i18n.T("onboarding.footer.skip")}
	case stepPermissions:
		st := a.permissions()
		both := st.Microphone && (st.Accessibility || !hasInputPermissions())
		f := footer{label: i18n.T("onboarding.footer.continue"), enabled: both}
		if !both {
			f.skip = i18n.T("onboarding.footer.skip")
		}
		return f
	case stepPractice:
		return footer{label: i18n.T("onboarding.footer.finish"), enabled: true}
	case stepDone:
		return footer{label: i18n.T("onboarding.footer.open_sayso"), enabled: true}
	}
	return footer{label: i18n.T("onboarding.footer.continue"), enabled: true}
}

// OnboardingView is the view of the onboarding window: the board, the
// sheet with its step trail, the step, and the footer.
func (a *App) OnboardingView(c *mu.Context) {
	p := a.look(c)
	if a.onb.started.IsZero() {
		a.onb.started = time.Now()
	}
	a.onb.access.clock = c.Now()
	a.onboardingShortcuts(c)
	c.Root().Background(p.Ground)
	bar := c.TitleBar()
	top := float32(10)
	if bar.Bottom > 0 && bar.Left == 0 {
		top = max(top, bar.Bottom) // Windows' caption buttons sit above the sheet
	}
	mu.Column(c).Fill().Padding(top, 10, 10, 10).Draw(a.grain(p, "board", 0)).Children(func() {
		sheetStack(mu.Column(c).Grow(1).Basis(0).MinHeight(0).Radius(12), p).Draw(a.grain(p, "sheet", 12)).
			Role(mu.RoleMain).Label(i18n.T("onboarding.welcome.title")).Children(func() {
			a.onboardingTopBar(c, p)
			dir := a.onb.dir
			if dir == 0 {
				dir = 1
			}
			// The step slides in from the side it comes from.
			mu.Box(c).Grow(1).Basis(0).MinHeight(0).ClipY().ChildExit(mu.Motion{X: -24 * dir}).Children(func() {
				mu.Scroll(c.Key(onbStepKey(a.onb.step))).Absolute().Fill().Transition(mu.ElementTransition{
					Duration: 180 * time.Millisecond,
					Enter:    &mu.Motion{X: 24 * dir},
					Exit:     &mu.Motion{X: -24 * dir},
				}).Children(func() { a.onboardingStep(c, p) })
			})
			a.onboardingFooter(c, p)
		})
	})
	if a.onb.step == stepModel && a.onb.allModels {
		a.allModelsDialog(c, p)
	}
}

// onbStepKey is the identity of a step's page, for its transition.
type onbStepKey int

func (a *App) onboardingStep(c *mu.Context, p theme.Colors) {
	switch a.onb.step {
	case stepWelcome:
		a.onbWelcome(c, p)
	case stepModel:
		a.onbModel(c, p)
	case stepAI:
		a.onbAI(c, p)
	case stepPermissions:
		a.onbPermissions(c, p)
	case stepHotkeys:
		a.onbHotkeys(c, p)
	case stepPractice:
		a.onbPractice(c, p)
	default:
		a.onbDone(c, p)
	}
}

func (a *App) onboardingShortcuts(c *mu.Context) {
	if a.onb.keys.slot != "" {
		a.recordKeys(c)
		return
	}
	if c.Shortcut(0, mu.KeyEscape) {
		switch {
		case a.onb.allModels:
			a.closeAllModels()
		case a.onb.ai.localOpen:
			a.onb.ai.localOpen = false
		}
	}
}

// onboardingTopBar is the 52 px strip at the top of the sheet: room for the
// window controls, the trail of steps, and "Step 2 of 7".
func (a *App) onboardingTopBar(c *mu.Context, p theme.Colors) {
	bar := c.TitleBar()
	side := max(float32(120), bar.Left, bar.Right)
	mu.Row(c).Height(52).Shrink(0).PaddingX(18).AlignItems(mu.Center).DragWindow().Children(func() {
		mu.Box(c).Width(side - 18).Shrink(0)
		mu.Row(a.k(c, "onboarding.trail")).Grow(1).Justify(mu.Center).AlignItems(mu.Center).Role(mu.RoleGroup).
			Label(i18n.T("onboarding.step", "step", a.onb.step+1, "total", OnboardingSteps)).Children(func() {
			for i := range OnboardingSteps {
				if i > 0 {
					col := p.Rule
					if i <= a.onb.step {
						col = p.InkFill
					}
					mu.Box(c).Size(36, 1.5).Shrink(0).Background(col)
				}
				a.trailDot(c, p, i)
			}
		})
		// A live node: a screen reader says the new step when it changes.
		uiText(c, i18n.T("onboarding.step", "step", a.onb.step+1, "total", OnboardingSteps), 12, 400, p.Graphite).
			Width(side - 18).Shrink(0).TextAlign(mu.End).SingleLine().Role(mu.RoleStatus)
	})
}

// trailDot is one step of the trail. A step that is done is a button that
// goes back to it; the others only show the place.
func (a *App) trailDot(c *mu.Context, p theme.Colors, i int) {
	switch {
	case i < a.onb.step:
		b := mu.ButtonBase(a.k(c, fmt.Sprintf("onboarding.trail.%d", i+1))).Label(i18n.T("onboarding.go_to_step", "step", i+1)).
			Size(17, 17).Justify(mu.Center).AlignItems(mu.Center).Radius(99).Margin(0, -4, 0, -4).Cursor(mu.CursorPointer)
		b.Children(func() { mu.Box(c).Size(9, 9).Radius(99).Background(p.InkFill) })
		if clicked(c, b) {
			a.goToStep(i)
		}
	case i == a.onb.step:
		mu.Box(c).Size(18, 18).Shrink(0).Radius(99).Background(p.InkFill.Alpha(0.14)).Margin(0, -3.5, 0, -3.5).
			Justify(mu.Center).AlignItems(mu.Center).Children(func() {
			mu.Box(c).Size(11, 11).Radius(99).Background(p.InkFill)
		})
	default:
		mu.Box(c).Size(9, 9).Shrink(0).Radius(99).Background(p.Deboss).InsetShadow(0, 1, 1.5, 0, p.ShadowA(0.3))
	}
}

// onboardingFooter is the 76 px strip under a rule: the hint, then Back,
// Skip and the primary button.
func (a *App) onboardingFooter(c *mu.Context, p theme.Colors) {
	f := a.footerFor()
	last := a.onb.step == stepDone
	mu.Row(c).Height(76).Shrink(0).PaddingX(28).AlignItems(mu.Center).Gap(16).BorderWidth(1, 0, 0, 0).BorderColor(p.Rule).Children(func() {
		if !last {
			uiText(c, i18n.T("onboarding.footer.hint"), 13, 400, p.Graphite).Grow(1).Basis(0).MinWidth(0).SingleLine().Ellipsis("…")
		} else {
			mu.Spacer(c)
		}
		mu.Row(c).Gap(8).AlignItems(mu.Center).Shrink(0).Children(func() {
			if !last && a.onb.step > 0 {
				if a.button(c, p, "onboarding.back", i18n.T("onboarding.footer.back"), bopt{kind: btnGhost, size: sizeLarge}) {
					a.goToStep(a.onb.step - 1)
				}
			}
			if f.skip != "" && a.button(c, p, "onboarding.skip", f.skip, bopt{kind: btnGhost, size: sizeLarge}) {
				a.onboardingSkip()
			}
			if a.button(c, p, "onboarding.next", f.label, bopt{kind: btnPrimary, size: sizeLarge, trail: "arrow-right", disabled: !f.enabled}) && f.enabled {
				a.onboardingNext()
			}
		})
	})
}

// background runs slow work off the UI thread, which comes back with
// a.post. Without a UI thread (tests) it runs at once.
func (a *App) background(fn func()) {
	if a.d.Post == nil {
		fn()
		return
	}
	go fn()
}

// ---- Pieces ----

// onbSide is the side padding of a step: 72, as designed.
const onbSide = 72

// stepColumn is a step's column: the title and lead, then body.
func stepColumn(c *mu.Context, p theme.Colors, title, lead string, body func()) {
	mu.Column(c).Padding(28, onbSide, 24, onbSide).Gap(22).Children(func() {
		mu.Column(c).Gap(8).Shrink(0).Children(func() {
			pressed(c, p, title, 36, 44).Role(mu.RoleHeading)
			uiText(c, lead, 15, 400, p.Graphite).FixedLineHeight(22)
		})
		body()
	})
}

// pressed is a letterpressed Fraunces title with a line of line DIPs.
// MyGo never makes a line shorter than the face's ascent and descent, so a
// tighter line takes negative margins (see heading).
func pressed(c *mu.Context, p theme.Colors, s string, size, line float32) mu.Element {
	e := mu.Text(c, s).Font(theme.Display).FontSize(size).FontWeight(600).TextColor(p.Ink).LineHeight(line / size)
	if d := (frauncesLine*size - line) / 2; d > 0 {
		e.Margin(-d, 0, -d, 0)
	}
	return letterpress(e, p)
}

// title is a Fraunces SemiBold card title (text::title).
func cardTitle(c *mu.Context, p theme.Colors, s string, size, line float32) mu.Element {
	return mu.Text(c, s).Font(theme.Display).FontSize(size).FontWeight(600).TextColor(p.Ink).FixedLineHeight(line)
}

// onbCard is a raised card, as in the onboarding design.
func onbCard(c *mu.Context, p theme.Colors) mu.Element {
	return raised(mu.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(12).Padding(18).Radius(14), p)
}

// ring marks the card that is chosen, or the one that needs you now.
func ring(e mu.Element, p theme.Colors) mu.Element {
	return e.Shadow(0, 0, 0, 2, p.Ink)
}

// checkLine is a check in an ink disc and a label: "Allowed", "Works".
func checkLine(c *mu.Context, p theme.Colors, label string, weight int) mu.Element {
	return mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
		mu.Box(c).Size(16, 16).Shrink(0).Radius(99).Background(p.InkFill).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
			mu.Icon(c, icon("check")).FontSize(10).TextColor(p.OnInk)
		})
		uiText(c, label, 13, weight, p.Ink).Grow(1).Basis(0).MinWidth(0)
	})
}

type bannerKind int

const (
	bannerWarning bannerKind = iota
	bannerInfo
)

// banner is a notice row (Banner in basics.rs): a warning in the danger
// wash, or an info note with a rule. It is a live node.
func banner(c *mu.Context, p theme.Colors, kind bannerKind, text string) {
	bg, fg, border, ic, role := p.DangerWash, p.Danger, p.Danger.Alpha(0.18), "warning", mu.RoleAlert
	if kind == bannerInfo {
		bg, fg, border, ic, role = mu.Color{}, p.Graphite, p.Rule, "info", mu.RoleStatus
	}
	mu.Row(c).Gap(12).Padding(12, 14).Radius(10).Background(bg).Border(1, border).AlignItems(mu.Start).Role(role).Label(text).Children(func() {
		mu.Icon(c, icon(ic)).FontSize(16).TextColor(fg).Margin(1, 0, 0, 0)
		uiText(c, text, 13, 400, fg).FixedLineHeight(19).Grow(1).Basis(0).MinWidth(0)
	})
}

// sizeText is "1.3 GB" or "626 MB".
func sizeText(bytes uint64) string {
	if bytes >= 1_000_000_000 {
		return i18n.T("onboarding.size_gb", "number", i18n.Decimal(float64(bytes)/1e9, 1))
	}
	return i18n.T("onboarding.size_mb", "number", int(math.Round(float64(bytes)/1e6)))
}

// sizeLabel is a model's size, or "macOS" for a model of the system.
func sizeLabel(m models.ModelInfo) string {
	if m.SizeBytes == 0 {
		return "macOS"
	}
	return models.FormatSize(m.SizeBytes)
}

// ---- Step 1: Welcome ----

// heroLevels is a calm stroke for the hero: high in the middle, two
// smaller swells.
var heroLevels = [12]float32{0.35, 0.55, 0.8, 0.62, 0.9, 1.0, 0.86, 0.66, 0.84, 0.6, 0.42, 0.3}

func (a *App) onbWelcome(c *mu.Context, p theme.Colors) {
	reduce := reduceMotion(c)
	started := a.onb.started
	mu.Column(c).FillWidth().MinHeight(462).Justify(mu.Center).AlignItems(mu.Center).Gap(28).Padding(0, 96, 24, 96).Children(func() {
		mu.Box(c).Size(520, 84).Shrink(0).AccessHidden(true).Draw(func(pt *mu.Painter, r mu.Rect) {
			t := float32(0)
			if !reduce {
				t = float32(time.Since(started).Seconds()) * 0.35
				pt.AnimationFrame()
			}
			levels := make([]float32, len(heroLevels))
			for i, l := range heroLevels {
				levels[i] = l * (0.92 + 0.08*float32(math.Sin(float64(t*2+float32(i)*0.7))))
			}
			paintInkLayers(pt, r, levels, t, p.Ink, inkLayers)
			for _, d := range [][4]float32{{484, 21, 2.6, 0.4}, {86, 60, 2.1, 0.3}, {313, 74, 1.6, 0.3}} {
				pt.Fill(mu.Rect{X: r.X + d[0], Y: r.Y + d[1], W: d[2] * 2, H: d[2] * 2}, p.Ink.Alpha(d[3]), d[2])
			}
		})
		mu.Column(c).Gap(14).AlignItems(mu.Center).Children(func() {
			pressed(c, p, i18n.T("onboarding.welcome.title"), 56, 59).Role(mu.RoleHeading).TextAlign(mu.Center)
			uiText(c, i18n.TOS("onboarding.welcome.lead", nil), 17, 400, p.Graphite).FixedLineHeight(26).TextAlign(mu.Center).Width(520)
		})
		mu.Row(c).Gap(28).Padding(8, 0, 0, 0).Wrap().Justify(mu.Center).Children(func() {
			for _, f := range []string{i18n.TOS("onboarding.welcome.fact_engine", nil), i18n.T("onboarding.welcome.fact_every_app"), i18n.T("onboarding.welcome.fact_setup_time")} {
				mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
					mu.Box(c).Size(6, 6).Shrink(0).Radius(99).Background(p.Ink)
					uiText(c, f, 14, 400, p.Ink).FixedLineHeight(18).SingleLine()
				})
			}
		})
		mu.Row(c).Gap(24).Wrap().Justify(mu.Center).AlignItems(mu.Center).Children(func() {
			mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
				uiText(c, i18n.T("onboarding.welcome.interface_language"), 13, 400, p.Graphite)
				a.languageSelect(c, "onboarding.language")
			})
			mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
				uiText(c, i18n.T("onboarding.welcome.dictation_language"), 13, 400, p.Graphite)
				a.dictationLanguageSelect(c, p)
			})
		})
	})
}

// dictationLanguageSelect picks the language the user speaks. A new
// language also selects the model suggested for it.
func (a *App) dictationLanguageSelect(c *mu.Context, p theme.Colors) {
	codes := sortedLanguages()
	labels := make([]string, len(codes))
	sel := ""
	for i, code := range codes {
		labels[i] = dictationLanguageName(code)
		if code == a.cfg.Dictation.Language {
			sel = labels[i]
		}
	}
	if sel == "" {
		sel = dictationLanguageName(a.cfg.Dictation.Language)
	}
	a.selectBtn(c, p, "onboarding.dictation_language", i18n.T("onboarding.welcome.dictation_language"), &sel, labels, 0).OnChange(func() {
		for i, l := range labels {
			if l == sel {
				a.setDictationLanguage(codes[i])
			}
		}
	})
}

// setDictationLanguage sets the language and the model suggested for it.
func (a *App) setDictationLanguage(code string) {
	a.cfg.Dictation.Language = code
	a.cfg.Dictation.Model = suggestedModel(code)
	a.saveConfig()
	a.invalidate()
}

// ---- Step 7: Done ----

func (a *App) onbDone(c *mu.Context, p theme.Colors) {
	h := a.cfg.Hotkeys
	lead := i18n.T("onboarding.done.lead_none")
	switch {
	case h.Toggle != nil:
		lead = i18n.T("onboarding.done.lead_toggle", "keys", keysText(h.Toggle.Keycaps()))
	case h.PushToTalk != nil:
		lead = i18n.T("onboarding.done.lead_push", "keys", keysText(h.PushToTalk.Keycaps()))
	}
	mu.Column(c).FillWidth().MinHeight(462).Justify(mu.Center).AlignItems(mu.Center).Gap(26).Padding(0, onbSide, 16, onbSide).Children(func() {
		ringedSeal(c, p, 92)
		mu.Column(c).Gap(10).AlignItems(mu.Center).Children(func() {
			pressed(c, p, i18n.T("onboarding.done.title"), 48, 58).Role(mu.RoleHeading).TextAlign(mu.Center)
			uiText(c, lead, 16, 400, p.Graphite).FixedLineHeight(24).TextAlign(mu.Center).Width(480)
		})
		mu.Row(c).FillWidth().Gap(14).AlignItems(mu.Stretch).Children(func() {
			tourCard(c, p, func() {
				mu.Box(c).Size(52, 14).Radius(99).Background(p.Sheet).InsetShadow(0, 1, 0, 0, p.Highlight(1)).
					Shadow(0, 1, 2, 0, p.ShadowA(0.2)).Shadow(0, 4, 12, 0, p.ShadowA(0.14)).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
					mu.Box(c).Size(18, 2).Radius(2).Background(p.Ink.Alpha(0.55))
				})
			}, i18n.T("onboarding.done.pill_title"), i18n.T("onboarding.done.pill_body"))
			tourCard(c, p, func() {
				mu.Box(c).Size(34, 24).Radius(6).Background(p.Ink.Alpha(0.1)).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
					mu.Icon(c, icon("wordmark")).FontSize(18).TextColor(p.Ink)
				})
			}, i18n.TOS("onboarding.done.menu_title", nil), i18n.T("onboarding.done.menu_body"))
			tourCard(c, p, func() {
				if pl := h.PasteLast; pl != nil {
					keycaps(c, p, pl.Keycaps(), true)
				} else {
					uiText(c, i18n.T("onboarding.done.not_set"), 12, 400, p.Graphite)
				}
			}, i18n.T("onboarding.done.paste_title"), i18n.T("onboarding.done.paste_body"))
		})
		// Shown only on a computer that has Pindrop data.
		if a.pindropFound() {
			line := a.set.importMsg
			if line == "" {
				line = i18n.T("onboarding.done.pindrop_found")
			}
			running := a.set.importMsg == i18n.T("app.pindrop.running")
			mu.Row(c).FillWidth().Gap(14).AlignItems(mu.Center).Children(func() {
				uiText(c, line, 13, 400, p.Graphite).FixedLineHeight(19).Grow(1).Basis(0).Role(mu.RoleStatus)
				if a.button(c, p, "onboarding.import_pindrop", i18n.T("onboarding.done.import_pindrop"), bopt{size: sizeSmall, disabled: running}) && !running {
					a.importPindrop()
				}
			})
		}
	})
}

// pindropFound is true when this computer has Pindrop data; it looks once.
func (a *App) pindropFound() bool {
	if !a.onb.pindropChecked {
		a.onb.pindropChecked = true
		a.onb.pindrop = a.d.Store != nil && store.PindropFound(pindropDir())
	}
	return a.onb.pindrop
}

func tourCard(c *mu.Context, p theme.Colors, art func(), title, body string) {
	raised(mu.Column(c).Grow(1).Basis(0).MinWidth(0).AlignItems(mu.Center).Gap(12).Padding(18, 16).Radius(14), p).Children(func() {
		mu.Row(c).Height(40).Justify(mu.Center).AlignItems(mu.Center).Children(art)
		uiText(c, title, 14, 600, p.Ink).FixedLineHeight(18).TextAlign(mu.Center)
		uiText(c, body, 13, 400, p.Graphite).FixedLineHeight(19).TextAlign(mu.Center)
	})
}

// ringedSeal is the large onboarding seal: the domed accent stamp with an
// inner ring, a check and a halo (Seal::ringed).
func ringedSeal(c *mu.Context, p theme.Colors, size float32) {
	halo := float32(6)
	mu.Box(c).Size(size+halo*2, size+halo*2).Shrink(0).Radius(99).Background(p.Accent.Alpha(0.10)).
		Justify(mu.Center).AlignItems(mu.Center).AccessHidden(true).Children(func() {
		mu.Box(c).Size(size, size).Radius(99).Background(p.Accent).Justify(mu.Center).AlignItems(mu.Center).
			InsetShadow(0, 1, 0, 0, white.Alpha(0.25)).InsetShadow(0, -2, 3, 0, mu.RGBA(0x0A, 0x0E, 0x32, 0.35)).
			Shadow(0, 1, 2, 0, mu.RGBA(0x14, 0x18, 0x3C, 0.30)).Shadow(0, 6, 16, 0, mu.RGBA(0x14, 0x18, 0x3C, 0.18)).Children(func() {
			mu.Box(c).Size(size*0.76, size*0.76).Radius(99).Border(1, p.OnInk.Alpha(0.35)).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
				mu.Icon(c, icon("check")).FontSize(size * 0.5).TextColor(p.OnInk)
			})
		})
	})
}

// keysText is a hotkey's caps as a sentence shows them: "⌥ Space".
func keysText(caps []string) string {
	out := ""
	for i, k := range caps {
		if i > 0 {
			out += " "
		}
		out += k
	}
	return out
}
