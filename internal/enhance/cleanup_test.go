package enhance

import (
	"bufio"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/engine"
)

// The test binary is also a fake cleanup runtime.
func TestMain(m *testing.M) {
	if os.Getenv("SAYSO_FAKE") == "cleanup" {
		fakeCleanup()
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func fakeCleanup() {
	out := bufio.NewWriter(os.Stdout)
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	send := func(m map[string]any) {
		m["v"] = 1
		b, _ := json.Marshal(m)
		out.Write(append(b, '\n'))
		out.Flush()
	}
	for in.Scan() {
		var req map[string]any
		json.Unmarshal(in.Bytes(), &req)
		id := req["id"]
		switch req["type"] {
		case "hello":
			send(map[string]any{"id": id, "type": "hello", "version": 1, "llama_cpp": "b1", "backend": "cpu"})
		case "load":
			send(map[string]any{"id": id, "type": "loaded", "model_id": req["model_id"], "load_ms": 3})
		case "count":
			send(map[string]any{"id": id, "type": "count", "tokens": len(strings.Fields(req["text"].(string)))})
		case "complete":
			prompt := req["prompt"].(string)
			text := prompt[strings.Index(prompt, "]\n")+2 : strings.Index(prompt, "<|im_end|>\n<|im_start|>assistant")]
			stop := "end"
			if strings.Contains(text, "CUT") {
				stop = "limit"
			}
			send(map[string]any{"id": id, "type": "completion", "text": strings.ToUpper(strings.TrimSpace(text)) + "<|im_end|>", "stop": stop, "tokens_in": 10, "tokens_out": 4, "ms": 1})
		}
	}
}

func TestRender(t *testing.T) {
	s1, _ := FindCleanupModel("s1-mini")
	sp, _ := FindCleanupModel("spokenly-cleanup-1")
	p := Render(s1, "so um send the report by uh friday", Settings{}, "Slack", "", 10)
	want := "<|im_start|>system\n" + s1System + "<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: prose] [Context: general]\n" +
		"so um send the report by uh friday<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
	if p.Text != want || !reflect.DeepEqual(p.End, []string{"<|im_end|>"}) || p.MaxTokens != 45 {
		t.Fatalf("%q %d", p.Text, p.MaxTokens)
	}
	if !strings.Contains(Render(s1, "x", Settings{Layout: LayoutList}, "", "", 1).Text, "[Structure: lists] [Context: general]\nx<|im_end|>") ||
		!strings.Contains(Render(s1, "x", Settings{Format: FormatEmail}, "", "", 1).Text, "[Structure: prose] [Context: email]") ||
		!strings.Contains(Render(s1, "x", Settings{Tone: ToneVeryCasual}, "", "", 1).Text, "[Styling: semi-formal]") {
		t.Fatal("s1 settings")
	}
	if got := Render(sp, "hey are we still on", Settings{Tone: ToneCasual}, "Slack", "app.slack.com", 5).Text; got != "app: Slack\nsite: app.slack.com\ntone: casual\ntext: hey are we still on\nout:" {
		t.Fatal(got)
	}
	if got := Render(sp, "um hi", Settings{}, "", "  ", 2).Text; got != "tone: formal\ntext: um hi\nout:" {
		t.Fatal(got)
	}
	if got := Render(sp, "x", Settings{Tone: ToneVeryCasual}, "My\nApp", "", 1).Text; got != "app: My App\ntone: very casual\ntext: x\nout:" {
		t.Fatal(got)
	}
}

func TestRunsAndSettings(t *testing.T) {
	s1, _ := FindCleanupModel("s1-mini")
	sp, _ := FindCleanupModel("spokenly-cleanup-1")
	for _, c := range []struct {
		s          CleanupSettings
		onS1, onSp bool
	}{
		{NewCleanupSettings(ToneFormal, LayoutParagraphs, FormatPlain), true, true},
		{NewCleanupSettings(ToneCasual, LayoutParagraphs, FormatPlain), true, true},
		{NewCleanupSettings(ToneFormal, LayoutParagraphs, FormatEmail), true, false},
		{NewCleanupSettings(ToneFormal, LayoutList, FormatPlain), true, false},
	} {
		if s1.Runs(c.s) != c.onS1 || sp.Runs(c.s) != c.onSp {
			t.Fatal(c)
		}
	}
	table := "table"
	newer := CleanupSettings{Layout: &table}
	if _, ok := newer.Known(); ok || s1.Runs(newer) {
		t.Fatal("newer value")
	}
	if NewCleanupSettings(ToneFormal, LayoutParagraphs, FormatPlain) != (CleanupSettings{}) {
		t.Fatal("defaults written")
	}
	for _, m := range CleanupCatalog() {
		if len(m.Revision) != 40 || len(m.SHA256) != 64 || !strings.Contains(m.FileURL(m.File), m.Revision) || !m.SupportsLanguage("en") {
			t.Fatal(m.ID)
		}
	}
	if s1.SupportsLanguage("de") || !sp.SupportsLanguage("de") {
		t.Fatal("languages")
	}
}

func byBytes(text string) (uint32, error) { return uint32((len(text) + 3) / 4), nil }

func plan(t *testing.T, text string, max uint32) []string {
	parts, err := PlanParts(text, max, byBytes)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Join(parts, "") != text {
		t.Fatalf("not whole: %q", parts)
	}
	for _, p := range parts {
		if p == "" {
			t.Fatal("empty part")
		}
	}
	return parts
}

func TestPlanParts(t *testing.T) {
	if p := plan(t, "Hello there.", 100); len(p) != 1 {
		t.Fatal(p)
	}
	parts := plan(t, "one two three four. five six seven eight. nine ten eleven twelve. thirteen fourteen.", 12)
	for _, p := range parts[:len(parts)-1] {
		if n, _ := byBytes(p); !strings.HasSuffix(strings.TrimRight(p, " "), ".") || n > 12 {
			t.Fatalf("%q", p)
		}
	}
	if p := plan(t, "alpha beta gamma, delta epsilon zeta, eta theta iota, kappa lambda mu", 10); !strings.HasSuffix(strings.TrimRight(p[0], " "), ",") {
		t.Fatal(p)
	}
	for _, p := range plan(t, "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi", 6) {
		if n, _ := byBytes(p); n > 6 {
			t.Fatal(p)
		}
	}
	if p := plan(t, "这是一个很长的句子没有任何标点符号也没有空格所以只能按字符切分", 5); len(p) <= 3 {
		t.Fatal(p)
	}
	if p := plan(t, "第一句话。第二句话。第三句话。第四句话。", 8); !strings.HasSuffix(p[0], "。") {
		t.Fatal(p)
	}
	lines := plan(t, "first line here\nsecond line here\nthird line here", 5)
	if !strings.HasSuffix(lines[0], "\n") {
		t.Fatal(lines)
	}
	answers := make([]string, len(lines))
	for i, p := range lines {
		answers[i] = strings.ToUpper(strings.TrimSpace(p))
	}
	if j := JoinAnswers(lines, answers); !strings.Contains(j, "FIRST LINE HERE\nSECOND") {
		t.Fatal(j)
	}
	if JoinAnswers([]string{"One. ", "Two."}, []string{"One.", "Two."}) != "One. Two." || JoinAnswers([]string{"um. ", "Hi."}, []string{"", "Hi."}) != "Hi." {
		t.Fatal("join")
	}
	calls := 0
	_, err := PlanParts(strings.Repeat("word ", 500), 10, func(string) (uint32, error) { calls++; return 0, errors.New("stopped") })
	if err == nil || calls != 1 {
		t.Fatal(calls)
	}
}

func TestCheckAnswer(t *testing.T) {
	ok := func(in, stop, answer, want string) {
		t.Helper()
		got, err := CheckAnswer(in, stop, answer)
		if err != nil || got != want {
			t.Fatalf("%q: %q %v", in, got, err)
		}
	}
	fail := func(in, stop, answer, kind string) {
		t.Helper()
		_, err := CheckAnswer(in, stop, answer)
		var cf *CheckFailure
		if !errors.As(err, &cf) || cf.Kind != kind {
			t.Fatalf("%q: %v", in, err)
		}
	}
	ok("um hi there", "end", " Hi there.\n", "Hi there.")
	fail("um hi there", "limit", "Hi there", "cut")
	if _, err := CheckAnswer("x", "context", "Hi"); err.Error() != "the answer stopped early (Context)" {
		t.Fatal(err)
	}
	ok("hi", "end", "Hi.<|im_end|>", "Hi.")
	ok("hi", "end", "<think>\n\n</think>\n\nHi.", "Hi.")
	ok("um uh you know", "end", "", "")
	ok("Äh ähm.", "end", " ", "")
	fail("send the report by friday", "end", "", "empty")
	if IsFillerOnly("you") || IsFillerOnly("") || !IsFillerOnly("えっと、あのー") {
		t.Fatal("fillers")
	}
	ok("the total is nine hundred and ninety nine thousand nine hundred and ninety nine dollars", "end", "The total is $999,999.", "The total is $999,999.")
	fail("so the plan for next week is that we finish the import screen on monday and test it on tuesday", "end", "So the plan.", "too_short")
	zh := "我们下周的计划是周一完成导入界面然后周二用三个大文件测试它最后写发布说明"
	fail(zh, "end", "我们下周。", "too_short")
	if _, err := CheckAnswer(zh, "end", "我们下周的计划是：周一完成导入界面，然后周二用三个大文件测试它，最后写发布说明。"); err != nil {
		t.Fatal(err)
	}
	fail("hi", "end", strings.Repeat("word ", 100), "too_long")
	for in, n := range map[string]int{"nine hundred and ninety nine dollars": 2, "$999,999 at 5:30 PM": 4, "bread and butter": 3, "我们下周": 4, "Wir sehen uns um 3 Uhr.": 6} {
		if got := contentUnits(in); got != n {
			t.Errorf("%q: %d", in, got)
		}
	}
}

func TestRestoreSpelling(t *testing.T) {
	terms := []string{"GPUI", "GitHub", "US"}
	cases := [][3]string{
		{"open the GPUI docs", "Open the Gpui docs.", "Open the GPUI docs."},
		{"push it to GitHub", "push it to github", "push it to GitHub"},
		{"tell us more", "Tell us more.", "Tell us more."},
		{"GPUI", "gpuis are fun", "gpuis are fun"},
	}
	for _, c := range cases {
		if got := RestoreSpelling(c[0], c[1], terms); got != c[2] {
			t.Errorf("%q: %q", c[1], got)
		}
	}
	t2 := []string{"Claude", "Claude Code", "Zoë"}
	if got := RestoreSpelling("ask Claude Code and Zoë", "Ask claude code and zoë.", t2); got != "Ask Claude Code and Zoë." {
		t.Fatal(got)
	}
	if got := RestoreSpelling("ask Claude", "ask claude", t2); got != "ask Claude" {
		t.Fatal(got)
	}
	if got := RestoreSpelling("we may ship in May", "We may ship in may.", []string{"May"}); got != "We may ship in may." {
		t.Fatal(got)
	}
}

func TestLanguages(t *testing.T) {
	never := func(string) (string, bool) { panic("no detection needed") }
	if l, _ := DictationLanguage("de", []string{"en", "de"}, "x", never); l != "de" {
		t.Fatal(l)
	}
	if l, _ := DictationLanguage("auto", []string{"en"}, "x", never); l != "en" {
		t.Fatal(l)
	}
	if _, ok := DictationLanguage("auto", []string{"en", "de"}, "ok", func(string) (string, bool) { return "", false }); ok {
		t.Fatal("unsure")
	}
	if l, _ := DetectLanguage("We need to fix the login bug and update the docs before the release on Friday."); l != "en" {
		t.Fatal(l)
	}
	if l, _ := DetectLanguage("Wir müssen den Fehler beim Anmelden beheben und die Dokumentation vor Freitag aktualisieren."); l != "de" {
		t.Fatal(l)
	}
	if _, ok := DetectLanguage("ok"); ok {
		t.Fatal("ok is not sure")
	}
}

// testModel is a catalog entry for a small file served by a test server.
func testModel(content []byte) *CleanupModelInfo {
	sum := sha256.Sum256(content)
	return &CleanupModelInfo{ID: "test-model", Name: "Test", Repo: "test/repo", Revision: "rev1", File: "test.gguf",
		SizeBytes: uint64(len(content)), SHA256: hex.EncodeToString(sum[:]), LicenseFiles: []string{"LICENSE"},
		Languages: []string{"en"}, Layouts: []Layout{LayoutParagraphs}, Formats: []Format{FormatPlain}, Prompt: PromptS1Chat}
}

func TestDownloadResumesAndVerifies(t *testing.T) {
	content := []byte(strings.Repeat("0123456789abcdef", 10000))
	var mu sync.Mutex
	var ranges []string
	first := true
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/test/repo/resolve/rev1/LICENSE":
			w.Write([]byte("MIT"))
		case "/test/repo/resolve/rev1/test.gguf":
			mu.Lock()
			ranges = append(ranges, r.Header.Get("Range"))
			cut := first
			first = false
			mu.Unlock()
			var from int
			if rg := r.Header.Get("Range"); rg != "" {
				fmt.Sscanf(rg, "bytes=%d-", &from)
				w.Header().Set("Content-Range", fmt.Sprintf("bytes %d-%d/%d", from, len(content)-1, len(content)))
				w.Header().Set("Content-Length", fmt.Sprint(len(content)-from))
				w.WriteHeader(206)
			} else {
				w.Header().Set("Content-Length", fmt.Sprint(len(content)))
				w.WriteHeader(200)
			}
			if cut {
				// Half the file, then the connection drops.
				w.Write(content[:len(content)/2])
				w.(http.Flusher).Flush()
				hj, _ := w.(http.Hijacker)
				conn, _, _ := hj.Hijack()
				conn.Close()
				return
			}
			w.Write(content[from:])
		default:
			w.WriteHeader(404)
		}
	}))
	defer srv.Close()
	dir := t.TempDir()
	m := testModel(content)
	var cancel atomic.Bool
	var last DownloadProgress
	path, err := DownloadCleanupModel(dir, m, srv.URL, &cancel, func(p DownloadProgress) {
		if p.BytesDone < last.BytesDone {
			t.Error("progress went backwards")
		}
		last = p
	})
	if err != nil {
		t.Fatal(err)
	}
	if last.BytesDone != m.SizeBytes || !CleanupModelReady(dir, m) {
		t.Fatal(last)
	}
	if p, ok := CleanupModelPath(dir, m); !ok || p != path {
		t.Fatal(p)
	}
	if lic, _ := os.ReadFile(filepath.Join(CleanupModelDir(dir, m), "LICENSE")); string(lic) != "MIT" {
		t.Fatal("license")
	}
	if len(ranges) != 2 || ranges[0] != "" || ranges[1] != fmt.Sprintf("bytes=%d-", len(content)/2) {
		t.Fatal(ranges)
	}
	ready, _ := os.ReadFile(filepath.Join(CleanupModelDir(dir, m), "ready.json"))
	if string(ready) != `{"file":"test.gguf","revision":"rev1","sha256":"`+m.SHA256+`"}` {
		t.Fatal(string(ready))
	}
	// A wrong hash fails and leaves no partial file.
	bad := testModel(content)
	bad.ID, bad.SHA256 = "bad-model", strings.Repeat("0", 64)
	_, err = DownloadCleanupModel(dir, bad, srv.URL, &cancel, func(DownloadProgress) {})
	var de *DownloadError
	if !errors.As(err, &de) || de.Kind != "wrong_hash" {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(CleanupModelDir(dir, bad), "test.gguf.partial")); err == nil {
		t.Fatal("partial left")
	}
	if err := DeleteCleanupModel(dir, m); err != nil || CleanupModelReady(dir, m) {
		t.Fatal(err)
	}
	if DeleteCleanupModel(dir, m) != nil {
		t.Fatal("delete of a missing model")
	}
}

