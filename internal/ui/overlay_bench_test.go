package ui

import (
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
)

// benchmarkOverlayRecording measures one display frame of the overlay while
// it records: a new input level, the view, and the paint.
func benchmarkOverlayRecording(b *testing.B, style config.OverlayStyle) {
	a := newEnv(b).a
	a.cfg.Overlay.Style = style
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	w, h := OverlayWindowSize(a.cfg.Overlay)
	tt := mu.NewTester(a.OverlayView, w, h)
	for i := range 120 {
		a.wave.push(float32(i%10) / 10)
		tt.Advance(frame60)
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := range b.N {
		a.wave.push(float32(i%10) / 10)
		tt.Advance(frame60)
	}
}

func BenchmarkOverlayRecordingPill(b *testing.B)  { benchmarkOverlayRecording(b, config.OverlayPill) }
func BenchmarkOverlayRecordingNotch(b *testing.B) { benchmarkOverlayRecording(b, config.OverlayNotch) }
