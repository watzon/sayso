package ui

import (
	"image"
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
)

// macBar is the room the traffic lights take in a hub with a hidden title
// bar on macOS: the top left, in either direction.
var macBar = mu.TitleBar{Height: 28, Left: 78, Bottom: 28}

// forceDirection lays the interface out in d ("ltr" or "rtl") for the test,
// as an Arabic or English interface would be.
func forceDirection(t *testing.T, d string) {
	t.Helper()
	i18n.OverrideDirection(d)
	t.Cleanup(func() { i18n.OverrideDirection("") })
}

func roleBounds(t *testing.T, tt *mu.Tester, r mu.Role) mu.Rect {
	t.Helper()
	for _, a := range tt.AccessAll() {
		if a.Role == r {
			return a.Bounds
		}
	}
	t.Fatalf("nothing with role %v", r)
	return mu.Rect{}
}

func center(r mu.Rect) float32 { return r.X + r.W/2 }

func TestHubSidebarIsOnTheStartSide(t *testing.T) {
	for _, d := range []string{"ltr", "rtl"} {
		t.Run(d, func(t *testing.T) {
			forceDirection(t, d)
			e := newEnv(t)
			tt := e.hub(t)
			nav, page := roleBounds(t, tt, mu.RoleNavigation), roleBounds(t, tt, mu.RoleMain)
			if got := nav.X > page.X; got != (d == "rtl") {
				t.Errorf("sidebar %v, page %v: sidebar on the right is %v", nav, page, got)
			}
			// The rail and its collapse button follow it.
			press(t, tt, i18n.T("hub.sidebar.collapse"))
			tt.Frame()
			nav, page = roleBounds(t, tt, mu.RoleNavigation), roleBounds(t, tt, mu.RoleMain)
			if nav.W > railWidth || (nav.X > page.X) != (d == "rtl") {
				t.Errorf("collapsed: rail %v, page %v", nav, page)
			}
		})
	}
}

func TestHistoryListIsOnTheStartSideOfTheDetail(t *testing.T) {
	for _, d := range []string{"ltr", "rtl"} {
		t.Run(d, func(t *testing.T) {
			forceDirection(t, d)
			e := newEnv(t)
			e.addEntry(t, "Ship it on Thursday.", "Slack", true)
			e.a.SetPage(PageHistory)
			tt := e.hub(t)
			tt.Frame()
			list, ok := tt.Access(i18n.T("history.a11y.list"))
			if !ok {
				t.Fatal("no history list")
			}
			detail, ok := tt.Access(i18n.T("history.a11y.final_text"))
			if !ok {
				t.Fatalf("no detail: %q", tt.Texts())
			}
			if got := center(list.Bounds) > center(detail.Bounds); got != (d == "rtl") {
				t.Errorf("list %v, detail %v: list on the right is %v", list.Bounds, detail.Bounds, got)
			}
		})
	}
}

func TestStylesDrawerIsAwayFromTheSidebar(t *testing.T) {
	for _, d := range []string{"ltr", "rtl"} {
		t.Run(d, func(t *testing.T) {
			forceDirection(t, d)
			e := newEnv(t)
			e.a.SetPage(PageStyles)
			tt := e.hub(t)
			press(t, tt, i18n.T("styles.new_style"))
			tt.Frame()
			drawer, page := roleBounds(t, tt, mu.RoleDialog), roleBounds(t, tt, mu.RoleMain)
			// 12 px inside the sheet, on the side away from the sidebar.
			var gap float32
			if d == "rtl" {
				gap = drawer.X - page.X
			} else {
				gap = page.X + page.W - (drawer.X + drawer.W)
			}
			if gap < 11 || gap > 13 {
				t.Errorf("drawer %v in page %v: %v px from its edge", drawer, page, gap)
			}
		})
	}
}

func TestOnboardingNextIsAtTheEndOfTheFooter(t *testing.T) {
	for _, d := range []string{"ltr", "rtl"} {
		t.Run(d, func(t *testing.T) {
			forceDirection(t, d)
			e := newEnv(t)
			e.eng.setState(string(e.a.FinalModel()), engine.ModelStatus{State: engine.StateDownloaded})
			e.a.onb.step = stepModel
			tt := mu.NewTester(e.a.OnboardingView, OnboardingW, OnboardingH)
			tt.Frame()
			back, ok1 := controlBounds(tt, i18n.T("onboarding.footer.back"))
			next, ok2 := controlBounds(tt, i18n.T("onboarding.footer.continue"))
			if !ok1 || !ok2 {
				t.Fatalf("no back or next: %q", tt.Texts())
			}
			if got := next.X < back.X; got != (d == "rtl") {
				t.Errorf("back %v, next %v: next to the left of back is %v", back, next, got)
			}
			// Back still goes back, whichever side it is on.
			press(t, tt, i18n.T("onboarding.footer.back"))
			if e.a.OnboardingStep() != stepWelcome {
				t.Errorf("back went to step %d", e.a.OnboardingStep())
			}
		})
	}
}

