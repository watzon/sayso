package ui

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// TestScreenshots renders the main screens headless, light and dark, into
// PNGs named <screen>-<theme>-<locale>.png, for a look at a release's
// layout without running the app. It is a test, so it never ships, and it
// runs only when asked:
//
//	SAYSO_SHOTS=1 go test ./internal/ui -run TestScreenshots
//
// SAYSO_SHOTS_DIR is the folder (default /tmp/sayso-prerelease-rtl),
// SAYSO_SHOTS_LOCALE the interface language (default en) and
// SAYSO_SHOTS_DIRECTION "rtl" or "ltr" to force a direction whatever the
// language, which adds it to the locale part of the names (en-rtl). The
// macOS traffic lights are painted over the hub and onboarding shots where
// the system draws them, to show what the layout must keep clear of.
func TestScreenshots(t *testing.T) {
	if os.Getenv("SAYSO_SHOTS") == "" {
		t.Skip("set SAYSO_SHOTS=1 to render the screenshots")
	}
	dir := os.Getenv("SAYSO_SHOTS_DIR")
	if dir == "" {
		dir = "/tmp/sayso-prerelease-rtl"
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	locale := os.Getenv("SAYSO_SHOTS_LOCALE")
	if locale == "" {
		locale = "en"
	}
	forced := os.Getenv("SAYSO_SHOTS_DIRECTION")
	label := locale
	if forced != "" {
		label += "-" + forced
	}
	i18n.OverrideDirection(forced)
	t.Cleanup(func() { i18n.OverrideDirection("") })

	macBar := mu.TitleBar{Height: 28, Left: 78, Bottom: 28}
	type shot struct {
		name  string
		setup func(t *testing.T, e *testEnv) *mu.Tester
	}
	hubAt := func(w, h int, set func(e *testEnv)) func(*testing.T, *testEnv) *mu.Tester {
		return func(t *testing.T, e *testEnv) *mu.Tester {
			if set != nil {
				set(e)
			}
			tt := mu.NewTester(e.a.HubView, w, h)
			tt.SetTitleBar(macBar)
			return tt
		}
	}
	shots := []shot{
		{"home", hubAt(HubWidth, HubHeight, func(e *testEnv) { addShotEntries(t, e) })},
		{"home-rail", hubAt(860, HubHeight, func(e *testEnv) { addShotEntries(t, e) })},
		{"history", hubAt(HubWidth, HubHeight, func(e *testEnv) { addShotEntries(t, e); e.a.SetPage(PageHistory) })},
		{"history-menu", func(t *testing.T, e *testEnv) *mu.Tester {
			addShotEntries(t, e)
			e.a.SetPage(PageHistory)
			tt := hubAt(HubWidth, HubHeight, nil)(t, e)
			tt.Frame()
			press(t, tt, i18n.T("history.a11y.more_actions"))
			return tt
		}},
		{"styles", hubAt(HubWidth, HubHeight, func(e *testEnv) { e.a.SetPage(PageStyles) })},
		{"styles-drawer", func(t *testing.T, e *testEnv) *mu.Tester {
			e.a.SetPage(PageStyles)
			tt := hubAt(HubWidth, HubHeight, nil)(t, e)
			tt.Frame()
			press(t, tt, i18n.T("styles.new_style"))
			return tt
		}},
		{"dictionary", hubAt(HubWidth, HubHeight, func(e *testEnv) { e.a.SetPage(PageDictionary) })},
		{"models", hubAt(HubWidth, HubHeight, func(e *testEnv) { e.a.SetPage(PageModels) })},
		{"settings", hubAt(HubWidth, HubHeight, func(e *testEnv) { e.a.OpenSettings(SecGeneral) })},
		{"settings-ai", hubAt(HubWidth, HubHeight, func(e *testEnv) { e.a.OpenSettings(SecProviders) })},
		{"popover", func(t *testing.T, e *testEnv) *mu.Tester { return mu.NewTester(e.a.PopoverView, PopoverW, PopoverH) }},
		{"overlay-compact", func(t *testing.T, e *testEnv) *mu.Tester {
			e.a.cfg.Overlay.Size, e.a.cfg.Overlay.ShowPreview = config.OverlayMedium, false
			e.a.StateChanged(dictation.State{Kind: dictation.StateRecording})
			for i := range 48 {
				e.a.Level(0.3 + 0.5*float32(i%7)/7)
			}
			w, h := OverlayWindowSize(e.a.cfg.Overlay)
			return mu.NewTesterWith(e.a.OverlayView, w, h, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
		}},
		{"overlay-expanded", func(t *testing.T, e *testEnv) *mu.Tester {
			e.a.cfg.Overlay.Size, e.a.cfg.Overlay.ShowPreview = config.OverlayLarge, true
			e.a.StateChanged(dictation.State{Kind: dictation.StateRecording})
			e.a.Preview(shotSpeech, "")
			for i := range 48 {
				e.a.Level(0.3 + 0.5*float32(i%7)/7)
			}
			w, h := OverlayWindowSize(e.a.cfg.Overlay)
			return mu.NewTesterWith(e.a.OverlayView, w, h, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
		}},
		{"overlay-notch", func(t *testing.T, e *testEnv) *mu.Tester {
			e.a.cfg.Overlay.Style, e.a.cfg.Overlay.ShowPreview = config.OverlayNotch, true
			e.a.StateChanged(dictation.State{Kind: dictation.StateRecording})
			e.a.Preview(shotSpeech, "")
			return mu.NewTesterWith(e.a.OverlayView, NotchWindowW, NotchWindowH, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
		}},
		{"permissions", func(t *testing.T, e *testEnv) *mu.Tester {
			tt := mu.NewTester(e.a.PermissionsView, 560, 640)
			tt.SetTitleBar(macBar)
			return tt
		}},
		{"whats-new", func(t *testing.T, e *testEnv) *mu.Tester {
			tt := mu.NewTester(e.a.WhatsNewView, 560, 640)
			tt.SetTitleBar(macBar)
			return tt
		}},
	}
	// The faces Arabic text gets, whatever the interface language: the
	// sans of the interface, the serif of headings and transcripts, and a
	// hotkey and a path kept left to right inside a sentence.
	shots = append(shots, shot{"type-arabic", func(t *testing.T, e *testEnv) *mu.Tester {
		view := func(c *mu.Context) {
			p := e.a.look(c)
			c.Root().Background(p.Sheet).Direction(mu.RightToLeft)
			mu.Column(c).Padding(28).Gap(14).Children(func() {
				mu.Text(c, "مرحبًا بك في Sayso").Font(theme.SerifFor(theme.Display, "ar")).FontSize(34).FontWeight(600).TextColor(p.Ink)
				mu.Text(c, "تحدّث، وسيكتب Sayso النص في التطبيق الذي تستخدمه.").Font(theme.SerifFor(theme.Body, "ar")).FontSize(18).TextColor(p.Ink)
				mu.Text(c, "اضغط "+lriText("⌥ Space")+" في أي تطبيق لتبدأ الإملاء.").Font(theme.UIFor("ar")).FontSize(15).TextColor(p.Ink)
				mu.Text(c, "الإعدادات محفوظة في "+lriText("~/Library/Application Support/Sayso/config.toml")+".").Font(theme.UIFor("ar")).FontSize(13).TextColor(p.Graphite)
				mu.Text(c, "أمليتَ ١٢٣ كلمة اليوم، في "+lriText("Slack")+" و"+lriText("Mail")+".").Font(theme.UIFor("ar")).FontSize(13).FontWeight(500).TextColor(p.Graphite)
			})
		}
		return mu.NewTester(view, 640, 300)
	}})
	steps := []string{"welcome", "model", "ai", "permissions", "hotkeys", "practice", "done"}
	for s := range OnboardingSteps {
		shots = append(shots, shot{"onboarding-" + steps[s], func(t *testing.T, e *testEnv) *mu.Tester {
			e.a.onb.step = s
			tt := mu.NewTesterWith(e.a.OnboardingView, OnboardingW, OnboardingH, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
			tt.SetTitleBar(macBar)
			return tt
		}})
	}

	for _, dark := range []bool{false, true} {
		mode := "light"
		if dark {
			mode = "dark"
		}
		for _, s := range shots {
			t.Run(s.name+"-"+mode, func(t *testing.T) {
				e := newEnv(t)
				i18n.SetLocale(locale)
				e.a.cfg.General.Language = locale
				if dark {
					e.a.cfg.Appearance.Theme = config.ThemeDark
				}
				tt := s.setup(t, e)
				tt.SetScale(2)
				tt.SetDark(dark)
				for range 4 {
					tt.Frame()
					tt.Advance(time.Second)
				}
				img := tt.Image()
				if strings.HasPrefix(s.name, "home") || strings.HasPrefix(s.name, "history") || strings.HasPrefix(s.name, "styles") ||
					strings.HasPrefix(s.name, "settings") || s.name == "dictionary" || s.name == "models" ||
					strings.HasPrefix(s.name, "onboarding") || s.name == "permissions" || s.name == "whats-new" {
					paintTrafficLights(img, 2)
				}
				writePNG(t, filepath.Join(dir, s.name+"-"+mode+"-"+label+".png"), img)
			})
		}
	}
}

func addShotEntries(t *testing.T, e *testEnv) {
	e.addEntry(t, "Let's move the standup to Thursday and invite the design team to join us.", "Slack", true)
	e.addEntry(t, "Remind me to send the invoice to Maria before Friday.", "Mail", false)
	e.addEntry(t, "The build on main is green again, ship it.", "Terminal", false)
}

// paintTrafficLights draws the three window buttons where macOS puts them
// in a window with a hidden title bar: the top left, whatever the language.
func paintTrafficLights(img *image.RGBA, scale float32) {
	for i, col := range []color.RGBA{{0xFF, 0x5F, 0x57, 0xFF}, {0xFE, 0xBC, 0x2E, 0xFF}, {0x28, 0xC8, 0x40, 0xFF}} {
		cx, cy, r := (20+float32(i)*20)*scale, 14*scale, 6*scale
		for y := int(cy - r); y <= int(cy+r); y++ {
			for x := int(cx - r); x <= int(cx+r); x++ {
				if dx, dy := float32(x)-cx, float32(y)-cy; dx*dx+dy*dy <= r*r {
					img.SetRGBA(x, y, col)
				}
			}
		}
	}
}

func writePNG(t *testing.T, path string, img image.Image) {
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}

// shotSpeech is what the overlay shots show as said: Arabic once the locale
// is Arabic, else English.
var shotSpeech = "Let's move the standup to Thursday and invite the design team to join"

// lriText isolates a left-to-right token, as i18n.Isolate does in a
// right-to-left interface, whatever the interface direction of the shot.
func lriText(s string) string { return "\u2066" + s + "\u2069" }
