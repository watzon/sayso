//go:build !mygo_noinspector

package main

import (
	"log"
	"math"
	"os"
	"strings"
	"time"

	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/ui"
)

// startDevPreview, with SAYSO_DEV_PREVIEW=1, plays dictations on the
// overlay, one after the other: the recording state with input levels and
// a live preview that grows word by word, as the speech model sends it,
// then processing, the inserted notice, and a pause at rest. It is for
// driving the overlay with mygo-automation. With SAYSO_DEV_FRAMES=1 the
// overlay logs the gaps between its frames for each recording. Release
// builds leave both out.
func startDevPreview(a *ui.App) {
	if os.Getenv("SAYSO_DEV_FRAMES") == "1" {
		a.LogOverlayFrames(func(s string) { log.Print(s) })
	}
	if os.Getenv("SAYSO_DEV_PREVIEW") != "1" {
		return
	}
	words := strings.Fields("Okay, we are so close in terms of the design, but the overlay does need a bit of help, " +
		"though specifically right now it looks like this, but if you keep talking the slip should hold its place " +
		"above the pill and the older lines should go up out of view while the latest words stay right here at the bottom.")
	// StateChanged, Level and Preview are the dictation observer's: they
	// post to the main thread.
	state := a.StateChanged
	go func() {
		for {
			state(dictation.State{Kind: dictation.StateRecording})
			record(a, words, 8*time.Second)
			state(dictation.State{Kind: dictation.StateProcessing})
			time.Sleep(1200 * time.Millisecond)
			state(dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeInserted, StyleName: "Clean"}})
			time.Sleep(1500 * time.Millisecond)
			state(dictation.State{Kind: dictation.StateIdle})
			time.Sleep(2500 * time.Millisecond)
		}
	}()
}

// record plays a recording of length d: a level each 20 ms, as the
// microphone gives them, and a word each 260 ms.
func record(a *ui.App, words []string, d time.Duration) {
	start := time.Now()
	levels := time.NewTicker(20 * time.Millisecond)
	defer levels.Stop()
	said := 0
	next := start
	for t := range levels.C {
		if t.Sub(start) > d {
			return
		}
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
}
