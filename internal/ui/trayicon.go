package ui

import (
	"bytes"
	_ "embed"
	"image"
	"image/draw"
	"image/png"
	"runtime"
)

// The tray icons of the Rust app (assets/app): a waveform drawn in black on
// clear, 36 px for 18 points at 2x, which macOS shows as a template image
// in the colors of the menu bar, and Windows as it is; Linux shows the app
// icon.
//
//go:generate cp ../../assets/app/tray.png tray.png
//go:generate cp ../../assets/app/icon-64.png tray-linux.png
var (
	//go:embed tray.png
	trayPNG []byte
	//go:embed tray-linux.png
	trayLinuxPNG []byte
)

// trayIconFor is the tray icon of a system, dimmed while incognito is on
// (Rust dim_icon).
func trayIconFor(goos string, incognito bool) []byte {
	src := trayPNG
	if goos == "linux" {
		src = trayLinuxPNG
	}
	if !incognito {
		return src
	}
	img, err := png.Decode(bytes.NewReader(src))
	if err != nil {
		return src
	}
	rgba := image.NewNRGBA(img.Bounds())
	draw.Draw(rgba, rgba.Bounds(), img, img.Bounds().Min, draw.Src)
	for i := 3; i < len(rgba.Pix); i += 4 {
		rgba.Pix[i] = uint8(float32(rgba.Pix[i]) * 0.4)
	}
	var b bytes.Buffer
	if png.Encode(&b, rgba) != nil {
		return src
	}
	return b.Bytes()
}

// trayIcon is this system's tray icon.
func trayIcon(incognito bool) []byte { return trayIconFor(runtime.GOOS, incognito) }

// trayTemplate is true where the system tints the icon to its menu bar.
const trayTemplate = runtime.GOOS == "darwin"
