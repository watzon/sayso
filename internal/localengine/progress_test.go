package localengine

import (
	"net/http"
	"net/http/httptest"
	"strconv"
	"testing"
	"time"
)

// slowFile serves size bytes in pieces, with a pause after each, so that
// progress messages fall between them. withLength false sends it chunked.
func slowFile(size, pieces int, withLength bool) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if withLength {
			w.Header().Set("Content-Length", strconv.Itoa(size))
		}
		piece := make([]byte, size/pieces)
		for range pieces {
			w.Write(piece)
			w.(http.Flusher).Flush()
			time.Sleep(progressInterval + 50*time.Millisecond)
		}
	})
}

func progressMessages(messages []map[string]any) []map[string]any {
	var out []map[string]any
	for _, m := range messages {
		if m["type"] == "download_progress" {
			out = append(out, m)
		}
	}
	return out
}

func TestADownloadStreamsProgressWhileTheBodyArrives(t *testing.T) {
	const size, pieces = 4000, 4
	for _, tc := range []struct {
		name       string
		withLength bool
	}{{"content length", true}, {"chunked", false}} {
		t.Run(tc.name, func(t *testing.T) {
			srv := httptest.NewServer(slowFile(size, pieces, tc.withLength))
			defer srv.Close()
			h := newHarness(t, srv.URL)
			r := h.call(obj{"v": 1, "type": "download", "id": "d1", "model": "vad", "engine": silero()})
			if last(r)["type"] != "ok" {
				t.Fatalf("%v", r)
			}
			msgs := progressMessages(r)
			var middle []map[string]any
			for _, m := range msgs {
				if d := m["bytes_done"].(float64); d > 0 && d < size {
					middle = append(middle, m)
				}
			}
			if len(middle) < 2 {
				t.Fatalf("want progress while the body arrives, got %v", msgs)
			}
			prevDone, prevFraction := 0.0, 0.0
			for _, m := range middle {
				done, fraction, total := m["bytes_done"].(float64), m["fraction"].(float64), m["bytes_total"].(float64)
				if done <= prevDone {
					t.Fatalf("bytes_done does not advance: %v", middle)
				}
				if tc.withLength {
					if total != size || fraction <= prevFraction || fraction != done/size {
						t.Fatalf("the total comes from Content-Length: %v", m)
					}
				} else if total != 0 || fraction != 0 {
					t.Fatalf("an unknown total is 0 with fraction 0: %v", m)
				}
				prevDone, prevFraction = done, fraction
			}
			for _, m := range r {
				if m["type"] == "model_state" && m["state"] == "downloading" {
					if _, ok := m["bytes_done"]; !ok {
						t.Fatalf("model_state downloading lacks the progress: %v", m)
					}
				}
			}
		})
	}
}
