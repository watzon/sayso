package ui

import (
	"math"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The overlay shows a dictation on the screen: the pill (a slip of paper
// near the bottom of the screen, which can wait there idle and be dragged)
// or the notch (a black camera notch at the top center that opens wings
// while recording, with a paper slip under it for the live preview and
// notices). The desktop part (window, placement, click-through) is in the
// shell; this file draws it and decides when it shows. Its look is the
// Paper page "Overlay" and the Rust overlay.

// The overlay window at size Large (Rust overlay WIDTH and HEIGHT), in DIPs.
// The pill's window scales with the size; the notch's does not.
const (
	PillWindowW  = 520
	PillWindowH  = 300
	NotchWindowW = PillWindowW
	NotchWindowH = PillWindowH
	// overlayMargin is the room between the card and the window edge that
	// holds it, in window DIPs.
	overlayMargin = 16
)

// OverlayWindowSize is the size of the overlay window, in DIPs.
func OverlayWindowSize(o config.Overlay) (w, h int) {
	if o.Style == config.OverlayNotch {
		return NotchWindowW, NotchWindowH
	}
	s := o.Size.Scale()
	return int(float32(PillWindowW) * s), int(float32(PillWindowH) * s)
}

type overlayState struct {
	previewBelow bool // the pill sits in the top half of its display
	visible      bool
	ready        bool // the motions below are set up
	// pen writes the live preview out on the slip.
	pen pen
	// card is the visible card, which alone takes the mouse; cardShown is
	// whether this frame built it; region is the box the shell was last
	// given for it, in window DIPs.
	card      mu.Handle
	cardShown bool
	region    mu.Rect
	// text is the preview's paragraph, whose height says when it scrolls.
	text    mu.Handle
	hovered bool // the pointer is over the pill, which opens it
	// The motions (Rust overlay/motion.rs): the pill's card eases to the
	// size of each state, grows in and shrinks out; the slip grows out of
	// the pill (or feeds out of the notch) and goes back; the notch's
	// wings open from the camera and close into it.
	cardM, slipM morph
	notch        notchMotion
	// wings are the contents of the notch's left and right wings.
	wings [2]mu.Handle
	// slipPreview: the slip shows the last live preview while it goes back.
	slipPreview bool
	// phase names the state the card shows; phaseAt is the frame time it
	// began, from which its content fades in.
	phase   overlayPhaseKey
	phaseAt time.Time
	fade    float32
	// layout names what the card shows. A change asks for one more frame,
	// in which the motions measure the new content.
	layout overlayLayoutKey
	// probe logs frame gaps (a dev option); nil does nothing.
	probe *frameProbe
}

// setUp makes the motions at rest and away.
func (o *overlayState) setUp() {
	if o.ready {
		return
	}
	o.ready = true
	o.cardM, o.slipM, o.notch = newMorph(exitShrink), newMorph(exitPoint), newNotchMotion()
}

// overlayInput is a shell that lets the mouse through the overlay window
// outside the card.
type overlayInput interface {
	SetOverlayInputRegion(r mu.Rect)
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
	if !a.closesWhenIdle() {
		return true
	}
	// The card closes with motion; the window hides at its end.
	return a.overlayClosing()
}

// closesWhenIdle is true when the card goes away while no dictation runs:
// the notch, or no idle pill.
func (a *App) closesWhenIdle() bool {
	return a.cfg.Overlay.Style == config.OverlayNotch || !a.cfg.Overlay.IdlePill
}

// overlayClosing is true while the card still closes.
func (a *App) overlayClosing() bool {
	if a.cfg.Overlay.Style == config.OverlayNotch {
		return !a.ov.notch.gone()
	}
	return !a.ov.cardM.gone()
}

// overlayAnimates is true in the states that draw motion: the waveform and
// pulse, the ink line of the processing stage, and the countdown of the
// undo. The other notices are still, so the overlay draws them once and not
// on every frame (Rust overlay animates).
func (a *App) overlayAnimates() bool {
	switch a.state.Kind {
	case dictation.StateRecording, dictation.StateProcessing:
		return true
	case dictation.StateShowing:
		return a.state.Notice.Kind == dictation.NoticeCancelled
	}
	return false
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

// OverlayView is the view of the overlay window. While something moves it
// asks for a frame each display refresh (AnimationFrame); at rest it asks
// for none.
func (a *App) OverlayView(c *mu.Context) {
	a.ov.setUp()
	if a.ov.probe != nil {
		a.ov.probe.frame(c.Now(), time.Now(), a.state.Kind == dictation.StateRecording)
	}
	p := a.look(c)
	c.Root().Background(mu.Color{})
	if a.overlayAnimates() {
		c.AnimationFrame()
	}
	a.overlayPhase(c)
	a.wave.advance(c.Now())
	a.writePreview(c)
	a.ov.cardShown = false
	if a.cfg.Overlay.Style == config.OverlayNotch {
		c.SetZoom(1)
		a.ov.slipM.exit = exitFold
		a.notchView(c, p)
	} else {
		c.SetZoom(a.cfg.Overlay.Size.Scale())
		a.ov.slipM.exit = exitPoint
		a.pillView(c, p)
	}
	if key := a.overlayLayout(); key != a.ov.layout {
		a.ov.layout = key
		c.AnimationFrame()
	}
	a.followCard(c)
	// The card has closed: hide the window until the next dictation.
	if a.ov.visible && !a.OverlayVisible() {
		a.post(a.syncOverlay)
	}
}

// overlayLayoutKey names what the card shows. It is a value, not a string,
// because OverlayView makes one in every frame. set tells the first key
// from the zero value, as the first frame differs from no frame.
type overlayLayoutKey struct {
	set           bool
	kind          dictation.StateKind
	stage         dictation.Stage
	notice        dictation.NoticeKind
	styleName     string
	hovered       bool
	lastNotice    string
	noticeShowing bool
	incognito     bool
	size          config.OverlaySize
	style         config.OverlayStyle
}

// overlayLayout names what the card shows, for OverlayView.
func (a *App) overlayLayout() overlayLayoutKey {
	return overlayLayoutKey{true, a.state.Kind, a.state.Stage, a.state.Notice.Kind, a.state.Notice.StyleName, a.ov.hovered, a.lastNotice,
		a.noticeShowing(), a.incognito, a.cfg.Overlay.Size, a.cfg.Overlay.Style}
}

// overlayPhaseKey names the state the card shows.
type overlayPhaseKey struct {
	set           bool
	kind          dictation.StateKind
	stage         dictation.Stage
	notice        dictation.NoticeKind
	noticeShowing bool
}

// overlayPhase notes when the card began to show its state, and how far
// its content has faded in (Rust FADE_IN).
func (a *App) overlayPhase(c *mu.Context) {
	key := overlayPhaseKey{true, a.state.Kind, a.state.Stage, a.state.Notice.Kind, a.noticeShowing()}
	if key != a.ov.phase {
		a.ov.phase, a.ov.phaseAt = key, c.Now()
	}
	a.ov.fade = fade(float32(c.Now().Sub(a.ov.phaseAt).Seconds()), stillMotion(c))
	if a.ov.fade < 1 {
		c.AnimationFrame()
	}
}

// reduceMotion is the system's setting for less motion.
func reduceMotion(c *mu.Context) bool { return c.Preferences().ReduceMotion }

// stillMotion is true when the overlay's motions show their end at once:
// with reduce motion, and in frames that disable animations.
func stillMotion(c *mu.Context) bool { return reduceMotion(c) || c.Instant() }

// seconds is the clock of the overlay's ink (the ripple, the pulse), or 0
// with reduce motion.
func seconds(c *mu.Context) float32 {
	if reduceMotion(c) {
		return 0
	}
	return float32(c.Now().UnixMilli()%1_000_000) / 1000
}

// ts is a text size at Large in the zoomed pill: text shrinks half as much
// as shapes (Rust OverlaySize::text_scale).
func (a *App) ts(v float32) float32 {
	if a.cfg.Overlay.Style == config.OverlayNotch {
		return v
	}
	s := a.cfg.Overlay.Size
	return v * s.TextScale() / s.Scale()
}

// livePreview is true while the slip has words to show.
func (a *App) livePreview() bool {
	return a.state.Kind == dictation.StateRecording && a.cfg.Overlay.ShowPreview && (a.committed != "" || a.tentative != "")
}

// writePreview gives the pen the live preview and moves it on; a recording
// with no preview yet starts a new text (Rust OverlayView::write).
func (a *App) writePreview(c *mu.Context) {
	switch {
	case a.livePreview():
		a.ov.pen.set(a.committed, a.tentative)
	case a.state.Kind == dictation.StateRecording:
		a.ov.pen.clear()
	}
	if a.ov.pen.update(c.Now(), reduceMotion(c)) {
		c.AnimationFrame()
	}
}

// followCard gives the shell the card's box, so that the rest of the window
// lets the mouse through (Rust overlay: only the visible card takes it).
// The box is the one the last frame laid out; while the card moves, each
// frame moves the region with it, and a change asks for one more frame so
// the region catches the card where it stops.
func (a *App) followCard(c *mu.Context) {
	var r mu.Rect
	if a.ov.cardShown {
		r = a.ov.card.Bounds(c)
		z := c.Zoom()
		r = mu.Rect{X: r.X * z, Y: r.Y * z, W: r.W * z, H: r.H * z}
	}
	if r == a.ov.region {
		return
	}
	a.ov.region = r
	if s, ok := a.d.Shell.(overlayInput); ok {
		s.SetOverlayInputRegion(r)
	}
	c.AnimationFrame()
}

func (a *App) pillView(c *mu.Context, p theme.Colors) {
	k := a.cfg.Overlay.Size.Scale()
	// The slip and the pill sit at the window edge nearer the display edge;
	// the slip grows away from it, so the pill never moves. The slip is the
	// first child, so the pill draws over it and the slip comes out from
	// behind the pill; a reversed column puts it under the pill.
	col := mu.Column(c).Fill().AlignItems(mu.Center).Justify(mu.End).Padding(0, 0, overlayMargin/k, 0)
	if a.ov.previewBelow {
		col.Reverse().Padding(overlayMargin/k, 0, 0, 0)
	}
	col.Children(func() {
		a.previewSlip(c, p)
		a.pill(c, p)
	})
}

// cardPresent is true when the pill's card shows something: not at rest
// when the card closes while idle, and not for a discarded dictation.
func (a *App) cardPresent() bool {
	switch a.state.Kind {
	case dictation.StateIdle:
		return a.noticeShowing() || !a.closesWhenIdle()
	case dictation.StateShowing:
		return a.state.Notice.Kind != dictation.NoticeDiscarded
	}
	return true
}

// pill is the card itself. At rest it is a small dash; hovered, it says
// how to dictate; while dictating or showing a notice it holds them. The
// card eases to the size of each state, and the new state fades in.
func (a *App) pill(c *mu.Context, p theme.Colors) {
	present := a.cardPresent()
	failed := a.state.Kind == dictation.StateShowing && a.state.Notice.Kind == dictation.NoticeInsertFailed
	idle := a.state.Kind == dictation.StateIdle && !a.noticeShowing()
	var content func()
	if present {
		content = func() {
			mu.Row(c).Opacity(a.ov.fade).AlignItems(mu.Center).Children(func() {
				if failed {
					mu.Column(c).Width(320).Padding(14, 0, 12, 0).PaddingStart(16).PaddingEnd(14).Gap(10).Role(mu.RoleStatus).Children(func() { a.failedBody(c, p) })
					return
				}
				a.pillFace(c, p, idle)
			})
		}
	} else {
		a.ov.hovered = false
	}
	// The card holds the pressable part and, for a notice, its actions as
	// siblings: a control inside a button is no control of its own.
	card := a.morphBox(c, "overlay.card", &a.ov.cardM, present, alignCenter, content).Bind(&a.ov.card).DragWindow()
	a.ov.cardShown = !a.ov.cardM.gone()
	// The card's look follows the hover of the part inside it.
	switch {
	case failed:
		floating(card.Radius(14), p)
	case idle && present && !a.ov.hovered:
		card.Radius(99).Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).
			Shadow(0, 1, 2, 0, p.ShadowA(0.16)).Shadow(0, 4, 12, 0, p.ShadowA(0.12))
	case idle && present:
		card.Radius(99).Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).
			Shadow(0, 1, 2, 0, p.ShadowA(0.16)).Shadow(0, 8, 22, 0, p.ShadowA(0.14))
	default:
		floating(card.Radius(99), p)
	}
}

// pillFace is the pressable pill and, for a cancelled dictation, its Undo.
func (a *App) pillFace(c *mu.Context, p theme.Colors, idle bool) {
	label := i18n.T("overlay.a11y.start")
	if a.Busy() {
		label = i18n.T("overlay.a11y.stop")
	}
	b := mu.ButtonBase(a.k(c, "overlay.pill")).Label(label).Radius(99).AlignItems(mu.Center).Justify(mu.Center)
	hover := b.Hovered()
	a.ov.hovered = hover
	switch {
	case idle && !hover:
		b.Size(52, 14)
	case idle:
		b.Padding(7, 0, 7, 0).PaddingStart(8).PaddingEnd(14).Gap(10)
	}
	b.Children(func() {
		if idle {
			a.idleContent(c, p, hover)
			return
		}
		a.pillContent(c, p, b, hover)
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
	if a.state.Kind == dictation.StateShowing && a.state.Notice.Kind == dictation.NoticeCancelled {
		a.undoButton(c, p)
	}
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
	uiText(c, i18n.T("overlay.to_dictate"), a.ts(14), 500, p.Graphite).SingleLine()
}

// pillContent is what the pill holds in each state; b is the pressable
// part, which takes the padding of the state.
func (a *App) pillContent(c *mu.Context, p theme.Colors, b mu.Element, hover bool) {
	switch a.state.Kind {
	case dictation.StateRecording:
		b.Padding(10, 0, 10, 0).PaddingStart(10).PaddingEnd(18).Gap(14)
		debossed(mu.Box(c).Size(36, 36).Shrink(0).Radius(99).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
			if hover {
				// A click anywhere on the pill stops, so hover shows a stop button.
				mu.Box(c).Size(12, 12).Radius(3).Background(p.Accent)
				return
			}
			pulse := float32(0)
			if !reduceMotion(c) {
				pulse = float32(math.Sin(float64(seconds(c))*2.4))*0.5 + 0.5
			}
			mu.Box(c).Size(12, 12).Radius(99).Background(p.Accent).
				Shadow(0, 0, 0, 3+2*pulse, p.Accent.Alpha(0.16+0.1*pulse)).Shadow(0, 1, 1, 0, mu.RGBA(0x14, 0x18, 0x3C, 0.35))
		})
		inkWaveform(c, a.wave.points(28), seconds(c), p.Ink).Size(232, 36).Shrink(0)
		mono(c, a.elapsedLabel(), a.ts(13), p.Graphite).Width(a.ts(36)).TextAlign(mu.End)
		if a.incognito {
			debossed(mu.Row(c).Height(22).PaddingX(9).Shrink(0).Radius(99).AlignItems(mu.Center), p).Children(func() {
				uiText(c, i18n.T("overlay.incognito"), a.ts(12), 500, p.Graphite).SingleLine()
			})
		}
	case dictation.StateProcessing:
		b.Padding(10, 18).Gap(12)
		inkLine(c, processingProgress(float32(a.now().Sub(a.stateAt).Seconds()), reduceMotion(c)), p.Ink, p.Pencil).Size(150, 36).Shrink(0)
		mu.Text(c, a.stageLabel()).Font(serifFont(theme.Body)).Italic().FontSize(a.ts(16)).LineHeight(20.0 / 16).TextColor(p.Graphite).SingleLine()
	case dictation.StateShowing:
		a.noticeContent(c, p, b)
	default:
		if a.noticeShowing() {
			a.noticePill(c, p, b, "info", a.lastNotice, "")
		}
	}
}

// noticePill is a notice on the card: an icon in a debossed well, a title,
// and a detail under it.
func (a *App) noticePill(c *mu.Context, p theme.Colors, b mu.Element, iconName, title, detail string) {
	b.Padding(8, 0, 8, 0).PaddingStart(8).PaddingEnd(18).Gap(10)
	debossed(mu.Box(c).Size(28, 28).Shrink(0).Radius(99).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
		mu.Icon(c, dirIcon(c, iconName)).FontSize(14).TextColor(p.Ink)
	})
	mu.Column(c).Gap(1).Shrink(1).Children(func() {
		uiText(c, title, a.ts(14), 600, p.Ink).SingleLine().Ellipsis("…").MaxWidth(320).Role(mu.RoleStatus)
		if detail != "" {
			uiText(c, detail, a.ts(13), 400, p.Graphite).SingleLine().Ellipsis("…").MaxWidth(320)
		}
	})
}

// noticeContent: Inserted, Inserted raw, Cancelled, No speech, or an error.
func (a *App) noticeContent(c *mu.Context, p theme.Colors, b mu.Element) {
	n := a.state.Notice
	switch n.Kind {
	case dictation.NoticeInserted:
		b.Padding(8, 0, 8, 0).PaddingStart(8).PaddingEnd(18).Gap(10)
		if n.StyleName != "" {
			b.Padding(8, 0, 8, 0).PaddingStart(8).PaddingEnd(10)
		}
		inkSeal(c, p, 28)
		uiText(c, i18n.T("overlay.inserted"), a.ts(14), 600, p.Ink).SingleLine().Role(mu.RoleStatus)
		if n.StyleName != "" {
			a.overlayStyleTag(c, p, n.StyleName)
		}
	case dictation.NoticeInsertedRaw:
		a.noticePill(c, p, b, "check", i18n.T("overlay.inserted_raw"), n.Reason)
	case dictation.NoticeCancelled, dictation.NoticeDiscarded:
		b.Padding(8, 0, 8, 0).PaddingStart(18).Gap(12)
		uiText(c, i18n.T("overlay.cancelled"), a.ts(14), 500, p.Graphite).SingleLine().Role(mu.RoleStatus)
	case dictation.NoticeNoSpeech:
		a.noticePill(c, p, b, "info", i18n.T("overlay.error.no_speech"), "")
	default:
		msg := n.Message
		if msg == "" {
			msg = a.lastNotice
		}
		a.noticePill(c, p, b, "info", msg, "")
	}
}

// inkSeal is the indigo seal of an inserted dictation: a domed stamp with a check.
func inkSeal(c *mu.Context, p theme.Colors, size float32) {
	mu.Box(c).Size(size, size).Shrink(0).Radius(99).Background(p.Accent).Justify(mu.Center).AlignItems(mu.Center).
		InsetShadow(0, 1, 0, 0, white.Alpha(0.25)).InsetShadow(0, -2, 3, 0, mu.RGBA(0x0A, 0x0E, 0x32, 0.35)).
		Shadow(0, 1, 2, 0, mu.RGBA(0x14, 0x18, 0x3C, 0.30)).Children(func() {
		mu.Icon(c, icon("check")).FontSize(size / 2).TextColor(p.OnInk)
	})
}

// overlayStyleTag is the style of a dictation in a debossed tag with its ink dot.
func (a *App) overlayStyleTag(c *mu.Context, p theme.Colors, name string) {
	mu.Row(c).Height(24).PaddingX(10).Gap(6).Shrink(0).Radius(99).AlignItems(mu.Center).Background(p.Deboss).
		InsetShadow(0, 1, 2, 0, p.ShadowA(0.18)).InsetShadow(0, -1, 0, 0, p.Highlight(0.8)).Children(func() {
		mu.Box(c).Size(6, 6).Shrink(0).Radius(99).Background(p.Ink)
		uiText(c, name, a.ts(13), 500, p.Graphite).SingleLine().Ellipsis("…").MaxWidth(140)
	})
}

// undoWindow is how long a cancelled dictation can be brought back.
const undoWindow = 5 * time.Second

// undoButton is the ink Undo of a cancelled dictation, with a ring that
// empties as the time to undo runs out.
func (a *App) undoButton(c *mu.Context, p theme.Colors) {
	left := 1 - float32(a.now().Sub(a.stateAt))/float32(undoWindow)
	b := mu.ButtonBase(a.k(c, "overlay.undo")).Height(30).PaddingStart(10).PaddingEnd(12).MarginStart(12).MarginEnd(8).Gap(8).Radius(99).
		AlignItems(mu.Center).Cursor(mu.CursorPointer)
	inkButton(b, p, b.Pressed()).Children(func() {
		countdownRing(c, left, p.OnInk).Size(18, 18).Shrink(0)
		uiText(c, i18n.T("overlay.undo"), a.ts(14), 600, p.OnInk).SingleLine()
	})
	if clicked(c, b) && a.d.Dictation != nil {
		a.d.Dictation.Undo()
	}
}

// failedBody is the reason a dictation could not go in, its text, and
// Dismiss and Copy.
func (a *App) failedBody(c *mu.Context, p theme.Colors) {
	n, ctl := a.state.Notice, a.d.Dictation
	uiText(c, n.Reason, a.ts(13), 600, p.Danger)
	mu.Text(c, n.Text).Font(serifFont(theme.Body)).FontSize(a.ts(15)).LineHeight(21.0 / 15).TextColor(p.Ink).MaxLines(4).Ellipsis("…")
	mu.Row(c).Gap(6).Justify(mu.End).Children(func() {
		if a.button(c, p, "overlay.dismiss", i18n.T("overlay.dismiss"), bopt{size: sizeSmall}) && ctl != nil {
			ctl.Dismiss()
		}
		if a.button(c, p, "overlay.copy", i18n.T("overlay.copy_text"), bopt{kind: btnPrimary, size: sizeSmall}) {
			c.WriteClipboard(n.Text)
			if ctl != nil {
				ctl.Dismiss()
			}
		}
	})
}

// The live preview slip: at most three lines, the latest at the bottom.
const (
	slipLines = 3
	// slipLetters is the number of committed letters the pill's slip keeps
	// at Large (Rust: 150·k); the notch's keeps notchSlipLetters.
	slipLetters      = 150
	notchSlipLetters = 110
)

// previewLoading is true while the model of the live preview loads.
func (a *App) previewLoading() bool {
	if !a.cfg.Overlay.ShowPreview || !a.cfg.Dictation.LivePreview {
		return false
	}
	id, _ := a.previewModel()
	return id != "" && a.modelStatus(id).State == engine.StateOptimizing
}

// slipGap is the room between the pill and its slip.
const slipGap = 12

// previewSlip is the live preview on the pill's slip. It grows out of the
// center of the pill to its place, and goes back into the pill the same way
// when the recording stops, with the last words on it.
func (a *App) previewSlip(c *mu.Context, p theme.Colors) {
	live := a.livePreview()
	loading := a.state.Kind == dictation.StateRecording && !live && a.previewLoading()
	present := live || loading
	if present {
		a.ov.slipPreview = live
	}
	m := &a.ov.slipM
	if !present && m.gone() {
		a.ov.slipPreview = false
		return
	}
	k := a.cfg.Overlay.Size.Scale()
	var content func()
	switch {
	case live || (!present && a.ov.slipPreview):
		content = func() {
			mu.Column(a.k(c, "overlay.preview")).Width(420).Padding(14, 18).Role(mu.RoleStatus).Children(func() {
				a.inkText(c, p, previewSpans(&a.ov.pen, int(slipLetters*k), p), 384, a.ts(17), 24.0/17)
			})
		}
	case loading:
		content = func() {
			mu.Column(a.k(c, "overlay.preview")).Width(420).Padding(12, 18).Role(mu.RoleStatus).Children(func() {
				loadingLine(c, p, a.ts(15), 21)
			})
		}
	}
	wrap := mu.Box(c).Shrink(0)
	wrap.Children(func() {
		a.morphBox(c, "overlay.slip", m, present, alignCenter, content).Radius(12).Background(p.Sheet).
			InsetShadow(0, 1, 0, 0, p.Highlight(0.9)).Shadow(0, 1, 2, 0, p.ShadowA(0.10)).Shadow(0, 10, 28, 0, p.ShadowA(0.12))
	})
	// How far the slip is from the center of the pill: all the way at the
	// start, none at its place.
	_, pill := a.ov.cardM.size()
	_, h := m.size()
	away := (1 - m.opacity()) * (slipGap + (pill+h)/2)
	if a.ov.previewBelow {
		wrap.Padding(slipGap, 0, 0, 0).Translate(0, -away)
	} else {
		wrap.Padding(0, 0, slipGap, 0).Translate(0, away)
	}
	if m.gone() {
		a.ov.slipPreview = false
	}
}

// loadingLine is a pencil line that draws across, and the reason in pencil
// italic: the slip of a live preview whose model still loads.
func loadingLine(c *mu.Context, p theme.Colors, size, line float32) {
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		inkLine(c, processingProgress(seconds(c), reduceMotion(c)), p.Pencil, p.Pencil.Alpha(0.5)).Size(34, 10).Shrink(0)
		mu.Text(c, i18n.T("overlay.preview_loading")).Font(serifFont(theme.Body)).Italic().FontSize(size).LineHeight(line / 15).TextColor(p.Pencil)
	})
}

