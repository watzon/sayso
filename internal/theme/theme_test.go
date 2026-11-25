package theme

import (
	"bytes"
	"fmt"
	"math"
	"os"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
)

// testdata/sayso-ink-golden.txt was printed by the Rust sayso-ink of 0.16
// (Palette::new and Palette::contrast_report) for the six inks, a custom
// #123456 and a pale #D0D0D0.
func goldenInks() map[string]Ink {
	m := map[string]Ink{"custom": {Custom: 0x123456, IsCustom: true}, "pale": {Custom: 0xD0D0D0, IsCustom: true}}
	for _, p := range AllInks {
		m[strings.ReplaceAll(p.Name(), " ", "")] = Ink{Preset: p}
	}
	return m
}

func fields(p Palette) string {
	return fmt.Sprintf("ink=%s fill=%s accent=%s wash=%s ground=%s sheet=%s raised=%s deboss=%s shade=%s rule=%s onink=%s gr=%s pe=%s dg=%s dw=%s su=%s sh=%s",
		p.Ink.Hex(), p.InkFill.Hex(), p.Accent.Hex(), p.AccentWash.Hex(), p.Ground.Hex(), p.Sheet.Hex(), p.SheetRaised.Hex(),
		p.Deboss.Hex(), p.DebossShade.Hex(), p.Rule.Hex(), p.OnInk.Hex(), p.Graphite.Hex(), p.Pencil.Hex(),
		p.Danger.Hex(), p.DangerWash.Hex(), p.Success.Hex(), p.Shadow.Hex())
}

func TestPalettesMatchRust(t *testing.T) {
	data, err := os.ReadFile("testdata/sayso-ink-golden.txt")
	if err != nil {
		t.Fatal(err)
	}
	inks := goldenInks()
	palettes, contrasts := 0, 0
	// A checkout made before .gitattributes kept this file LF has CRLF.
	for _, line := range strings.Split(strings.TrimSpace(strings.ReplaceAll(string(data), "\r\n", "\n")), "\n") {
		name, rest, _ := strings.Cut(line, " ")
		ink, ok := inks[name]
		if !ok {
			t.Fatalf("unknown ink %q in golden file", name)
		}
		if strings.HasPrefix(rest, "contrast ") {
			var l, d float64
			fmt.Sscanf(rest, "contrast %f %f", &l, &d)
			r := NewContrastReport(ink)
			if math.Abs(r.LightRatio-l) > 1e-5 || math.Abs(r.DarkRatio-d) > 1e-5 {
				t.Errorf("%s contrast %.6f %.6f, want %.6f %.6f", name, r.LightRatio, r.DarkRatio, l, d)
			}
			contrasts++
			continue
		}
		mode, want, _ := strings.Cut(rest, " ")
		got := fields(NewPalette(ink, mode == "Dark"))
		if got != want {
			t.Errorf("%s %s:\n got %s\nwant %s", name, mode, got, want)
		}
		palettes++
	}
	if palettes != 16 || contrasts != 8 {
		t.Errorf("golden file has %d palettes and %d contrast lines, want 16 and 8", palettes, contrasts)
	}
}

func TestSumiHexValues(t *testing.T) {
	light, dark := NewPalette(Ink{Preset: Sumi}, false), NewPalette(Ink{Preset: Sumi}, true)
	want := map[string][2]Rgb{
		"ink":          {0x1D1B18, 0xECE5D8},
		"accent":       {0x2D3C8C, 0x8D9BE6},
		"accent_wash":  {0xD7D7DE, 0x343540},
		"danger_wash":  {0xF3E7DC, 0x3A2420},
		"deboss_shade": {0xCFC8BB, 0x0E0D0C},
		"rule":         {0xD8D1C4, 0x34322E},
		"danger":       {0x8E2F22, 0xE08A7C},
		"success":      {0x4E7A4A, 0x8FB98A},
		"sheet":        {0xF7F4EE, 0x201F1C},
		"ground":       {0xE9E4DA, 0x171614},
		"raised":       {0xFCFAF6, 0x292825},
		"shadow":       {0x3C2E1C, 0x000000},
	}
	get := func(p Palette) map[string]Rgb {
		return map[string]Rgb{"ink": p.Ink, "accent": p.Accent, "accent_wash": p.AccentWash, "danger_wash": p.DangerWash,
			"deboss_shade": p.DebossShade, "rule": p.Rule, "danger": p.Danger, "success": p.Success, "sheet": p.Sheet,
			"ground": p.Ground, "raised": p.SheetRaised, "shadow": p.Shadow}
	}
	l, d := get(light), get(dark)
	for k, w := range want {
		if l[k] != w[0] {
			t.Errorf("light %s = %s, want %s", k, l[k].Hex(), w[0].Hex())
		}
		if d[k] != w[1] {
			t.Errorf("dark %s = %s, want %s", k, d[k].Hex(), w[1].Hex())
		}
	}
}

