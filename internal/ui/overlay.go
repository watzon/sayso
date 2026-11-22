package ui

import (
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The overlay shows a dictation on the screen: the pill (a slip of paper
// near the bottom of the screen, which can wait there idle and be dragged)
// or the notch (a black camera notch at the top center that opens wings
// while recording, with a paper slip under it for the live preview and
// notices). The desktop part (window, placement, click-through) is in the
// shell; this file draws it and decides when it shows.

// Overlay window sizes at Large (scale 1), in DIPs; Medium and Small zoom them down.
const (
	PillWindowW  = 460
	PillWindowH  = 150
	NotchWindowW = 520
	NotchWindowH = 170
)

type overlayState struct {
	previewBelow bool // the pill sits in the top half of its display
	visible      bool
}

// OverlayVisible is whether the overlay should show now.
func (a *App) OverlayVisible() bool {
	switch a.state.Kind {
	case dictation.StateRecording, dictation.StateProcessing, dictation.StateShowing:
		return true
	}
	if a.noticeShowing() {
		return true
	}
	return a.cfg.Overlay.Style != config.OverlayNotch && a.cfg.Overlay.IdlePill
}

// OverlayClickThrough is true while recording: clicks reach the app below.
func (a *App) OverlayClickThrough() bool { return a.state.Kind == dictation.StateRecording }

// SetPreviewBelow tells the pill which half of the display it is in.
func (a *App) SetPreviewBelow(below bool) { a.ov.previewBelow = below }

// syncOverlay shows or hides the overlay window for the state.
func (a *App) syncOverlay() {
	v := a.OverlayVisible()
	a.ov.visible = v
	if a.d.Shell != nil {
		a.d.Shell.ShowOverlay(v)
	}
}

// OverlayView is the view of the overlay window.
func (a *App) OverlayView(c *mu.Context) {
	p := a.look(c)
	c.Root().Background(mu.Color{})
	c.SetZoom(a.cfg.Overlay.Size.Scale())
	if a.cfg.Overlay.Style == config.OverlayNotch {
		a.notchView(c, p)
		return
	}
	a.pillView(c, p)
}

func (a *App) pillView(c *mu.Context, p theme.Colors) {
	col := mu.Column(c).Fill().AlignItems(mu.Center).Gap(8).Justify(mu.End).Padding(10)
	if a.ov.previewBelow {
		col.Justify(mu.Start)
	}
	col.Children(func() {
		if !a.ov.previewBelow {
			a.previewSlip(c, p)
		}
		a.pill(c, p)
		if a.ov.previewBelow {
			a.previewSlip(c, p)
		}
	})
}

// pill is the slip itself.
func (a *App) pill(c *mu.Context, p theme.Colors) {
	label := i18n.T("overlay.a11y.start")
	if a.Busy() {
		label = i18n.T("overlay.a11y.stop")
	}
	// The slip holds the pressable part and, for a notice, its actions as
	// siblings: a control inside a button is no control of its own.
	mu.Row(c).Padding(0, 6, 0, 0).Height(44).Gap(4).Radius(22).AlignItems(mu.Center).
		Background(p.SheetRaised).Border(1, p.Rule).
		Shadow(0, 2, 6, 0, p.Shadow.Alpha(0.12)).Shadow(0, 10, 26, -6, p.Shadow.Alpha(0.2)).DragWindow().Children(func() {
		b := mu.ButtonBase(a.k(c, "overlay.pill")).Label(label).Padding(0, 10, 0, 16).FillHeight().Gap(10).Radius(22).AlignItems(mu.Center)
		b.Children(func() { a.pillContent(c, p) })
		if clicked(c, b) {
			switch a.state.Kind {
			case dictation.StateShowing:
				if a.d.Dictation != nil {
					a.d.Dictation.Dismiss()
				}
			default:
				a.Toggle()
			}
		}
		if a.state.Kind == dictation.StateShowing {
			a.noticeActions(c, p)
		}
	})
}

func (a *App) pillContent(c *mu.Context, p theme.Colors) {
	switch a.state.Kind {
	case dictation.StateRecording:
		dot := mu.Box(c).Size(10, 10).Radius(99).Background(p.Accent)
		dot.Opacity(0.55 + 0.45*dot.Loop("breath", 1400*time.Millisecond, mu.EaseInOut))
		waveform(c, a.levelsInOrder()[16:], p.Ink).Width(120).Height(22)
		mu.Text(c, a.elapsedLabel()).Font(theme.Mono).FontSize(13).TextColor(p.Graphite)
		if a.incognito {
			mu.Icon(c, icon("eye")).FontSize(14).TextColor(p.Accent).Label(i18n.T("overlay.incognito"))
		}
	case dictation.StateProcessing:
		line := mu.Box(c).Width(60).Height(10)
		t := line.Loop("draw", 1100*time.Millisecond, mu.EaseInOut)
		line.Draw(func(pt *mu.Painter, r mu.Rect) {
			pt.Fill(mu.Rect{X: r.X, Y: r.Y + r.H/2 - 1, W: r.W * t, H: 2}, p.Ink, 1)
		})
		mu.Text(c, a.stageLabel()).FontSize(13).TextColor(p.Ink)
	case dictation.StateShowing:
		a.noticeContent(c, p)
	default:
		if a.noticeShowing() {
			mu.Icon(c, icon("info")).FontSize(15).TextColor(p.Graphite)
			mu.Text(c, a.lastNotice).FontSize(13).TextColor(p.Ink).SingleLine().Ellipsis("…").MaxWidth(300).Role(mu.RoleStatus)
			return
		}
		if h := a.cfg.Hotkeys.Toggle; h != nil {
			for _, k := range h.Keycaps() {
				keycap(c, p, k)
			}
		}
		mu.Text(c, i18n.T("overlay.to_dictate")).FontSize(13).TextColor(p.Graphite)
	}
}

// noticeContent: Inserted, Cancelled (with Undo), or a failure (with Copy).
func (a *App) noticeContent(c *mu.Context, p theme.Colors) {
	n := a.state.Notice
	switch n.Kind {
	case dictation.NoticeInserted:
		mu.Icon(c, icon("check")).FontSize(15).TextColor(p.Success)
		text := i18n.T("overlay.inserted")
		if n.StyleName != "" {
			text += " · " + n.StyleName
		}
		mu.Text(c, text).FontSize(13).TextColor(p.Ink)
	case dictation.NoticeInsertedRaw:
		mu.Icon(c, icon("warning")).FontSize(15).TextColor(p.Pencil)
		mu.Text(c, i18n.T("overlay.inserted_raw")).FontSize(13).TextColor(p.Ink)
	case dictation.NoticeCancelled, dictation.NoticeDiscarded:
		mu.Text(c, i18n.T("overlay.cancelled")).FontSize(13).TextColor(p.Graphite)
	case dictation.NoticeNoSpeech:
		mu.Text(c, i18n.T("overlay.error.no_speech")).FontSize(13).TextColor(p.Graphite)
	case dictation.NoticeInsertFailed:
		mu.Icon(c, icon("warning")).FontSize(15).TextColor(p.Danger)
		mu.Text(c, n.Reason).FontSize(13).TextColor(p.Ink).SingleLine().Ellipsis("…").MaxWidth(200)
	default:
		mu.Icon(c, icon("warning")).FontSize(15).TextColor(p.Danger)
		msg := n.Message
		if msg == "" {
			msg = a.lastNotice
		}
		mu.Text(c, msg).FontSize(13).TextColor(p.Ink).SingleLine().Ellipsis("…").MaxWidth(260)
	}
}

// noticeActions are a notice's buttons: Undo, Copy, Dismiss.
func (a *App) noticeActions(c *mu.Context, p theme.Colors) {
	n, ctl := a.state.Notice, a.d.Dictation
	if n.Kind == dictation.NoticeCancelled && a.btn(c, p, "overlay.undo", i18n.T("overlay.undo"), btnGhost) && ctl != nil {
		ctl.Undo()
	}
	if n.Kind == dictation.NoticeInsertFailed && a.btn(c, p, "overlay.copy", i18n.T("overlay.copy_text"), btnSecondary) {
		c.WriteClipboard(n.Text)
		if ctl != nil {
			ctl.Dismiss()
		}
	}
	if (n.Kind == dictation.NoticeInsertFailed || n.Kind == dictation.NoticeError) &&
		a.iconBtn(c, p, "overlay.dismiss", "close", i18n.T("overlay.dismiss")) && ctl != nil {
		ctl.Dismiss()
	}
}

// previewSlip is the live preview: committed words in ink, the tentative
// tail in pencil.
func (a *App) previewSlip(c *mu.Context, p theme.Colors) {
	if !a.cfg.Overlay.ShowPreview || a.state.Kind != dictation.StateRecording {
		return
	}
	mu.Box(c).MaxWidth(420).Padding(8, 12).Radius(10).Background(p.SheetRaised).Border(1, p.Rule).
		Shadow(0, 4, 14, 0, p.Shadow.Alpha(0.14)).Role(mu.RoleStatus).Children(func() {
		if a.committed == "" && a.tentative == "" {
			mu.Text(c, i18n.T("overlay.preview_loading")).FontSize(13).TextColor(p.Pencil)
			return
		}
		mu.RichText(c, mu.Span{Text: a.committed, Color: p.Ink}, mu.Span{Text: " " + a.tentative, Color: p.Pencil}).
			Font(theme.Body).FontSize(14).MaxLines(3)
	})
}

// notchView is the black notch with wings, and the slip under it.
func (a *App) notchView(c *mu.Context, p theme.Colors) {
	black := mu.RGB(0, 0, 0)
	mu.Column(c).Fill().AlignItems(mu.Center).Gap(6).Children(func() {
		open := a.Busy() || a.state.Kind == dictation.StateShowing
		w := float32(200)
		if open {
			w = 360
		}
		n := mu.ButtonBase(a.k(c, "overlay.notch")).Label(i18n.T("overlay.a11y.name")).Width(w).Height(34).
			Padding(0, 18).Gap(10).AlignItems(mu.Center).Background(black).Radius(0, 0, 16, 16)
		n.Children(func() {
			if !open {
				return
			}
			// Left wing: the state; right wing: the waveform or timer.
			mu.Row(c).Gap(8).Grow(1).AlignItems(mu.Center).Children(func() {
				switch a.state.Kind {
				case dictation.StateRecording:
					mu.Box(c).Size(8, 8).Radius(99).Background(p.Accent)
					mu.Text(c, a.elapsedLabel()).Font(theme.Mono).FontSize(12).TextColor(mu.RGB(235, 235, 235))
				case dictation.StateProcessing:
					mu.Text(c, a.stageLabel()).FontSize(12).TextColor(mu.RGB(235, 235, 235))
				}
			})
			if a.state.Kind == dictation.StateRecording {
				waveform(c, a.levelsInOrder()[28:], mu.RGB(245, 245, 245)).Width(90).Height(16)
			}
		})
		if clicked(c, n) {
			a.Toggle()
		}
		if a.state.Kind == dictation.StateIdle && a.noticeShowing() {
			mu.Text(c, a.lastNotice).FontSize(13).TextColor(p.Ink).Padding(8, 14).Radius(10).Background(p.SheetRaised).
				Border(1, p.Rule).Role(mu.RoleStatus)
		} else if a.state.Kind == dictation.StateShowing {
			mu.Row(c).Gap(10).Padding(8, 14).Radius(10).Background(p.SheetRaised).Border(1, p.Rule).AlignItems(mu.Center).
				Shadow(0, 4, 14, 0, p.Shadow.Alpha(0.14)).Children(func() {
				a.noticeContent(c, p)
				a.noticeActions(c, p)
			})
		} else {
			a.previewSlip(c, p)
		}
	})
}