// inkText is the preview at a size: the pen's spans in one paragraph of
// width w, with the caret drawn after them. Past three lines the oldest
// slide up under the top of the slip and fade, so the latest words always
// show and the slip keeps its height.
func (a *App) inkText(c *mu.Context, p theme.Colors, spans []mu.Span, w, size, lineHeight float32) {
	for i := range spans {
		spans[i].Font, spans[i].Size = serifFont(theme.Body), size
	}
	// The caret: a 1.5 px indigo bar standing on the baseline, 5 px after the
	// tail (Paper); spans ends with Rust's caret glyph, whose color it takes.
	caret := spans[len(spans)-1].Color
	spans[len(spans)-1] = mu.Span{Text: "￼", ObjectWidth: 1.5, ObjectHeight: size * 18 / 17 * 0.8, ObjectDescent: size * 18 / 17 * 0.2,
		PaddingStart: size * 5 / 17}
	line := size * lineHeight
	// A reversed column stands the text on its bottom edge: lines past
	// three go up, out of the top, and the top line fades out as it goes.
	// The text's height is the one of the last frame.
	lines := int(a.ov.text.Bounds(c).H/line + 0.5)
	box := mu.Column(c).Width(w).Reverse().MaxHeight(slipLines * line).ClipY()
	if lines > slipLines {
		box.MaskLinearGradient(mu.LinearGradient{From: mu.RGBA(0, 0, 0, 0), To: mu.RGB(0, 0, 0), Angle: 180, End: 0.6 / slipLines})
	}
	box.Children(func() {
		mu.RichText(c, spans...).Bind(&a.ov.text).Width(w).Shrink(0).Font(serifFont(theme.Body)).FontSize(size).LineHeight(lineHeight).DrawObjects(func(pt *mu.Painter, _ int, r mu.Rect) {
			pt.Fill(r, caret, 1)
		})
	})
}