func TestInkPresets(t *testing.T) {
	want := map[InkPreset]Rgb{Sumi: 0x1D1B18, Indigo: 0x2D3C8C, IronGall: 0x2F3A4F, Sepia: 0x6B4A2E, Verdigris: 0x2F5E52, Oxblood: 0x6E2A24}
	if len(AllInks) != 6 {
		t.Fatalf("%d inks", len(AllInks))
	}
	for p, c := range want {
		if p.Color() != c || !p.Valid() {
			t.Errorf("%s = %s", p, p.Color().Hex())
		}
	}
	if InkPreset("nope").Valid() || InkPreset("nope").Color() != 0x1D1B18 {
		t.Error("an unknown preset is Sumi")
	}
	if IronGall.Name() != "Iron Gall" {
		t.Errorf("name %q", IronGall.Name())
	}
}

func TestEveryNamedInkPassesContrast(t *testing.T) {
	for _, p := range AllInks {
		ink := Ink{Preset: p}
		if r := NewContrastReport(ink); !r.Passes() {
			t.Errorf("%s fails: %+v", p, r)
		}
		for _, dark := range []bool{false, true} {
			pal := NewPalette(ink, dark)
			if pal.Graphite.Contrast(pal.Sheet) < 4.5 || pal.Accent.Contrast(pal.Sheet) < 3.0 || pal.OnInk.Contrast(pal.InkFill) < 4.5 {
				t.Errorf("%s dark=%v: contrast floor", p, dark)
			}
		}
	}
	if NewContrastReport(Ink{Custom: 0xD0D0D0, IsCustom: true}).Passes() {
		t.Error("a pale custom ink must fail")
	}
}

func TestContrastReference(t *testing.T) {
	if r := Rgb(0x000000).Contrast(0xFFFFFF); math.Abs(r-21) > 0.01 {
		t.Errorf("black/white = %f", r)
	}
	if r := Rgb(0x777777).Contrast(0xFFFFFF); math.Abs(r-4.48) > 0.05 {
		t.Errorf("777/white = %f", r)
	}
}

func TestParseInk(t *testing.T) {
	i, err := ParseInk("iron_gall")
	if err != nil || i.Preset != IronGall || i.IsCustom || i.String() != "iron_gall" {
		t.Errorf("named: %+v %v", i, err)
	}
	i, err = ParseInk("#123456")
	if err != nil || !i.IsCustom || i.Custom != 0x123456 || i.String() != "#123456" {
		t.Errorf("custom: %+v %v", i, err)
	}
	for _, bad := range []string{"", "#12345", "#GGGGGG", "plaid", "#1234567"} {
		if _, err := ParseInk(bad); err == nil {
			t.Errorf("%q parsed", bad)
		}
	}
	if c, ok := ParseRgb("abcdef"); !ok || c.Hex() != "#ABCDEF" {
		t.Errorf("no # : %v %v", c, ok)
	}
}

func TestBuildMyGoTheme(t *testing.T) {
	light := BuildMyGoTheme(Sumi, "", false)
	if light.UI.Dark || light.UI.Font != UI || light.UI.FontSize != 14 || light.UI.Radius != 10 {
		t.Errorf("light theme: %+v", light.UI)
	}
	if light.UI.Background != ui.Hex("#E9E4DA") || light.UI.Text != ui.Hex("#1D1B18") || light.UI.Focus != ui.Hex("#2D3C8C") {
		t.Error("light theme colors")
	}
	if light.UI.Selection != ui.Hex("#D7D7DE") || light.UI.AccentText != ui.Hex("#F7F4EE") {
		t.Error("light selection / accent text")
	}
	dark := BuildMyGoTheme(Sumi, "", true)
	if !dark.UI.Dark || dark.UI.Text != ui.Hex("#ECE5D8") || dark.UI.Background != ui.Hex("#171614") {
		t.Error("dark theme")
	}
	if dark.Palette.Ink != 0xECE5D8 || dark.Colors.Ink != ui.Hex("#ECE5D8") {
		t.Error("dark palette")
	}
	// A custom hex wins over the preset; a bad one is ignored.
	c := BuildMyGoTheme(Oxblood, "#123456", false)
	if c.Palette.Ink != 0x123456 {
		t.Errorf("custom ink = %s", c.Palette.Ink.Hex())
	}
	if b := BuildMyGoTheme(Oxblood, "banana", false); b.Palette.Ink != 0x6E2A24 {
		t.Errorf("bad custom ink = %s", b.Palette.Ink.Hex())
	}
	if u := BuildMyGoTheme("unknown", "", false); u.Palette.Ink != 0x1D1B18 {
		t.Error("unknown preset should be Sumi")
	}
	// Each theme is its own copy: ui.LightTheme() is not shared.
	if BuildMyGoTheme(Sumi, "", false).UI == light.UI {
		t.Error("themes share a pointer")
	}
}