// The traffic lights stay at the top left right to left: no control and no
// page sheet may be under them.
func TestRightToLeftKeepsClearOfTheTrafficLights(t *testing.T) {
	forceDirection(t, "rtl")
	lights := mu.Rect{X: 0, Y: 0, W: macBar.Left, H: macBar.Bottom}
	check := func(t *testing.T, tt *mu.Tester, sheet bool) {
		t.Helper()
		for _, a := range tt.AccessAll() {
			if a.Role == mu.RoleText || a.Role == mu.RoleNone || a.Bounds.W == 0 {
				continue
			}
			if overlaps(a.Bounds, lights) && (a.Role != mu.RoleMain || sheet) {
				t.Errorf("%v %q at %v is under the traffic lights", a.Role, a.Name, a.Bounds)
			}
		}
	}
	for _, setup := range []struct {
		name string
		set  func(*App)
	}{
		{"home", func(*App) {}},
		{"history", func(a *App) { a.SetPage(PageHistory) }},
		{"styles", func(a *App) { a.SetPage(PageStyles) }},
		{"models", func(a *App) { a.SetPage(PageModels) }},
		{"settings", func(a *App) { a.OpenSettings(SecProviders) }},
	} {
		t.Run(setup.name, func(t *testing.T) {
			e := newEnv(t)
			e.addEntry(t, "Ship it on Thursday.", "Slack", true)
			setup.set(e.a)
			tt := e.hub(t)
			tt.SetTitleBar(macBar)
			tt.Frame()
			if page := roleBounds(t, tt, mu.RoleMain); page.Y < macBar.Bottom {
				t.Errorf("the page sheet starts at %v, under the lights", page.Y)
			}
			check(t, tt, true)
		})
	}
	t.Run("onboarding", func(t *testing.T) {
		e := newEnv(t)
		for s := range OnboardingSteps {
			e.a.onb.step = s
			tt := mu.NewTester(e.a.OnboardingView, OnboardingW, OnboardingH)
			tt.SetTitleBar(macBar)
			tt.Frame()
			// The onboarding sheet is under the lights in both directions,
			// with its top strip kept free for them.
			check(t, tt, false)
			if r, ok := tt.Find(i18n.T("onboarding.step", "step", s+1, "total", OnboardingSteps)); !ok || overlaps(r, lights) {
				t.Errorf("step %d: the step label at %v is under the lights", s+1, r)
			}
		}
	})
}

// A hotkey reads left to right in a right-to-left interface.
func TestHotkeyKeycapsStayLeftToRight(t *testing.T) {
	forceDirection(t, "rtl")
	e := newEnv(t)
	h := e.a.cfg.Hotkeys.Toggle
	if h == nil {
		t.Skip("no toggle hotkey on this system")
	}
	caps := h.Keycaps()
	if len(caps) < 2 {
		t.Skipf("one key: %q", caps)
	}
	for _, view := range []struct {
		name string
		tt   *mu.Tester
	}{
		{"home", e.hub(t)},
		{"popover", mu.NewTester(e.a.PopoverView, PopoverW, PopoverH)},
	} {
		view.tt.Frame()
		prev := float32(-1)
		for _, k := range caps {
			r, ok := view.tt.Find(k)
			if !ok {
				t.Fatalf("%s: no keycap %q: %q", view.name, k, view.tt.Texts())
			}
			if r.X <= prev {
				t.Errorf("%s: keycap %q at %v is left of the key before it", view.name, k, r.X)
			}
			prev = r.X
		}
	}
}