// notchOpens is true when the notch opens its wings for the state.
func (a *App) notchOpens() bool {
	switch a.state.Kind {
	case dictation.StateIdle:
		return a.noticeShowing()
	case dictation.StateShowing:
		return a.state.Notice.Kind != dictation.NoticeDiscarded
	}
	return true
}

// notchView is the black notch with wings, and the slip under it (Paper
// page "Overlay · Notch"). The wings open from the camera and close into
// it; closed and at rest, nothing shows and the window hides.
func (a *App) notchView(c *mu.Context, p theme.Colors) {
	night := a.themes.get(a.cfg.Appearance.Ink, true).Colors
	open := a.notchOpens()
	// Both wings have the width of the wider content, so the middle stays
	// on the camera; a wing grows to fit a longer label.
	l, r := a.ov.wings[0].Bounds(c).W, a.ov.wings[1].Bounds(c).W
	wing := max(max(l, r)+notchWingPad, notchWing)
	if a.ov.notch.update(c.Now(), open, wing, stillMotion(c)) {
		c.AnimationFrame()
	}
	if a.ov.notch.gone() {
		return
	}
	a.ov.cardShown = true
	ww := a.ov.notch.width()
	mu.Column(c).Fill().AlignItems(mu.Center).Children(func() {
		mu.Column(a.k(c, "overlay.card")).Bind(&a.ov.card).AlignItems(mu.Center).Shrink(0).Opacity(a.ov.notch.opacity()).Children(func() {
			n := mu.ButtonBase(a.k(c, "overlay.notch")).Label(i18n.T("overlay.a11y.name")).Height(notchHeight).PaddingX(notchShoulder).
				AlignItems(mu.Center).Draw(paintNotch)
			n.Children(func() {
				hover := n.Hovered()
				a.notchWing(c, 0, ww, func() { a.leftWing(c, night, hover) })
				mu.Box(c).Width(notchCamera).Shrink(0)
				a.notchWing(c, 1, ww, func() { a.rightWing(c, night) })
			})
			if clicked(c, n) {
				a.Toggle()
			}
			a.notchSlip(c, p)
		})
	})
}

