package bridge

import (
	"encoding/json"
	"errors"
	"math"
	"reflect"
	"strings"
	"testing"
)

func TestEncodeKeepsTheOrderOfTheRustHost(t *testing.T) {
	id := "7"
	got := Encode(&id, ReplyOK(HelloReply{HostVersion: "0.15.0", Protocol: 1, Features: []string{}, Capabilities: []Capability{CapModelsRead}, Denied: []Capability{}}))
	want := `{"type":"ok","host_version":"0.15.0","protocol":1,"features":[],"capabilities":["models.read"],"denied":[],"v":1,"id":"7"}`
	if got != want {
		t.Fatalf("\n got %s\nwant %s", got, want)
	}
	if got := Encode(nil, ReplyOK(nil)); got != `{"type":"ok","v":1}` {
		t.Fatal(got)
	}
	if got := Encode(nil, HostClosing{}); got != `{"type":"host_closing","v":1}` {
		t.Fatal(got)
	}
	if got := Encode(nil, JobProgress{Job: 3, Fraction: 1}); got != `{"type":"job_progress","job":3,"fraction":1.0,"v":1}` {
		t.Fatal(got)
	}
	if got := Encode(nil, JobProgress{Job: 3, Fraction: 0.25}); got != `{"type":"job_progress","job":3,"fraction":0.25,"v":1}` {
		t.Fatal(got)
	}
	job := uint64(2)
	seg := Encode(nil, SegmentEvent{Job: &job, Segment: Segment{StartMs: 1, EndMs: 2, Text: "hi"}})
	if seg != `{"type":"segment","job":2,"start_ms":1,"end_ms":2,"text":"hi","v":1}` {
		t.Fatal(seg)
	}
	e := Encode(&id, capabilityError(CapAudioMic))
	if e != `{"type":"error","code":"capability","message":"the user did not grant the capability audio.mic","capability":"audio.mic","v":1,"id":"7"}` {
		t.Fatal(e)
	}
}

func TestDecodeRequests(t *testing.T) {
	d, err := DecodeRequest(`{"v":1,"type":"hello","id":"1","protocol":1,"capabilities":["audio.mic","camera.read"],"extra":true}`)
	if err != nil {
		t.Fatal(err)
	}
	m := d.(*Message[Request])
	h := m.Body.(*Hello)
	if *m.ID != "1" || h.Protocol != 1 || !reflect.DeepEqual(h.Capabilities, []Capability{CapAudioMic, "camera.read"}) {
		t.Fatalf("%+v %+v", m, h)
	}
	if Capability("camera.read").Known() {
		t.Fatal("unknown names stay unknown")
	}
	if d, _ := DecodeRequest(`{"type":"bye"}`); reflect.TypeOf(d.(*Message[Request]).Body) != reflect.TypeOf(&Bye{}) {
		t.Fatal("bye")
	}
	u, err := DecodeRequest(`{"type":"translate","id":"2","x":1}`)
	if err != nil || u.(*Unknown).Kind != "translate" || *u.(*Unknown).ID != "2" {
		t.Fatalf("%v %v", u, err)
	}
	var bad *BadFields
	if _, err := DecodeRequest(`{"type":"hello","id":"3"}`); !errors.As(err, &bad) || *bad.ID != "3" || !strings.Contains(bad.Message, "protocol") {
		t.Fatalf("%v", err)
	}
	if _, err := DecodeRequest(`{"type":"cancel_job","job":"x"}`); !errors.As(err, &bad) {
		t.Fatalf("wrong type: %v", err)
	}
	var not *NotAMessage
	for _, line := range []string{`nope`, `[1]`, `{"id":"1"}`, `{"type":1}`} {
		if _, err := DecodeRequest(line); !errors.As(err, &not) {
			t.Fatalf("%s: %v", line, err)
		}
	}
	if d, _ := DecodeRequest(`{"type":"list_models","id":5}`); d.(*Message[Request]).ID != nil {
		t.Fatal("an id that is not a string is no id")
	}
}