func TestEmbeddedAssets(t *testing.T) {
	if len(Fonts) != 16 {
		t.Fatalf("%d fonts listed, want 16", len(Fonts))
	}
	families := map[string]int{}
	for _, f := range Fonts {
		families[f.Family]++
		d := FontData(f.File)
		if len(d) < 10_000 || !bytes.HasPrefix(d, []byte{0, 1, 0, 0}) && !bytes.HasPrefix(d, []byte("OTTO")) {
			t.Errorf("font %s: %d bytes or not TrueType", f.File, len(d))
		}
	}
	if families["Fraunces"] != 5 || families["Fraunces Text"] != 5 || families["Instrument Sans"] != 4 || families["IBM Plex Mono"] != 2 {
		t.Errorf("families: %v", families)
	}
	entries, _ := assets.ReadDir("assets/fonts")
	if len(entries) != 16 {
		t.Errorf("%d font files embedded, want 16", len(entries))
	}
	icons, _ := assets.ReadDir("assets/icons")
	if len(icons) != 48 {
		t.Fatalf("%d icons embedded, want 48", len(icons))
	}
	for _, e := range icons {
		name := strings.TrimSuffix(e.Name(), ".svg")
		if d := IconSVG(name); len(d) == 0 || !bytes.Contains(d, []byte("<svg")) {
			t.Errorf("icon %s empty or not SVG", name)
		}
	}
	if len(IconSVG("mic.svg")) == 0 || IconSVG("nope") != nil || IconSVG("../x") != nil || IconSVG("") != nil {
		t.Error("icon lookup rules")
	}
	for _, n := range []string{"start", "stop", "cancel", "insert"} {
		d := SoundWAV(n)
		if len(d) < 44 || string(d[:4]) != "RIFF" || string(d[8:12]) != "WAVE" {
			t.Errorf("sound %s: not a WAV (%d bytes)", n, len(d))
		}
	}
	for _, n := range []string{"board-light", "board-dark", "sheet-light", "sheet-dark"} {
		if d := TexturePNG(n); len(d) < 8 || !bytes.HasPrefix(d, []byte("\x89PNG")) {
			t.Errorf("texture %s not PNG", n)
		}
	}
	svgs := []string{"apple", "claude-color", "cursor", "gemini-color", "kimi-color", "lmstudio", "ollama", "openai", "opencode", "openrouter", "qwen-color"}
	for _, n := range svgs {
		if d := ProviderLogo(n); !bytes.Contains(d, []byte("<svg")) {
			t.Errorf("provider %s not SVG", n)
		}
	}
	for _, n := range []string{"spokenly", "superwhisper", "spokenly.png"} {
		if d := ProviderLogo(n); !bytes.HasPrefix(d, []byte("\x89PNG")) {
			t.Errorf("provider %s not PNG", n)
		}
	}
	if ProviderLogo("unknown") != nil {
		t.Error("unknown provider")
	}
}

func TestRegisterFonts(t *testing.T) {
	if err := RegisterFonts(); err != nil {
		t.Fatal(err)
	}
	if err := RegisterFonts(); err != nil {
		t.Fatalf("second call: %v", err)
	}
	reg := map[string]bool{}
	for _, f := range ui.FontFamilies() {
		if f.Registered {
			reg[f.Name] = true
		}
	}
	for _, name := range []string{Display, Body, UI, Mono} {
		if !reg[name] {
			t.Errorf("family %q not registered; have %v", name, reg)
		}
	}
}
