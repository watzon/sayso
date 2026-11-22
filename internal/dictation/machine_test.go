package dictation

import (
	"slices"
	"testing"
)

func m() *Machine { return NewMachine(DefaultTiming()) }

func has(fx []Effect, want Effect) bool { return slices.Contains(fx, want) }

func recordAndTranscribe(mc *Machine, text string) SessionID {
	mc.Handle(Event{Kind: EvToggle}, 0)
	mc.Handle(Event{Kind: EvToggle}, 2000)
	s, _ := mc.CurrentSession()
	mc.Handle(Event{Kind: EvTranscribed, Session: s, Text: text}, 2100)
	return s
}

func TestAMicrophoneThatDoesNotStartShowsAnError(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	fx := mc.Handle(Event{Kind: EvCaptureFailed, Session: 1, Message: "no microphone"}, 50)
	if !slices.Equal(fx, []Effect{{Kind: FxDiscard, Session: 1}}) {
		t.Fatal(fx)
	}
	if mc.State != (State{Kind: StateShowing, Session: 1, Notice: Notice{Kind: NoticeError, Message: "no microphone"}, UntilMs: 4050}) {
		t.Fatalf("%+v", mc.State)
	}
	if len(mc.Handle(Event{Kind: EvUndo}, 100)) != 0 {
		t.Fatal("nothing to undo")
	}
	if mc.State.Phase() != "failed" {
		t.Fatal(mc.State.Phase())
	}
}

func TestALateCaptureFailureOfAnOldSessionIsIgnored(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	mc.Handle(Event{Kind: EvCancel}, 1000)
	mc.Handle(Event{Kind: EvToggle}, 2000)
	if s, _ := mc.CurrentSession(); s != 2 {
		t.Fatal(s)
	}
	if len(mc.Handle(Event{Kind: EvCaptureFailed, Session: 1, Message: "late"}, 2100)) != 0 || !mc.IsRecording() {
		t.Fatal("late failure")
	}
}

func TestToggleRecordsAndStops(t *testing.T) {
	mc := m()
	fx := mc.Handle(Event{Kind: EvToggle}, 0)
	if !has(fx, Effect{Kind: FxStartCapture, Session: 1}) || !has(fx, Effect{Kind: FxPlaySound, Sound: SoundStart}) || !mc.IsRecording() || mc.State.Phase() != "recording" {
		t.Fatal(fx)
	}
	fx = mc.Handle(Event{Kind: EvToggle}, 1500)
	if !has(fx, Effect{Kind: FxStopCapture, Session: 1}) || !has(fx, Effect{Kind: FxTranscribe, Session: 1}) {
		t.Fatal(fx)
	}
	if mc.State != (State{Kind: StateProcessing, Session: 1, Stage: StageTranscribing, RecordedMs: 1500}) || mc.State.Phase() != "processing" {
		t.Fatalf("%+v", mc.State)
	}
}

func TestHappyPathInsertsAndSaves(t *testing.T) {
	mc := m()
	s := recordAndTranscribe(mc, "hello")
	mc.Handle(Event{Kind: EvEnhanceStarted, Session: s}, 2150)
	if mc.State.Stage != StageEnhancing {
		t.Fatal(mc.State)
	}
	fx := mc.Handle(Event{Kind: EvReadyToInsert, Session: s, Text: "Hello.", StyleName: "Clean"}, 2500)
	if !has(fx, Effect{Kind: FxInsert, Session: s, Text: "Hello."}) || mc.State.Phase() != "inserting" {
		t.Fatal(fx)
	}
	fx = mc.Handle(Event{Kind: EvInserted, Session: s}, 2600)
	if !has(fx, Effect{Kind: FxSaveHistory, Session: s}) {
		t.Fatal(fx)
	}
	if mc.State != (State{Kind: StateShowing, Session: s, Notice: Notice{Kind: NoticeInserted, StyleName: "Clean"}, UntilMs: 3800}) || mc.State.Phase() != "done" {
		t.Fatalf("%+v", mc.State)
	}
	mc.Handle(Event{Kind: EvTick}, 3799)
	if mc.State.Kind != StateShowing {
		t.Fatal("early")
	}
	mc.Handle(Event{Kind: EvTick}, 3800)
	if mc.State.Kind != StateIdle {
		t.Fatal("not idle")
	}
}

func TestStyleFailureShowsRawNoticeLonger(t *testing.T) {
	mc := m()
	s := recordAndTranscribe(mc, "hello")
	mc.Handle(Event{Kind: EvReadyToInsert, Session: s, Text: "hello", HasEnhanceErr: true, EnhanceError: "OpenRouter timed out after 4 s"}, 2200)
	mc.Handle(Event{Kind: EvInserted, Session: s}, 2300)
	if mc.State.Notice.Kind != NoticeInsertedRaw || mc.State.UntilMs != 4900 {
		t.Fatalf("%+v", mc.State)
	}
}

func TestPushToTalkRecordsWhileHeld(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvPushToTalkDown}, 0)
	if len(mc.Handle(Event{Kind: EvToggle}, 100)) != 0 {
		t.Fatal("a toggle while holding push-to-talk")
	}
	if !has(mc.Handle(Event{Kind: EvPushToTalkUp}, 900), Effect{Kind: FxTranscribe, Session: 1}) {
		t.Fatal("up")
	}
}

func TestShortPressIsDiscardedSilently(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvPushToTalkDown}, 0)
	fx := mc.Handle(Event{Kind: EvPushToTalkUp}, 120)
	if !has(fx, Effect{Kind: FxDiscard, Session: 1}) || slices.ContainsFunc(fx, func(e Effect) bool { return e.Kind == FxPlaySound }) || mc.State.Kind != StateIdle {
		t.Fatal(fx)
	}
}