func TestDecodeHostMessages(t *testing.T) {
	d, err := DecodeHost(`{"type":"ok","host_version":"1","protocol":1,"features":[],"capabilities":[],"denied":[],"v":1,"id":"1"}`)
	if err != nil {
		t.Fatal(err)
	}
	var hr HelloReply
	if err := d.(*Message[any]).Body.(OKFields).Parse(&hr); err != nil || hr.HostVersion != "1" {
		t.Fatal(hr, err)
	}
	d, _ = DecodeHost(`{"type":"speakers","job":1,"turns":[{"start_ms":0,"end_ms":5,"speaker":2},{"start_ms":5,"end_ms":9,"speaker":"B"}]}`)
	sp := d.(*Message[any]).Body.(*SpeakersEvent)
	if sp.Turns[0].Speaker != "2" || sp.Turns[1].Speaker != "B" {
		t.Fatal(sp.Turns)
	}
	d, _ = DecodeHost(`{"type":"error","code":"busy","message":"m"}`)
	if e := d.(*Message[any]).Body.(*ErrorReply); *e.Code != CodeBusy || e.Error() != "m (busy)" {
		t.Fatal(e)
	}
}

func TestRequestCapabilities(t *testing.T) {
	if got := RequestCapabilities(&SessionStart{Sources: []Source{SourceMic, SourceSystem, SourceClient}}); !reflect.DeepEqual(got, []Capability{CapSessionLive, CapAudioMic, CapAudioSystem}) {
		t.Fatal(got)
	}
	for _, r := range []Request{&Hello{}, &Bye{}, &CancelJob{}, &SessionStop{}} {
		if len(RequestCapabilities(r)) != 0 {
			t.Fatal(r.Kind())
		}
	}
}

func TestPCM(t *testing.T) {
	if EncodeI16([]int16{1, -2}) != "AQD+/w==" {
		t.Fatal(EncodeI16([]int16{1, -2}))
	}
	s := []int16{0, 1, -1, 1000, -1000, math.MaxInt16, math.MinInt16}
	back, err := DecodeI16(EncodeI16(s))
	if err != nil || !reflect.DeepEqual(back, s) {
		t.Fatal(back, err)
	}
	f, _ := DecodeF32(EncodeI16(s))
	if again, _ := DecodeI16(EncodeF32(f)); !reflect.DeepEqual(again, s) {
		t.Fatal(again)
	}
	if clipped, _ := DecodeI16(EncodeF32([]float32{2, -2, 0.5})); !reflect.DeepEqual(clipped, []int16{math.MaxInt16, math.MinInt16, 16384}) {
		t.Fatal(clipped)
	}
	if _, err := DecodeI16("not base64!"); err == nil {
		t.Fatal("bad base64")
	}
	if _, err := DecodeI16("AQID"); err == nil {
		t.Fatal("odd bytes")
	}
}

func TestPartialText(t *testing.T) {
	for _, c := range [][3]string{
		{"Hello", "world", "Hello world"},
		{"Hello ", " world", "Hello world"},
		{"Hello", ", world", "Hello, world"},
		{"今日は", "いい天気", "今日はいい天気"},
		{"", "x", "x"},
		{"x", "", "x"},
	} {
		if got := (PartialEvent{Committed: c[0], Tentative: c[1]}).Text(); got != c[2] {
			t.Errorf("%q + %q = %q", c[0], c[1], got)
		}
	}
}

func TestEveryTypeHasAKind(t *testing.T) {
	for _, k := range RequestTypes {
		if newRequest(k).Kind() != k {
			t.Fatal(k)
		}
	}
	for _, k := range EventTypes {
		if newEvent(k).Kind() != k {
			t.Fatal(k)
		}
	}
	b, _ := json.Marshal(ModelInfo{ID: "x", Roles: []Role{}, Languages: []string{}})
	if strings.Contains(string(b), "choice") {
		t.Fatal("an empty choice is left out", string(b))
	}
}
