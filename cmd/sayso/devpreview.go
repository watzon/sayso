//go:build !mygo_noinspector

package main

import (
	"math"
	"os"
	"strings"
	"time"

	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/ui"
)

// startDevPreview, with SAYSO_DEV_PREVIEW=1, plays a long dictation on the
// overlay: the recording state, input levels, and a live preview that grows
// word by word, as the speech model sends it. It is for driving the
// overlay with mygo-automation; release builds leave it out.
func startDevPreview(a *ui.App) {
	if os.Getenv("SAYSO_DEV_PREVIEW") != "1" {
		return
	}
	words := strings.Fields("Okay, we are so close in terms of the design, but the overlay does need a bit of help, " +
		"though specifically right now it looks like this, but if you keep talking the slip should hold its place " +
		"above the pill and the older lines should go up out of view while the latest words stay right here at the bottom.")
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	go func() {
		start := time.Now()
		levels := time.NewTicker(time.Second / 30)
		defer levels.Stop()
		said := 0
		next := time.Now()
		for t := range levels.C {
			s := t.Sub(start).Seconds()
			a.Level(float32(0.45 + 0.35*math.Sin(s*5)*math.Cos(s*1.7)))
			if t.Before(next) {
				continue
			}
			next = t.Add(260 * time.Millisecond)
			if said < len(words) {
				said++
			}
			// The last three words are tentative; the rest are committed.
			cut := max(0, said-3)
			a.Preview(strings.Join(words[:cut], " "), strings.Join(words[cut:said], " "))
		}
	}()
}
