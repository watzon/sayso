package ui

import (
	"testing"
	"time"

	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/store"
)

// A dictation's history entry is written on its own goroutine, after its
// last state change already had Home and History read the store, so they
// showed no new entry until something else made them read it again (issue
// 3). HistoryChanged, which the controller sends after the write, makes
// Home (Recent and Stats) and History read the store again and redraws.
func TestHomeAndHistoryFollowALateHistoryWrite(t *testing.T) {
	e := newEnv(t)
	// Posts run on the test goroutine, as on the main thread.
	posts := make(chan func(), 64)
	e.a.d.Post = func(fn func()) { posts <- fn }
	drain := func() {
		for {
			select {
			case fn := <-posts:
				fn()
			default:
				return
			}
		}
	}
	const text = "Ship the release notes before lunch"
	tt := e.hub(t)

	// The dictation ends: Processing, then Showing. Both views read the
	// store for the change, before the entry is written.
	e.a.StateChanged(dictation.State{Kind: dictation.StateProcessing})
	drain()
	e.a.StateChanged(dictation.State{Kind: dictation.StateShowing})
	drain()
	e.a.SetPage(PageHistory)
	tt.Frame()
	e.a.SetPage(PageHome)
	tt.Frame()
	if tt.HasText(text) || !e.a.home.loaded || !e.a.hist.loaded || e.a.home.stats.WordsToday != 0 {
		t.Fatalf("before the write: loaded %v/%v, words %d", e.a.home.loaded, e.a.hist.loaded, e.a.home.stats.WordsToday)
	}

	// The save goroutine, held by a slow disk until the gate opens.
	gate, saved := make(chan struct{}), make(chan struct{})
	go func() {
		<-gate
		en := store.HistoryEntry{CreatedAt: fixedNow.Add(-time.Minute), DurationMs: 3000, Transcript: text, FinalText: text,
			StyleID: "raw", Model: "parakeet-tdt-v3", App: &store.TargetApp{BundleID: "com.notes", Name: "Notes"}}
		en.Insert.Result = store.InsertPasted
		if _, err := e.db.InsertEntry(en); err != nil {
			t.Error(err)
		}
		e.a.HistoryChanged()
		close(saved)
	}()
	tt.Frame()
	if tt.HasText(text) {
		t.Fatal("the entry shows before it is written")
	}
	close(gate)
	<-saved
	// Written, but nothing read it yet: the reload waits on the main thread.
	tt.Frame()
	if tt.HasText(text) {
		t.Fatal("Home read the store without being told")
	}
	before := e.shell.redraws.Load()
	drain()
	if e.shell.redraws.Load() == before {
		t.Error("HistoryChanged redrew no window")
	}
	tt.Frame()
	if !tt.HasText(text) {
		t.Fatalf("Home Recent misses the new entry: %q", tt.Texts())
	}
	if e.a.home.stats.WordsToday != 6 {
		t.Errorf("Home Stats: %d words today, want 6", e.a.home.stats.WordsToday)
	}
	e.a.SetPage(PageHistory)
	tt.Frame()
	if !tt.HasText(text) {
		t.Fatalf("History misses the new entry: %q", tt.Texts())
	}
}
