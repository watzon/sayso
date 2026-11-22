package theme

import (
	"embed"
	"fmt"
	"strings"
	"sync"

	"github.com/egoist/mygo/ui"
)

//go:embed assets/fonts/*.ttf assets/icons/*.svg assets/providers/*.svg assets/providers/*.png assets/sounds/*.wav assets/textures/*.png
var assets embed.FS

// Font is one embedded face and the family it is registered under.
type Font struct {
	File   string
	Family string
}

// Fonts are all 16 embedded faces. The weight and style of each come from
// the font file.
var Fonts = []Font{
	{"Fraunces-Regular.ttf", Display}, {"Fraunces-Medium.ttf", Display}, {"Fraunces-SemiBold.ttf", Display},
	{"Fraunces-Bold.ttf", Display}, {"Fraunces-Italic.ttf", Display},
	{"FrauncesText-Regular.ttf", Body}, {"FrauncesText-Medium.ttf", Body}, {"FrauncesText-SemiBold.ttf", Body},
	{"FrauncesText-Bold.ttf", Body}, {"FrauncesText-Italic.ttf", Body},
	{"InstrumentSans-Regular.ttf", UI}, {"InstrumentSans-Medium.ttf", UI}, {"InstrumentSans-SemiBold.ttf", UI},
	{"InstrumentSans-Bold.ttf", UI},
	{"IBMPlexMono-Regular.ttf", Mono}, {"IBMPlexMono-Medium.ttf", Mono},
}

var registerOnce sync.Once
var registerErr error

// RegisterFonts registers every embedded face with MyGo. It runs once; later
// calls return the first result.
func RegisterFonts() error {
	registerOnce.Do(func() {
		for _, f := range Fonts {
			data, err := assets.ReadFile("assets/fonts/" + f.File)
			if err == nil {
				err = ui.RegisterFont(data, f.Family)
			}
			if err != nil {
				registerErr = fmt.Errorf("theme: font %s: %w", f.File, err)
				return
			}
		}
	})
	return registerErr
}

// FontData returns the bytes of an embedded font file, or nil.
func FontData(file string) []byte { return read("assets/fonts", file, ".ttf") }

// IconSVG returns the SVG of an icon ("mic", "chevron-down"), or nil.
func IconSVG(name string) []byte { return read("assets/icons", name, ".svg") }

// ProviderLogo returns the logo of a provider ("openai", "claude-color"): the
// SVG, or the PNG for those that only have one. Nil when unknown.
func ProviderLogo(name string) []byte {
	if b := read("assets/providers", name, ".svg"); b != nil {
		return b
	}
	return read("assets/providers", name, ".png")
}

// SoundWAV returns a sound cue: "start", "stop", "cancel" or "insert".
func SoundWAV(name string) []byte { return read("assets/sounds", name, ".wav") }

// TexturePNG returns a paper grain: "board-light", "board-dark",
// "sheet-light" or "sheet-dark".
func TexturePNG(name string) []byte { return read("assets/textures", name, ".png") }

// read returns dir/name, with ext added when name has none. Names with a
// path in them find nothing.
func read(dir, name, ext string) []byte {
	if name == "" || strings.ContainsAny(name, `/\`) {
		return nil
	}
	if !strings.HasSuffix(strings.ToLower(name), ext) {
		name += ext
	}
	b, err := assets.ReadFile(dir + "/" + name)
	if err != nil {
		return nil
	}
	return b
}