// notchWing is one wing at width w. It clips its content, which sits at
// its outer edge, so the content slides out from under the camera. The
// padding closes with the wing, so a closed notch has the camera's size.
func (a *App) notchWing(c *mu.Context, side int, w float32, content func()) {
	pad := min(float32(notchWingPad), w)
	wing := mu.Row(c).Width(w).Height(notchHeight).Shrink(0).ClipX().AlignItems(mu.Center)
	if side == 0 {
		wing.PaddingStart(pad).Justify(mu.Start)
	} else {
		wing.PaddingEnd(pad).Justify(mu.End)
	}
	wing.Children(func() {
		mu.Row(c).Bind(&a.ov.wings[side]).Shrink(0).Gap(7).AlignItems(mu.Center).Children(content)
	})
}

// The notch at the size of a MacBook's camera housing, with its wings.
const (
	notchCamera   = 185
	notchWing     = 76
	notchShoulder = 5
	notchWingPad  = 13
	notchRadius   = 12
	notchHeight   = 32
	notchWidth    = notchCamera + 2*notchWing + 2*notchShoulder
	// notchSlipW is the slip under the notch: 15 narrower on each side.
	notchSlipW = notchWidth - 30
)

// paintNotch draws the black notch, with concave shoulders at the top edge
// of the display and round bottom corners.
func paintNotch(pt *mu.Painter, r mu.Rect) {
	x, y, w, h := r.X, r.Y, r.W, r.H
	const s, rad = notchShoulder, notchRadius
	var path mu.Path
	path.MoveTo(x, y).LineTo(x+w, y).QuadTo(x+w-s, y, x+w-s, y+s).LineTo(x+w-s, y+h-rad).QuadTo(x+w-s, y+h, x+w-s-rad, y+h).
		LineTo(x+s+rad, y+h).QuadTo(x+s, y+h, x+s, y+h-rad).LineTo(x+s, y+s).QuadTo(x+s, y, x, y).Close()
	pt.FillPath(&path, mu.RGB(0, 0, 0))
}

