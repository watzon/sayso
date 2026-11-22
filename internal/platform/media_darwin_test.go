package platform

import (
	"errors"
	"reflect"
	"strings"
	"testing"
)

// scriptRunner fakes osascript: each "get" gives the next of gets (the last
// one repeats), and every call is recorded.
type scriptRunner struct {
	gets  []string
	calls []string
	err   error
}

func (r *scriptRunner) run(args ...string) (string, error) {
	r.calls = append(r.calls, strings.Join(args, " "))
	if r.err != nil {
		return "", r.err
	}
	if args[0] == "send" {
		return `{"sent":true}`, nil
	}
	out := r.gets[0]
	if len(r.gets) > 1 {
		r.gets = r.gets[1:]
	}
	return out, nil
}

func (r *scriptRunner) sends() []string {
	var out []string
	for _, c := range r.calls {
		if strings.HasPrefix(c, "send") {
			out = append(out, c)
		}
	}
	return out
}

func fakeMedia(gets ...string) (*macMedia, *scriptRunner) {
	r := &scriptRunner{gets: gets}
	return &macMedia{backend: &jxaMedia{run: r.run}, fallback: func() mediaBackend { return noMedia{} }}, r
}

const (
	spotifyPlaying = `{"playing":true,"bundleIdentifier":"com.spotify.client","processIdentifier":812}`
	spotifyPaused  = `{"playing":false,"bundleIdentifier":"com.spotify.client","processIdentifier":812}`
)

func TestMacMediaPausesAndResumesThePlayer(t *testing.T) {
	m, r := fakeMedia(spotifyPlaying, spotifyPaused)
	was, err := m.Pause()
	if !was || err != nil {
		t.Fatalf("pause: %v %v", was, err)
	}
	if err := m.Resume(); err != nil {
		t.Fatal(err)
	}
	if got, want := r.sends(), []string{"send 1", "send 0"}; !reflect.DeepEqual(got, want) {
		t.Errorf("sends %v, want %v", got, want)
	}
}

func TestMacMediaNotPlayingStartsNothing(t *testing.T) {
	for name, get := range map[string]string{"paused": spotifyPaused, "none": "null"} {
		m, r := fakeMedia(get)
		if was, err := m.Pause(); was || err != nil {
			t.Fatalf("%s: pause %v %v", name, was, err)
		}
		if err := m.Resume(); err != nil {
			t.Fatal(err)
		}
		if len(r.sends()) != 0 {
			t.Errorf("%s: sent %v", name, r.sends())
		}
		// The resume reads nothing either: nothing was paused.
		if len(r.calls) != 1 {
			t.Errorf("%s: calls %v", name, r.calls)
		}
	}
}

func TestMacMediaResumesOnlyThePausedPlayer(t *testing.T) {
	for name, after := range map[string]string{
		"other app":     `{"playing":false,"bundleIdentifier":"com.apple.Music","processIdentifier":90}`,
		"new process":   `{"playing":false,"bundleIdentifier":"com.spotify.client","processIdentifier":813}`,
		"none":          "null",
		"playing again": spotifyPlaying,
	} {
		m, r := fakeMedia(spotifyPlaying, after)
		if was, _ := m.Pause(); !was {
			t.Fatalf("%s: not paused", name)
		}
		if err := m.Resume(); err != nil {
			t.Fatal(err)
		}
		if got := r.sends(); !reflect.DeepEqual(got, []string{"send 1"}) {
			t.Errorf("%s: sends %v", name, got)
		}
	}
}

func TestMacMediaResumeWaitsForThePause(t *testing.T) {
	// The player still reports "playing" right after the pause.
	m, r := fakeMedia(spotifyPlaying, spotifyPlaying, spotifyPaused)
	m.Pause()
	if err := m.Resume(); err != nil {
		t.Fatal(err)
	}
	if got := r.sends(); !reflect.DeepEqual(got, []string{"send 1", "send 0"}) {
		t.Errorf("sends %v", got)
	}
}

func TestMacMediaReportsScriptErrors(t *testing.T) {
	m, r := fakeMedia(spotifyPlaying)
	r.err = errors.New("osascript: timeout")
	if was, err := m.Pause(); was || err == nil {
		t.Errorf("pause: %v %v", was, err)
	}
}

type fakeBackend struct {
	now   *nowPlaying
	sends []int
}

func (f *fakeBackend) nowPlaying() (*nowPlaying, error) { return f.now, nil }
func (f *fakeBackend) send(c int) error                 { f.sends = append(f.sends, c); return nil }

func TestMacMediaFallsBackWithoutNowPlayingRequest(t *testing.T) {
	r := &scriptRunner{gets: []string{`{"unsupported":true}`}}
	direct := &fakeBackend{now: &nowPlaying{player: "812", playing: true}}
	made := 0
	m := &macMedia{backend: &jxaMedia{run: r.run}, fallback: func() mediaBackend { made++; return direct }}
	if was, err := m.Pause(); !was || err != nil {
		t.Fatalf("pause: %v %v", was, err)
	}
	direct.now = &nowPlaying{player: "812"}
	if err := m.Resume(); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(direct.sends, []int{mediaPause, mediaPlay}) || made != 1 {
		t.Errorf("sends %v, fallbacks made %d", direct.sends, made)
	}
	// The script ran once: after it, the fallback answers.
	if len(r.calls) != 1 {
		t.Errorf("script calls %v", r.calls)
	}
}
