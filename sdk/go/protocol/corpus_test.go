package protocol

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"testing"

	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol/corpus"
)

var update = flag.Bool("update", false, "write sdk/protocol/testdata/names.json")

func reencode(t *testing.T, line string) (string, any) {
	t.Helper()
	var head struct{ Type string }
	json.Unmarshal([]byte(line), &head)
	var decoded any
	var err error
	if IsRequestType(head.Type) {
		decoded, err = DecodeRequest(line)
	} else {
		decoded, err = DecodeHost(line)
	}
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	switch d := decoded.(type) {
	case *Message[Request]:
		return Encode(d.ID, d.Body), d.Body
	case *Message[any]:
		switch body := d.Body.(type) {
		case OKFields:
			m := map[string]json.RawMessage(body)
			return Encode(d.ID, ReplyOK(m)), body
		case *ErrorReply:
			return Encode(d.ID, body), body
		case Event:
			return Encode(d.ID, body), body
		}
	case *Unknown:
		return EncodeUnknown(d), d
	}
	t.Fatalf("decoded to %T", decoded)
	return "", nil
}

func TestTheHostCorpusRoundTrips(t *testing.T) {
	testbin.Passed(t, "P01")
	cases, err := corpus.Read("host", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(cases) < 80 {
		t.Fatalf("only %d cases", len(cases))
	}
	for _, c := range cases {
		t.Run(c.String(), func(t *testing.T) {
			got, _ := reencode(t, c.Message)
			if c.DecodeOnly {
				return
			}
			same, err := corpus.Same(c.Message, got)
			if err != nil || !same {
				t.Fatalf("\n in  %s\n out %s\n%v", c.Message, got, err)
			}
		})
	}
}

func TestEveryEventAndRequestTypeIsInTheCorpus(t *testing.T) {
	cases, err := corpus.Read("host", "")
	if err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	codes := map[string]bool{}
	for _, c := range cases {
		var head struct {
			Type string
			Code string
		}
		json.Unmarshal([]byte(c.Message), &head)
		seen[head.Type] = true
		codes[head.Code] = true
	}
	for _, kind := range slices.Concat(RequestTypes, EventTypes, []string{"ok", "error"}) {
		if !seen[kind] {
			t.Errorf("no case of %s", kind)
		}
	}
	for _, code := range ErrorCodes {
		if !codes[string(code)] {
			t.Errorf("no case of the error code %s", code)
		}
	}
}

func TestUnknownNamesAreKept(t *testing.T) {
	testbin.Passed(t, "P01")
	d, err := DecodeRequest(`{"type":"hello","protocol":1,"features":[],"capabilities":["calendar.read","models.read"],"v":1,"id":"x"}`)
	if err != nil {
		t.Fatal(err)
	}
	hello := d.(*Message[Request]).Body.(*Hello)
	if hello.Capabilities[0] != "calendar.read" || hello.Capabilities[0].Known() || !hello.Capabilities[1].Known() {
		t.Fatal(hello.Capabilities)
	}
	d, err = DecodeHost(`{"type":"error","code":"quota_exceeded","message":"later","v":1,"id":"x3"}`)
	if err != nil {
		t.Fatal(err)
	}
	if e := d.(*Message[any]).Body.(*ErrorReply).Err(); e.Code != "quota_exceeded" {
		t.Fatal(e)
	}
	d, _ = DecodeHost(`{"type":"ok","models":[{"id":"m","name":"M","roles":["translate"],"state":"warming","languages":[],"timestamps":"char","size_bytes":0}],"v":1,"id":"x4"}`)
	var list ListModelsReply
	if err := d.(*Message[any]).Body.(OKFields).Parse(&list); err != nil {
		t.Fatal(err)
	}
	if m := list.Models[0]; m.Roles[0] != "translate" || m.State != "warming" || m.Timestamps != "char" {
		t.Fatal(m)
	}
	d, _ = DecodeHost(`{"type":"job_done","job":9007199254740991,"text":"","v":1}`)
	if job := d.(*Message[any]).Body.(*JobDone).Job; job != 1<<53-1 {
		t.Fatal(job)
	}
}

func TestTheNamesFileIsCurrent(t *testing.T) {
	testbin.Passed(t, "P01")
	path := filepath.Join(corpus.Dir(), "names.json")
	want := NamesJSON()
	if *update {
		if err := os.WriteFile(path, want, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(bytes.ReplaceAll(got, []byte("\r\n"), []byte("\n")), want) {
		t.Fatal("names.json is out of date: run go test ./protocol -run TestTheNamesFileIsCurrent -update")
	}
}

type tableEnv struct {
	vars map[string]string
	temp string
	uid  *uint32
	sid  string
	home string
}

func (e tableEnv) Var(k string) string       { return e.vars[k] }
func (e tableEnv) DarwinUserTempDir() string { return e.temp }
func (e tableEnv) UID() (uint32, bool) {
	if e.uid == nil {
		return 0, false
	}
	return *e.uid, true
}
func (e tableEnv) UserSID() string { return e.sid }
func (e tableEnv) Home() string    { return e.home }

type pathsTable struct {
	Socket []struct {
		Case       string
		System     string
		Env        map[string]string
		DarwinTemp string `json:"darwin_temp"`
		UID        *uint32
		SID        string
		Host       string
		Client     string
		Error      bool
	}
	Data []struct {
		Case    string
		GOOS    string
		Home    string
		Env     map[string]string
		Data    string
		Models  string
		Store   string
		Cleanup string
	}
}

func TestThePathsTable(t *testing.T) {
	testbin.Passed(t, "P02")
	raw, err := os.ReadFile(filepath.Join(corpus.Dir(), "paths.json"))
	if err != nil {
		t.Fatal(err)
	}
	var table pathsTable
	if err := json.Unmarshal(raw, &table); err != nil {
		t.Fatal(err)
	}
	systems := map[string]System{"linux": Linux, "macos": MacOS, "windows": Windows, "other": OtherUnix}
	for _, c := range table.Socket {
		env := tableEnv{vars: c.Env, temp: c.DarwinTemp, uid: c.UID, sid: c.SID}
		host, herr := HostSocketPathFor(systems[c.System], env)
		client, cerr := ClientSocketPathFor(systems[c.System], env)
		if c.Error {
			if herr == nil || cerr == nil {
				t.Errorf("%s: want an error, got %q %q", c.Case, host, client)
			}
			continue
		}
		if herr != nil || cerr != nil || host != c.Host || client != c.Client {
			t.Errorf("%s: got %q %q (%v %v), want %q %q", c.Case, host, client, herr, cerr, c.Host, c.Client)
		}
	}
	if runtime.GOOS == "windows" {
		t.Log("the data cases use / paths; checked after ToSlash")
	}
	for _, c := range table.Data {
		data := DataDirFor(c.GOOS, tableEnv{vars: c.Env, home: filepath.FromSlash(c.Home)})
		models := ModelsDir(data)
		got := []string{data, models, ModelStoreDir(models), CleanupModelsDir(models)}
		want := []string{c.Data, c.Models, c.Store, c.Cleanup}
		for i := range got {
			if filepath.ToSlash(got[i]) != want[i] {
				t.Errorf("%s: got %q, want %q", c.Case, filepath.ToSlash(got[i]), want[i])
			}
		}
	}
}

func TestTheRealEnvironmentGivesASocketPath(t *testing.T) {
	p, err := ClientSocketPath()
	if err != nil || p == "" {
		t.Fatal(p, err)
	}
}