// leftWing is the state, in the colors of dark paper.
func (a *App) leftWing(c *mu.Context, night theme.Colors, hover bool) {
	switch a.state.Kind {
	case dictation.StateRecording:
		if hover {
			mu.Box(c).Size(9, 9).Shrink(0).Radius(2.5).Background(night.Accent)
		} else {
			mu.Box(c).Size(8, 8).Shrink(0).Radius(99).Background(night.Accent).
				Shadow(0, 0, 0, 3, night.Accent.Alpha(0.18)).Shadow(0, 0, 8, 0, night.Accent.Alpha(0.55))
		}
		mono(c, a.elapsedLabel(), 12, night.Graphite)
	case dictation.StateProcessing:
		mu.Text(c, a.stageLabel()).Font(serifFont(theme.Body)).Italic().FontSize(13).TextColor(night.Graphite).SingleLine()
	case dictation.StateShowing:
		switch a.state.Notice.Kind {
		case dictation.NoticeInserted:
			inkSeal(c, night, 20)
		case dictation.NoticeDiscarded:
		default:
			mu.Icon(c, icon("info")).FontSize(14).TextColor(night.Ink)
		}
	default:
		if a.noticeShowing() {
			mu.Icon(c, icon("info")).FontSize(14).TextColor(night.Ink)
		}
	}
}