func TestRuntimeCleaner(t *testing.T) {
	exe, _ := os.Executable()
	t.Setenv("SAYSO_FAKE", "cleanup")
	dir := t.TempDir()
	m, _ := FindCleanupModel("s1-mini")
	mdir := CleanupModelDir(dir, m)
	os.MkdirAll(mdir, 0o755)
	os.WriteFile(filepath.Join(mdir, m.File), []byte("x"), 0o644)
	ready, _ := json.Marshal(map[string]string{"revision": m.Revision, "sha256": m.SHA256, "file": m.File})
	os.WriteFile(filepath.Join(mdir, "ready.json"), ready, 0o644)
	rt := engine.NewCleanupClient(exe)
	defer rt.Stop()
	c := &RuntimeCleaner{Runtime: rt, ModelsDir: dir}
	resp, err := c.Clean(CleanupRequest{Model: "s1-mini", Text: "send the report by friday", Timeout: 5 * time.Second})
	if err != nil || resp.Text != "SEND THE REPORT BY FRIDAY" || resp.Parts != 1 || resp.TokensIn != 10 || resp.Model != "s1-mini" {
		t.Fatal(resp, err)
	}
	_, err = c.Clean(CleanupRequest{Model: "s1-mini", Text: "CUT here", Timeout: 5 * time.Second})
	var ce *CleanError
	if !errors.As(err, &ce) || ce.Kind != "invalid_output" || ce.ToEnhanceError().Kind != ErrInvalidOutput {
		t.Fatal(err)
	}
	if _, err := c.Clean(CleanupRequest{Model: "spokenly-cleanup-1", Text: "x"}); !errors.As(err, &ce) || ce.Kind != "not_configured" {
		t.Fatal(err)
	}
	broken := &RuntimeCleaner{Runtime: engine.NewCleanupClient(filepath.Join(dir, "missing")), ModelsDir: dir}
	if _, err := broken.Clean(CleanupRequest{Model: "s1-mini", Text: "x"}); !errors.As(err, &ce) || ce.Kind != "not_configured" {
		t.Fatal(err)
	}
}
