package enhance

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestParseModelListUsesNamesSortsAndDropsRepeats(t *testing.T) {
	body := `{"data":[{"id":"b/two","name":"Two"},{"id":"a/one"},{"id":"b/two","name":"Again"},{"id":"c","name":""},{"name":"no id"}]}`
	got, err := ParseModelList([]byte(body))
	if err != nil {
		t.Fatal(err)
	}
	want := [][2]string{{"a/one", "a/one"}, {"b/two", "Two"}, {"c", "c"}}
	if len(got) != len(want) {
		t.Fatalf("got %+v", got)
	}
	for i, w := range want {
		if got[i].ID != w[0] || got[i].Name != w[1] {
			t.Errorf("%d: %+v, want %v", i, got[i], w)
		}
	}
}

func TestParseModelListOllamaNullAndOtherShapes(t *testing.T) {
	if got, err := ParseModelList([]byte(`{"object":"list","data":null}`)); err != nil || len(got) != 0 {
		t.Errorf("null data: %v %v", got, err)
	}
	for _, bad := range []string{"nope", `{"models":[]}`} {
		if _, err := ParseModelList([]byte(bad)); err == nil {
			t.Errorf("%q parsed", bad)
		}
	}
}

func TestDetectOllamaReadsTagsAndSortsThem(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/tags" {
			http.NotFound(w, r)
			return
		}
		_, _ = w.Write([]byte(`{"models":[{"name":"qwen3:4b"},{"name":"llama3.2"}]}`))
	}))
	defer srv.Close()
	names, ok := DetectOllamaAt(srv.URL, time.Second)
	if !ok || len(names) != 2 || names[0] != "llama3.2" || names[1] != "qwen3:4b" {
		t.Fatalf("got %v %v", names, ok)
	}
	srv.Close()
	if _, ok := DetectOllamaAt(srv.URL, 200*time.Millisecond); ok {
		t.Error("a stopped server answered")
	}
}

func TestListModelsSendsTheKeyAndReportsHTTPErrors(t *testing.T) {
	var auth string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auth = r.Header.Get("Authorization")
		if r.URL.Path == "/v1/models" {
			_, _ = w.Write([]byte(`{"data":[{"id":"gpt-x"}]}`))
			return
		}
		http.Error(w, "nope", http.StatusUnauthorized)
	}))
	defer srv.Close()
	got, err := ListModels(srv.URL+"/v1/", "sk-test", time.Second)
	if err != nil || len(got) != 1 || got[0].ID != "gpt-x" || auth != "Bearer sk-test" {
		t.Fatalf("got %v %v auth %q", got, err, auth)
	}
	_, err = ListModels(srv.URL+"/other", "", time.Second)
	var e *Error
	if !errors.As(err, &e) || e.Kind != ErrHTTP || e.Status != 401 {
		t.Errorf("err %v", err)
	}
}

type fixedEnhancer struct {
	ms  uint64
	err error
}

func (f fixedEnhancer) ID() string    { return "f" }
func (f fixedEnhancer) IsCloud() bool { return false }
func (f fixedEnhancer) Enhance(r Request) (Response, error) {
	if r.Transcript == "" || r.Temperature == nil || *r.Temperature != 0 {
		return Response{}, errors.New("not the test request")
	}
	return Response{Text: r.Transcript, ElapsedMs: f.ms}, f.err
}

func TestTestProviderReportsTheTime(t *testing.T) {
	if ms, err := TestProvider(fixedEnhancer{ms: 840}); err != nil || ms != 840 {
		t.Errorf("got %d %v", ms, err)
	}
	if _, err := TestProvider(fixedEnhancer{err: errors.New("boom")}); err == nil {
		t.Error("an error passed")
	}
}
