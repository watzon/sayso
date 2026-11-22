package platform

// App icons for the badges of Home and History, from Launch Services
// (NSWorkspace iconForFile:), as sayso-platform-macos's app_icon_png.

import "github.com/ebitengine/purego/objc"

// nsBitmapImageFileTypePNG is NSBitmapImageFileTypePNG.
const nsBitmapImageFileTypePNG = 4

type macAppIcons struct{}

// AppIconPNG renders the icon of the app with bundleID as a PNG of the
// smallest size at least size px wide, or nil when the app is not found.
func (macAppIcons) AppIconPNG(bundleID string, size int) []byte {
	if bundleID == "" || loadDarwin() != nil {
		return nil
	}
	var out []byte
	withPool(func() {
		ws := class("NSWorkspace").Send(sel("sharedWorkspace"))
		url := ws.Send(sel("URLForApplicationWithBundleIdentifier:"), nsString(bundleID))
		if url == 0 {
			return
		}
		img := ws.Send(sel("iconForFile:"), url.Send(sel("path")))
		if img == 0 {
			return
		}
		tiff := img.Send(sel("TIFFRepresentation"))
		if tiff == 0 {
			return
		}
		reps := class("NSBitmapImageRep").Send(sel("imageRepsWithData:"), tiff)
		n := objc.Send[uint64](reps, sel("count"))
		var best, largest objc.ID
		var bestW, largestW int64
		for i := range n {
			rep := reps.Send(sel("objectAtIndex:"), i)
			w := objc.Send[int64](rep, sel("pixelsWide"))
			if w >= int64(size) && (best == 0 || w < bestW) {
				best, bestW = rep, w
			}
			if w > largestW {
				largest, largestW = rep, w
			}
		}
		if best == 0 {
			best = largest
		}
		if best == 0 {
			return
		}
		props := class("NSDictionary").Send(sel("dictionary"))
		out = goBytes(best.Send(sel("representationUsingType:properties:"), uint64(nsBitmapImageFileTypePNG), props))
	})
	return out
}
