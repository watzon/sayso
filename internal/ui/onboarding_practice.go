package ui

import (
	"fmt"
	"runtime"
	"strings"
	"unicode"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
)

// Step 6: try a dictation into a real text field in this window. This
// window is the focused app, so the toggle hotkey records and the paste
// lands in this field. The result line comes from the newest history entry
// (onboarding/practice.rs).

// practiceSentence is the sentence to read.
func practiceSentence() string {
	switch runtime.GOOS {
	case "darwin":
		return "Sayso writes what I say, in any app on my Mac, and nothing leaves this computer."
	case "windows":
		return "Sayso writes what I say, in any app on my PC, and nothing leaves this computer."
	}
	return "Sayso writes what I say, in any app I use, and nothing leaves this computer."
}

// wordsCorrect counts the words of the sentence that result has, in order
// (the longest common subsequence), and the words of the sentence.
func wordsCorrect(sentence, result string) (int, int) {
	norm := func(s string) []string {
		var out []string
		for _, w := range strings.Fields(s) {
			w = strings.ToLower(strings.Map(func(r rune) rune {
				if unicode.IsLetter(r) || unicode.IsDigit(r) {
					return r
				}
				return -1
			}, w))
			if w != "" {
				out = append(out, w)
			}
		}
		return out
	}
	a, b := norm(sentence), norm(result)
	dp := make([][]int, len(a)+1)
	for i := range dp {
		dp[i] = make([]int, len(b)+1)
	}
	for i := 1; i <= len(a); i++ {
		for j := 1; j <= len(b); j++ {
			if a[i-1] == b[j-1] {
				dp[i][j] = dp[i-1][j-1] + 1
			} else {
				dp[i][j] = max(dp[i-1][j], dp[i][j-1])
			}
		}
	}
	return dp[len(a)][len(b)], len(a)
}

// insertDelayMs is the time from the stop to the insert: the final pass
// plus the style, if any.
func insertDelayMs(e store.HistoryEntry) uint64 {
	ms := e.TranscribeMs
	if e.Enhance.Result == store.EnhanceApplied {
		ms += e.Enhance.ElapsedMs
	}
	return ms
}

// practiceEntry is the newest dictation of this visit to the step.
func (a *App) practiceEntry() (store.HistoryEntry, bool) {
	a.loadHome()
	if len(a.home.recent) == 0 {
		return store.HistoryEntry{}, false
	}
	e := a.home.recent[0]
	if e.CreatedAt.Before(a.onb.practiceSince) {
		return store.HistoryEntry{}, false
	}
	return e, true
}

func readiness(c *mu.Context, p theme.Colors, label string, col mu.Color) {
	mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
		mu.Box(c).Size(7, 7).Shrink(0).Radius(99).Background(col)
		uiText(c, label, 13, 400, p.Ink).SingleLine()
	})
}