// Directional icons mirror right to left; the others never do.
func TestDirectionalIconsMirror(t *testing.T) {
	render := func(t *testing.T, d, name string) *image.RGBA {
		forceDirection(t, d)
		e := newEnv(t)
		view := func(c *mu.Context) {
			e.a.look(c)
			mu.Row(c).Padding(4).Children(func() { mu.Icon(c, dirIcon(c, name)).FontSize(32).TextColor(mu.RGB(0, 0, 0)) })
		}
		tt := mu.NewTester(view, 40, 40)
		tt.Frame()
		return tt.Image()
	}
	for _, tc := range []struct {
		name   string
		mirror bool
	}{
		{"chevron-left", true}, {"chevron-right", true}, {"arrow-right", true}, {"sidebar", true}, {"undo", true},
		{"check", false}, {"plus", false}, {"close", false}, {"mic", false}, {"play", false}, {"search", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			l, r := render(t, "ltr", tc.name), render(t, "rtl", tc.name)
			// RTL lays the 32 px icon out from the right of the 40 px row:
			// compare the icon's own box.
			lb, rb := inkBox(l), inkBox(r)
			if lb.Empty() || lb.Dx() != rb.Dx() || lb.Dy() != rb.Dy() {
				t.Fatalf("ink boxes %v and %v", lb, rb)
			}
			same, flipped := 0, 0
			for y := 0; y < lb.Dy(); y++ {
				for x := 0; x < lb.Dx(); x++ {
					a := inked(l, lb.Min.X+x, lb.Min.Y+y)
					if a == inked(r, rb.Min.X+x, rb.Min.Y+y) {
						same++
					}
					if a == inked(r, rb.Max.X-1-x, rb.Min.Y+y) {
						flipped++
					}
				}
			}
			n := lb.Dx() * lb.Dy()
			if tc.mirror && flipped < n*97/100 {
				t.Errorf("right to left is not the mirror image: %d of %d pixels match it", flipped, n)
			}
			if !tc.mirror && same != n {
				t.Errorf("right to left changed the icon: %d of %d pixels the same", same, n)
			}
		})
	}
}

// inked reports whether a pixel is of the black icon, not the light paper.
func inked(img *image.RGBA, x, y int) bool {
	p := img.RGBAAt(x, y)
	return p.R < 128 && p.A > 128
}

// inkBox is the box of the drawn pixels of an icon.
func inkBox(img *image.RGBA) image.Rectangle {
	b := img.Bounds()
	box := image.Rectangle{Min: b.Max, Max: b.Min}
	for y := b.Min.Y; y < b.Max.Y; y++ {
		for x := b.Min.X; x < b.Max.X; x++ {
			if inked(img, x, y) {
				box.Min.X, box.Min.Y = min(box.Min.X, x), min(box.Min.Y, y)
				box.Max.X, box.Max.Y = max(box.Max.X, x+1), max(box.Max.Y, y+1)
			}
		}
	}
	return box
}

func TestMirrorSVGWrapsTheIconInAFlip(t *testing.T) {
	in := []byte(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18"><path d="M3 9 H14"/></svg>`)
	want := `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18"><g transform="matrix(-1 0 0 1 18 0)"><path d="M3 9 H14"/></g></svg>`
	if got := string(mirrorSVG(in)); got != want {
		t.Errorf("got  %s\nwant %s", got, want)
	}
}

// A path shown right to left keeps its slashes and tilde on its own side:
// it is a left-to-right isolate in the text, not a run the sentence
// reorders ("models/ · لم يُنزَّل" with the slash at the wrong end).
func TestPathsAreIsolatedRightToLeft(t *testing.T) {
	forceDirection(t, "rtl")
	isolated := func(s string) string { return "⁦" + s + "⁩" }
	hasIn := func(tt *mu.Tester, want string) bool {
		for _, s := range tt.Texts() {
			if strings.Contains(s, want) {
				return true
			}
		}
		return false
	}
	e := newEnv(t)
	e.a.SetPage(PageModels)
	tt := e.hub(t)
	if want := isolated(tildePath(e.a.d.Paths.ModelsDir())); !hasIn(tt, want) {
		t.Errorf("models folder: want %q in %q", want, tt.Texts())
	}

	e = newEnv(t)
	e.a.OpenSettings(SecGeneral)
	tt = e.hub(t)
	if want := isolated(tildePath(e.a.d.ConfigPath)); !hasIn(tt, want) {
		t.Errorf("config file: want %q in %q", want, tt.Texts())
	}

	e.a.OpenSettings(SecAdvanced)
	tt.Frame()
	if want := isolated(e.a.d.Paths.CacheDir); !hasIn(tt, want) {
		t.Errorf("cache folder: want %q in %q", want, tt.Texts())
	}

	e = newEnv(t)
	e.a.cfg.AI.Providers = []config.Provider{{ID: "claude-code", Name: "Claude Code", Kind: config.KindClaudeCLI, Model: ptr("sonnet"), Path: ptr("/usr/local/bin/claude")}}
	e.a.cfg.AI.DefaultProvider = ptr("claude-code")
	e.a.OpenSettings(SecProviders)
	tt = e.hub(t)
	if want := isolated("/usr/local/bin/claude"); !hasIn(tt, want) {
		t.Errorf("provider path: want %q in %q", want, tt.Texts())
	}
}