func TestCancelThenUndoTranscribes(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	fx := mc.Handle(Event{Kind: EvCancel}, 1000)
	if !has(fx, Effect{Kind: FxStopCapture, Session: 1}) || slices.ContainsFunc(fx, func(e Effect) bool { return e.Kind == FxDiscard }) || mc.State.Phase() != "cancelled" {
		t.Fatal(fx)
	}
	if !has(mc.Handle(Event{Kind: EvUndo}, 2000), Effect{Kind: FxTranscribe, Session: 1}) || mc.State.Kind != StateProcessing {
		t.Fatal("undo")
	}
}

func TestCancelUndoWindowExpires(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	mc.Handle(Event{Kind: EvCancel}, 1000)
	if !has(mc.Handle(Event{Kind: EvTick}, 6000), Effect{Kind: FxDiscard, Session: 1}) || mc.State.Kind != StateIdle {
		t.Fatal("expire")
	}
	if len(mc.Handle(Event{Kind: EvUndo}, 6100)) != 0 {
		t.Fatal("undo after the window")
	}
}

func TestCancelDuringProcessingIgnoresLateResults(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	mc.Handle(Event{Kind: EvToggle}, 1000)
	mc.Handle(Event{Kind: EvCancel}, 1100)
	if len(mc.Handle(Event{Kind: EvTranscribed, Session: 1, Text: "late"}, 1200)) != 0 || mc.State.Notice.Kind != NoticeCancelled {
		t.Fatal(mc.State)
	}
}

func TestStartingOverACancelDiscardsTheOldAudio(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	mc.Handle(Event{Kind: EvCancel}, 1000)
	fx := mc.Handle(Event{Kind: EvToggle}, 1500)
	if !has(fx, Effect{Kind: FxDiscard, Session: 1}) || !has(fx, Effect{Kind: FxStartCapture, Session: 2}) {
		t.Fatal(fx)
	}
}

func TestAnEmptyTextAfterTheStyleInsertsNothing(t *testing.T) {
	mc := m()
	s := recordAndTranscribe(mc, "um uh")
	fx := mc.Handle(Event{Kind: EvReadyToInsert, Session: s, Text: " "}, 2200)
	if slices.ContainsFunc(fx, func(e Effect) bool { return e.Kind == FxInsert }) || !has(fx, Effect{Kind: FxDiscard, Session: s}) || mc.State.Notice.Kind != NoticeNoSpeech {
		t.Fatal(fx)
	}
}

func TestInsertFailureStaysUntilDismissed(t *testing.T) {
	mc := m()
	s := recordAndTranscribe(mc, "hi")
	mc.Handle(Event{Kind: EvReadyToInsert, Session: s, Text: "Hi."}, 2200)
	if !has(mc.Handle(Event{Kind: EvInsertFailed, Session: s, Text: "Hi.", Reason: "No text field is focused"}, 2300), Effect{Kind: FxSaveHistory, Session: s}) {
		t.Fatal("save")
	}
	mc.Handle(Event{Kind: EvTick}, 999_999)
	if mc.State.Notice.Kind != NoticeInsertFailed || !mc.State.Stays {
		t.Fatal(mc.State)
	}
	mc.Handle(Event{Kind: EvDismiss}, 1_000_000)
	if mc.State.Kind != StateIdle {
		t.Fatal("dismiss")
	}
	s = recordAndTranscribe(mc, "hi")
	mc.Handle(Event{Kind: EvReadyToInsert, Session: s, Text: "Hi."}, 2200)
	mc.Handle(Event{Kind: EvInsertFailed, Session: s, Text: "Hi.", Reason: "x"}, 2300)
	if !has(mc.Handle(Event{Kind: EvToggle}, 3000), Effect{Kind: FxStartCapture, Session: s + 1}) {
		t.Fatal("a new dictation replaces a failure card")
	}
}

func TestSilenceAndEngineFailure(t *testing.T) {
	mc := m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	mc.Handle(Event{Kind: EvToggle}, 1000)
	if !has(mc.Handle(Event{Kind: EvTranscribed, Session: 1, Text: "  "}, 1100), Effect{Kind: FxDiscard, Session: 1}) || mc.State.Notice.Kind != NoticeNoSpeech {
		t.Fatal("silence")
	}
	mc = m()
	mc.Handle(Event{Kind: EvToggle}, 0)
	mc.Handle(Event{Kind: EvToggle}, 1000)
	if !has(mc.Handle(Event{Kind: EvTranscribeFailed, Session: 1, Message: "Engine stopped"}, 1100), Effect{Kind: FxSaveHistory, Session: 1}) {
		t.Fatal("history keeps the audio")
	}
}

func TestMaxDurationAndStaleEvents(t *testing.T) {
	tm := DefaultTiming()
	tm.MaxDurationMs = 10_000
	mc := NewMachine(tm)
	mc.Handle(Event{Kind: EvToggle}, 0)
	if len(mc.Handle(Event{Kind: EvTick}, 9_999)) != 0 {
		t.Fatal("early")
	}
	if !has(mc.Handle(Event{Kind: EvTick}, 10_000), Effect{Kind: FxTranscribe, Session: 1}) {
		t.Fatal("max")
	}
	mc = m()
	for _, e := range []Event{{Kind: EvPushToTalkUp}, {Kind: EvUndo}, {Kind: EvInserted, Session: 9}} {
		if len(mc.Handle(e, 0)) != 0 {
			t.Fatal(e)
		}
	}
	if mc.State.Kind != StateIdle {
		t.Fatal("idle")
	}
}
