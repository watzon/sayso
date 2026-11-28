package deepgram

import (
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/speech"
)

type seen struct {
	url, auth string
	size      int
}

// server gives each reply in turn and records each request.
func server(t *testing.T, replies ...[2]string) (string, func() []seen) {
	var mu sync.Mutex
	var got []seen
	n := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		got = append(got, seen{r.URL.String(), r.Header.Get("Authorization"), len(b)})
		reply := replies[n]
		n++
		mu.Unlock()
		var status int
		fmt.Sscan(reply[0], &status)
		w.WriteHeader(status)
		w.Write([]byte(reply[1]))
	}))
	t.Cleanup(srv.Close)
	return srv.URL + "/v1", func() []seen { mu.Lock(); defer mu.Unlock(); return got }
}

const ok = `{"results":{"channels":[{"alternatives":[{"transcript":" Then push the fix. "}]}]}}`

func TestItSendsTheAudioAndReadsTheTranscript(t *testing.T) {
	url, got := server(t, [2]string{"200", ok}, [2]string{"200", ok})
	c := WithBaseURL("secret", url)
	for _, lang := range []string{"de", "auto"} {
		if text, err := c.Transcribe(make([]float32, 1600), lang); err != nil || text != "Then push the fix." {
			t.Fatal(text, err)
		}
	}
	s := got()
	if !strings.Contains(s[0].url, "model=nova-3") || !strings.Contains(s[0].url, "language=de") || s[0].auth != "Token secret" || s[0].size <= 3200 {
		t.Fatalf("%+v", s[0])
	}
	if !strings.Contains(s[1].url, "detect_language=true") {
		t.Fatal(s[1].url)
	}
}

func code(err error) protocol.ErrorCode {
	var e *speech.EngineError
	if errors.As(err, &e) {
		return e.Code
	}
	return "?"
}

func TestAFailureSaysWhatKindItIs(t *testing.T) {
	url, _ := server(t, [2]string{"401", "{}"}, [2]string{"429", "{}"}, [2]string{"503", "{}"}, [2]string{"400", "bad"}, [2]string{"200", "{}"})
	c := WithBaseURL("secret", url)
	for i, want := range []protocol.ErrorCode{CodeKey, CodeConnection, CodeConnection, "", ""} {
		if _, err := c.Transcribe(make([]float32, 160), "en"); code(err) != want {
			t.Fatalf("%d: %v", i, err)
		}
	}
}

func TestNoServerIsALostConnection(t *testing.T) {
	c := WithBaseURL("secret", "http://127.0.0.1:9/v1")
	if _, err := c.Transcribe(make([]float32, 160), "en"); code(err) != CodeConnection {
		t.Fatal(err)
	}
}

func TestTheKeyDoesNotShowInText(t *testing.T) {
	c := New("secret")
	for _, s := range []string{fmt.Sprint(c), fmt.Sprintf("%v %+v %#v", c, c, c)} {
		if strings.Contains(s, "secret") {
			t.Fatal(s)
		}
	}
}

type localEngine struct{ speech.Engine }

func (localEngine) Load(string) error { return errors.New("local") }
func (localEngine) Transcribe([]float32, string, string) (string, error) {
	return "local", nil
}

func TestTheFinalPassOfTheModelGoesToDeepgram(t *testing.T) {
	url, _ := server(t, [2]string{"200", ok})
	f := NewFinal(localEngine{}, WithBaseURL("k", url))
	if f.Load(Model) != nil || f.Load("other") == nil || !f.Status(Model).IsUsable() {
		t.Fatal("load")
	}
	segs, err := f.TranscribeSegments(make([]float32, 16000), Model, "en")
	if err != nil || len(segs) != 1 || segs[0].EndMs != 1000 || segs[0].Text != "Then push the fix." {
		t.Fatal(segs, err)
	}
	if text, _ := f.Transcribe(nil, "other", "en"); text != "local" {
		t.Fatal(text)
	}
}