// rightWing is the ink of the recording or the line of the processing stage.
func (a *App) rightWing(c *mu.Context, night theme.Colors) {
	switch a.state.Kind {
	case dictation.StateRecording:
		paleWaveform(c, a.wave.points(28), seconds(c), night.Ink).Size(50, 20).Shrink(0)
	case dictation.StateProcessing:
		inkLine(c, processingProgress(float32(a.now().Sub(a.stateAt).Seconds()), reduceMotion(c)), night.Ink, night.Pencil).Size(58, 12).Shrink(0)
	}
}

// paleWaveform is the waveform in the notch: pale ink with denser layers.
func paleWaveform(c *mu.Context, levels []float32, phase float32, col mu.Color) mu.Element {
	return mu.Box(c).Draw(mirrored(rtl(c), func(pt *mu.Painter, r mu.Rect) { paintInkLayers(pt, r, levels, phase, col, notchLayers) }))
}

// notchSlip is the slip under the notch: the live preview, or a notice. It
// feeds out of the notch and back in, so only its bottom corners are
// round, and the notch shades its top.
func (a *App) notchSlip(c *mu.Context, p theme.Colors) {
	live, notice := a.livePreview(), a.state.Kind == dictation.StateShowing || (a.state.Kind == dictation.StateIdle && a.noticeShowing())
	if a.state.Kind == dictation.StateShowing && a.state.Notice.Kind == dictation.NoticeDiscarded {
		notice = false
	}
	loading := a.state.Kind == dictation.StateRecording && !live && a.previewLoading()
	present := live || notice || loading
	if present {
		// A notice has its own slip: when it goes, the slip feeds back in
		// empty, not with an old preview.
		a.ov.slipPreview = live
	}
	m := &a.ov.slipM
	if !present && m.gone() {
		a.ov.slipPreview = false
		return
	}
	var content func()
	if present || a.ov.slipPreview {
		content = func() {
			slip := mu.Column(a.k(c, "overlay.preview")).Width(notchSlipW).Padding(13, 18, 15, 18).Role(mu.RoleStatus)
			slip.Children(func() {
				switch {
				case live || !present:
					a.inkText(c, p, previewSpans(&a.ov.pen, notchSlipLetters, p), notchSlipW-36, 16, 23.0/16)
				case loading:
					loadingLine(c, p, 15, 21)
				case a.state.Kind == dictation.StateShowing && a.state.Notice.Kind == dictation.NoticeInsertFailed:
					slip.Gap(10)
					a.failedBody(c, p)
				case a.state.Kind == dictation.StateShowing:
					mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
						uiText(c, a.notchNoticeTitle(), 14, 600, p.Ink).Grow(1).Shrink(1).SingleLine().Ellipsis("…")
						if a.state.Notice.Kind == dictation.NoticeCancelled {
							a.undoButton(c, p)
						}
					})
				default:
					uiText(c, a.lastNotice, 14, 600, p.Ink)
				}
			})
		}
	}
	a.morphBox(c, "overlay.slip", m, present, alignBottom, content).Radius(0, 0, 12, 12).Background(p.Sheet).
		InsetShadow(0, 7, 6, -6, p.Ink.Alpha(0.28)).Shadow(0, 1, 2, 0, p.ShadowA(0.14)).Shadow(0, 10, 24, 0, p.ShadowA(0.14))
	if m.gone() {
		a.ov.slipPreview = false
	}
}

// notchNoticeTitle is the title of a notice on the notch's slip.
func (a *App) notchNoticeTitle() string {
	n := a.state.Notice
	switch n.Kind {
	case dictation.NoticeInserted:
		if n.StyleName != "" {
			return i18n.T("overlay.inserted") + " · " + n.StyleName
		}
		return i18n.T("overlay.inserted")
	case dictation.NoticeInsertedRaw:
		return i18n.T("overlay.inserted_raw")
	case dictation.NoticeCancelled, dictation.NoticeDiscarded:
		return i18n.T("overlay.cancelled")
	case dictation.NoticeNoSpeech:
		return i18n.T("overlay.error.no_speech")
	}
	if n.Message != "" {
		return n.Message
	}
	return a.lastNotice
}
