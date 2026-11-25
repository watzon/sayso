package engine

import (
	"encoding/json"
	"testing"
)

func feed(t *testing.T, c *Client, lines ...string) {
	t.Helper()
	for _, l := range lines {
		m, err := parseMessage([]byte(l))
		if err != nil {
			t.Fatal(err)
		}
		c.event(m)
	}
}

func bareClient() *Client {
	return &Client{events: make(chan Event, 64), pending: map[string]*pending{}, statuses: map[string]ModelStatus{},
		engines: map[string]json.RawMessage{}, loaded: map[string]bool{}}
}

func TestDownloadProgressSurvivesTheModelStateThatFollowsIt(t *testing.T) {
	const progress = `{"v":1,"type":"download_progress","model":"m","fraction":0.42,"bytes_done":420,"bytes_total":1000}`
	for name, state := range map[string]string{
		"with the progress":    `{"v":1,"type":"model_state","model":"m","state":"downloading","fraction":0.42,"bytes_done":420,"bytes_total":1000}`,
		"without the progress": `{"v":1,"type":"model_state","model":"m","state":"downloading"}`,
	} {
		t.Run(name, func(t *testing.T) {
			c := bareClient()
			feed(t, c, progress, state)
			s, _ := c.Status("m")
			if s.State != StateDownloading || s.Fraction != 0.42 || s.BytesDone != 420 || s.BytesTotal != 1000 {
				t.Fatalf("status %+v", s)
			}
			var last Event
			for len(c.events) > 0 {
				last = <-c.events
			}
			if last.Status != s {
				t.Fatalf("the last event %+v is not the status %+v", last.Status, s)
			}
		})
	}
}

func TestANewDownloadStartsFromZero(t *testing.T) {
	c := bareClient()
	feed(t, c,
		`{"v":1,"type":"download_progress","model":"m","fraction":0.5,"bytes_done":500,"bytes_total":1000}`,
		`{"v":1,"type":"model_state","model":"m","state":"failed","message":"x"}`,
		`{"v":1,"type":"model_state","model":"m","state":"downloading"}`)
	if s, _ := c.Status("m"); s.State != StateDownloading || s.Fraction != 0 || s.BytesDone != 0 {
		t.Fatalf("status %+v", s)
	}
}
