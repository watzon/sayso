package enhance

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

// The app calls DownloadCleanupModel with no endpoint and no cancel flag.
func TestADownloadWithoutAnEndpointOrACancelFlagRuns(t *testing.T) {
	content := []byte("gguf model bytes")
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/test/repo/resolve/rev1/LICENSE":
			w.Write([]byte("MIT"))
		case "/test/repo/resolve/rev1/test.gguf":
			w.Write(content)
		default:
			w.WriteHeader(404)
		}
	}))
	defer srv.Close()
	t.Setenv("SAYSO_HF_ENDPOINT", srv.URL)
	dir := t.TempDir()
	m := testModel(content)
	var last DownloadProgress
	if _, err := DownloadCleanupModel(dir, m, "", nil, func(p DownloadProgress) { last = p }); err != nil {
		t.Fatal(err)
	}
	if !CleanupModelReady(dir, m) || last.BytesDone != m.SizeBytes {
		t.Fatalf("not ready: %+v", last)
	}
}

func TestTheDefaultCleanupEndpointIsHuggingFace(t *testing.T) {
	t.Setenv("SAYSO_HF_ENDPOINT", "")
	if got := cleanupEndpoint(""); got != "https://huggingface.co" {
		t.Fatal(got)
	}
	if got := cleanupEndpoint("http://mirror"); got != "http://mirror" {
		t.Fatal(got)
	}
}
