//go:build !darwin

package platform

// noopAppIcons is used on platforms without an app icon lookup backend.
// Badges fall back to the app's first letter.
type noopAppIcons struct{}

func (noopAppIcons) AppIconPNG(string, int) []byte { return nil }
func (noopAppIcons) AppName(string) string         { return "" }
