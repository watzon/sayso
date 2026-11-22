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

// pill is the slip itself. At rest it is a small dash; hovered, it says
// how to dictate; while dictating or showing a notice it holds them.
func (a *App) pill(c *mu.Context, p theme.Colors) {
	label := i18n.T("overlay.a11y.start")
	if a.Busy() {
		label = i18n.T("overlay.a11y.stop")
	}
	idle := a.state.Kind == dictation.StateIdle && !a.noticeShowing()
	// The slip holds the pressable part and, for a notice, its actions as
	// siblings: a control inside a button is no control of its own.
	shell := mu.Row(c).Gap(4).Radius(99).AlignItems(mu.Center).Background(p.SheetRaised).
		InsetShadow(0, 1, 0, 0, p.Highlight(1)).Shadow(0, 1, 2, 0, p.ShadowA(0.16)).Shadow(0, 4, 12, 0, p.ShadowA(0.12)).DragWindow()
	if !idle {
		shell.Height(44).Padding(0, 6, 0, 0)
	}
	shell.Children(func() {
		b := mu.ButtonBase(a.k(c, "overlay.pill")).Label(label).Radius(99).AlignItems(mu.Center).Justify(mu.Center)
		hover := b.Hovered()
		switch {
		case idle && !hover:
			b.Size(52, 14)
		case idle:
			b.Padding(7, 14, 7, 8).Gap(10)
		default:
			b.Padding(0, 10, 0, 16).FillHeight().Gap(10)
		}
		b.Children(func() {
			if idle {
				a.idleContent(c, p, hover)
				return
			}
			a.pillContent(c, p)
		})
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

// idleContent is the dash at rest, and the hotkey when hovered.
func (a *App) idleContent(c *mu.Context, p theme.Colors, hover bool) {
	if !hover {
		mu.Box(c).Size(18, 2).Radius(2).Background(p.Ink.Alpha(0.55))
		return
	}
	if h := a.cfg.Hotkeys.Toggle; h != nil {
		keycaps(c, p, h.Keycaps(), a.cfg.Overlay.Size.Scale() < 0.8)
	}
	uiText(c, i18n.T("overlay.to_dictate"), 14, 500, p.Graphite).SingleLine()
}

func (a *App) pillContent(c *mu.Context, p theme.Colors) {
	switch a.state.Kind {
	case dictation.StateRecording:
		dot := mu.Box(c).Size(10, 10).Radius(99).Background(p.Accent)
		dot.Opacity(0.55 + 0.45*dot.Loop("breath", 1400*time.Millisecond, mu.EaseInOut))
		waveform(c, a.levelsInOrder()[16:], p.Ink).Width(120).Height(22)
		mono(c, a.elapsedLabel(), 13, p.Graphite)
		if a.incognito {
			mu.Icon(c, icon("eye")).FontSize(14).TextColor(p.Accent).Label(i18n.T("overlay.incognito"))
		}
	case dictation.StateProcessing:
		line := mu.Box(c).Width(60).Height(10)
		t := line.Loop("draw", 1100*time.Millisecond, mu.EaseInOut)
		line.Draw(func(pt *mu.Painter, r mu.Rect) {
			pt.Fill(mu.Rect{X: r.X, Y: r.Y + r.H/2 - 1, W: r.W * t, H: 2}, p.Ink, 1)
		})
		uiText(c, a.stageLabel(), 14, 500, p.Ink)
	case dictation.StateShowing:
		a.noticeContent(c, p)
	default:
		if a.noticeShowing() {
			noticeIcon(c, p, "info", p.Ink)
			uiText(c, a.lastNotice, 14, 600, p.Ink).SingleLine().Ellipsis("…").MaxWidth(300).Role(mu.RoleStatus)
		}
	}
}

// noticeIcon is a notice's icon in a 28 px debossed circle.
func noticeIcon(c *mu.Context, p theme.Colors, name string, col mu.Color) {
	debossed(mu.Box(c).Size(28, 28).Shrink(0).Radius(99).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
		mu.Icon(c, icon(name)).FontSize(14).TextColor(col)
	})
}

// noticeContent: Inserted, Cancelled (with Undo), or a failure (with Copy).
func (a *App) noticeContent(c *mu.Context, p theme.Colors) {
	n := a.state.Notice
	title := func(s string) { uiText(c, s, 14, 600, p.Ink).SingleLine().Ellipsis("…").MaxWidth(260) }
	switch n.Kind {
	case dictation.NoticeInserted:
		noticeIcon(c, p, "check", p.Ink)
		text := i18n.T("overlay.inserted")
		if n.StyleName != "" {
			text += " · " + n.StyleName
		}
		title(text)
	case dictation.NoticeInsertedRaw:
		noticeIcon(c, p, "warning", p.Ink)
		title(i18n.T("overlay.inserted_raw"))
	case dictation.NoticeCancelled, dictation.NoticeDiscarded:
		noticeIcon(c, p, "close", p.Ink)
		title(i18n.T("overlay.cancelled"))
	case dictation.NoticeNoSpeech:
		noticeIcon(c, p, "mic", p.Ink)
		title(i18n.T("overlay.error.no_speech"))
	case dictation.NoticeInsertFailed:
		noticeIcon(c, p, "warning", p.Danger)
		title(n.Reason)
	default:
		noticeIcon(c, p, "warning", p.Danger)
		msg := n.Message
		if msg == "" {
			msg = a.lastNotice
		}
		title(msg)
	}
}

// noticeActions are a notice's buttons: Undo, Copy, Dismiss.
func (a *App) noticeActions(c *mu.Context, p theme.Colors) {
	n, ctl := a.state.Notice, a.d.Dictation
	if n.Kind == dictation.NoticeCancelled && a.button(c, p, "overlay.undo", i18n.T("overlay.undo"), bopt{kind: btnGhost, size: sizeSmall, icon: "undo"}) && ctl != nil {
		ctl.Undo()
	}
	if n.Kind == dictation.NoticeInsertFailed && a.button(c, p, "overlay.copy", i18n.T("overlay.copy_text"), bopt{size: sizeSmall, icon: "copy"}) {
		c.WriteClipboard(n.Text)
		if ctl != nil {
			ctl.Dismiss()
		}
	}
	if (n.Kind == dictation.NoticeInsertFailed || n.Kind == dictation.NoticeError) &&
		a.iconBtnSized(c, p, "overlay.dismiss", "close", i18n.T("overlay.dismiss"), 28, 14) && ctl != nil {
		ctl.Dismiss()
	}
}

// previewSlip is the live preview: committed words in ink, the tentative
// tail in pencil italic.
func (a *App) previewSlip(c *mu.Context, p theme.Colors) {
	if !a.cfg.Overlay.ShowPreview || a.state.Kind != dictation.StateRecording {
		return
	}
	raised(mu.Box(c).MaxWidth(420).Padding(14, 18).Radius(12), p).Background(p.Sheet).Role(mu.RoleStatus).Children(func() {
		if a.committed == "" && a.tentative == "" {
			mu.Text(c, i18n.T("overlay.preview_loading")).Font(theme.Body).Italic().FontSize(17).TextColor(p.Pencil)
			return
		}
		mu.RichText(c, mu.Span{Text: a.committed, Color: p.Ink}, mu.Span{Text: " " + a.tentative, Color: p.Pencil, Italic: true}).
			Font(theme.Body).FontSize(17).LineHeight(24.0 / 17).MaxLines(3)
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