func (a *App) onbPractice(c *mu.Context, p theme.Colors) {
	h := a.cfg.Hotkeys
	lead := i18n.T("onboarding.practice.lead_push")
	if h.Toggle != nil {
		lead = i18n.T("onboarding.practice.lead_toggle", "keys", keysText(h.Toggle.Keycaps()))
	}
	final := a.FinalModel()
	st := a.modelStatus(final)
	perm := a.permissions()
	a.pollPermissions(c)
	stepColumn(c, p, i18n.T("onboarding.practice.title"), lead, func() {
		mu.Column(c).Gap(6).PaddingX(4).Children(func() {
			caps(c, p, i18n.T("onboarding.practice.read_this"))
			mu.Text(c, "“"+practiceSentence()+"”").Font(serifFont(theme.Display)).FontSize(22).FixedLineHeight(30).Italic().TextColor(p.Graphite)
		})
		field := mu.Column(c).MinHeight(120).Gap(12).Padding(18, 20).Radius(12).Background(p.SheetRaised).
			InsetShadow(0, 1, 3, 0, p.ShadowA(0.12)).Shadow(0, 0, 0, 2, p.Accent).Shadow(0, 1, 0, 0, p.Highlight(1))
		field.Children(func() {
			mu.TextAreaBase(a.k(c, "onboarding.practice"), &a.onb.practiceText).Label(i18n.T("onboarding.practice.field_label")).
				Placeholder(i18n.T("onboarding.practice.placeholder")).PlaceholderColor(p.Pencil).Font(serifFont(theme.Display)).FontSize(20).
				FixedLineHeight(28).TextColor(p.Ink).MinHeight(56).Lines(2, 5).Bind(&a.onb.practiceField).AutoFocus()
			a.practiceResult(c, p)
		})
		mu.Row(c).Gap(20).PaddingX(4).Wrap().Children(func() {
			// The final pass.
			label, col := i18n.T("onboarding.practice.final_not_downloaded"), p.Pencil
			switch {
			case st.IsOnDisk():
				label, col = i18n.T("onboarding.practice.final_ready"), p.Success
			case st.State == engine.StateDownloading:
				label, col = i18n.T("onboarding.practice.final_downloading", "percent", fmt.Sprintf("%.0f", st.Fraction*100)), p.Accent
			case st.State == engine.StateFailed:
				label, col = i18n.T("onboarding.practice.final_stopped"), p.Danger
			}
			readiness(c, p, label, col)
			// The live preview.
			label, col = i18n.T("onboarding.practice.preview_off"), p.Pencil
			if a.cfg.Dictation.LivePreview {
				pid, _ := a.previewModel()
				ps := a.modelStatus(pid)
				switch {
				case ps.State == engine.StateReady || (a.d.Engine != nil && ps.IsOnDisk() && a.d.Engine.PreviewReady(string(pid))):
					label, col = i18n.T("onboarding.practice.preview_ready"), p.Success
				case ps.State == engine.StateDownloading:
					label, col = i18n.T("onboarding.practice.preview_downloading", "percent", fmt.Sprintf("%.0f", ps.Fraction*100)), p.Accent
				case ps.IsOnDisk():
					label, col = i18n.T("onboarding.practice.preview_loading"), p.Accent
				default:
					label = i18n.T("onboarding.practice.preview_none")
				}
			}
			readiness(c, p, label, col)
			// The model in memory.
			label, col = i18n.T("onboarding.practice.opt_after_download"), p.Pencil
			switch st.State {
			case engine.StateReady:
				label, col = i18n.TOS("onboarding.practice.opt_ready", nil), p.Success
			case engine.StateOptimizing:
				label, col = i18n.TOS("onboarding.practice.opt_optimizing", nil), p.Accent
			case engine.StateDownloaded:
				// Rust model_idle: a model Sayso does not keep loaded
				// (preload off) waits for the first dictation; else it is
				// on its way into memory.
				label, col = i18n.T("onboarding.practice.opt_loading"), p.Accent
				if !a.cfg.Dictation.PreloadModel {
					label, col = i18n.T("onboarding.practice.opt_idle"), p.Success
				}
			}
			readiness(c, p, label, col)
		})
		if why := a.notReadyReason(st.IsOnDisk(), perm.Microphone); why != "" {
			banner(c, p, bannerInfo, why)
		}
	})
}

// notReadyReason says why Sayso cannot dictate yet, or "".
func (a *App) notReadyReason(modelOnDisk, micOK bool) string {
	switch {
	case !modelOnDisk:
		return i18n.T("onboarding.practice.not_ready_model")
	case !micOK && a.platformPermissions() != nil:
		return i18n.T("onboarding.practice.not_ready_mic")
	case !a.engineRunning():
		return i18n.T("onboarding.practice.not_ready_engine")
	}
	return ""
}

// practiceResult is the line under the field: what the dictation is doing,
// or how the last one went. It is a live node.
func (a *App) practiceResult(c *mu.Context, p theme.Colors) {
	e, ok := a.practiceEntry()
	switch {
	case ok && e.Insert.Result != store.InsertFailed:
		got, total := wordsCorrect(practiceSentence(), e.FinalText)
		inserted := i18n.T("onboarding.practice.inserted_after", "seconds", i18n.Decimal(float64(insertDelayMs(e))/1000, 1))
		correct := i18n.T("onboarding.practice.words_correct", "correct", got, "total", total)
		mu.Row(a.k(c, "onboarding.practice.result")).Gap(10).AlignItems(mu.Center).Role(mu.RoleStatus).Label(inserted + " " + correct).Children(func() {
			mu.Box(c).Size(22, 22).Shrink(0).Radius(99).Background(p.Accent).InsetShadow(0, -1.5, 2, 0, black.Alpha(0.3)).
				Justify(mu.Center).AlignItems(mu.Center).Children(func() {
				mu.Icon(c, icon("check")).FontSize(11).TextColor(p.OnInk)
			})
			uiText(c, inserted, 13, 600, p.Ink).SingleLine()
			uiText(c, correct, 13, 400, p.Graphite).SingleLine()
		})
	case ok:
		key := "onboarding.practice.paste_failed"
		if hasInputPermissions() {
			key = "onboarding.practice.paste_failed_ax"
		}
		uiText(c, i18n.T(key, "reason", e.Insert.Reason), 13, 400, p.Danger).Role(mu.RoleAlert)
	default:
		text := i18n.T("onboarding.practice.words_appear")
		switch a.state.Kind {
		case dictation.StateRecording:
			text = i18n.T("onboarding.practice.listening")
		case dictation.StateProcessing:
			text = i18n.T("onboarding.practice.writing")
		}
		uiText(a.k(c, "onboarding.practice.result"), text, 13, 400, p.Graphite).Role(mu.RoleStatus)
	}
}
