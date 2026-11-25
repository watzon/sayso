package ui

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"image/color"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
)

// progressBar is the box of the progress element with an accessible name.
func progressBar(t *testing.T, tt *mu.Tester, name string) mu.Rect {
	t.Helper()
	for _, a := range tt.AccessAll() {
		if a.Role == mu.RoleProgress && a.Name == name {
			return a.Bounds
		}
	}
	t.Fatalf("no progress named %q; texts %q", name, tt.Texts())
	return mu.Rect{}
}

// hasExactText is true when some text is exactly s.
func hasExactText(tt *mu.Tester, s string) bool {
	for _, x := range tt.Texts() {
		if x == s {
			return true
		}
	}
	return false
}

// otherModel is a local model that is not the final model.
func otherModel(t *testing.T, e *testEnv) models.ModelInfo {
	t.Helper()
	for _, m := range e.a.modelList(0) {
		if m.ID != e.a.FinalModel() {
			return m
		}
	}
	t.Fatal("one model only")
	return models.ModelInfo{}
}

// pixelAt is the color of the screenshot at a point in DIPs.
func pixelAt(tt *mu.Tester, x, y float32) color.RGBA {
	img := tt.Image()
	k := float32(img.Bounds().Dx()) / float32(HubWidth)
	return img.RGBAAt(int(x*k), int(y*k))
}

func TestTheModelsPageShowsADownloadsProgress(t *testing.T) {
	for _, inUse := range []bool{false, true} {
		for _, dark := range []bool{false, true} {
			t.Run(fmt.Sprintf("in use %v, dark %v", inUse, dark), func(t *testing.T) {
				e := newEnv(t)
				e.a.SetPage(PageModels)
				m := otherModel(t, e)
				if inUse {
					e.a.cfg.Dictation.Model = m.ID
				}
				if got := e.a.FinalModel() == m.ID; got != inUse {
					t.Fatalf("in use %v", got)
				}
				e.eng.setState(string(m.ID), engine.ModelStatus{State: engine.StateDownloading, Fraction: 0.42, BytesDone: 42e6, BytesTotal: 100e6})
				tt := e.hub(t)
				tt.SetSize(HubWidth, 4000)
				tt.SetDark(dark)
				tt.Frame()
				if !hasExactText(tt, "42%") {
					t.Fatalf("no 42%%: %q", tt.Texts())
				}
				b := progressBar(t, tt, i18n.T("models.a11y.downloading", "name", m.Name))
				if b.W < 100 || b.H < 3 {
					t.Fatalf("the bar is too small to see: %+v", b)
				}
				y := b.Y + b.H/2
				if filled, empty := pixelAt(tt, b.X+b.W*0.2, y), pixelAt(tt, b.X+b.W*0.8, y); filled == empty {
					t.Fatalf("the filled part looks like the empty part: %v", filled)
				}
			})
		}
	}
}

func TestADownloadOfUnknownSizeShowsTheBytesSoFar(t *testing.T) {
	e := newEnv(t)
	e.a.SetPage(PageModels)
	m := otherModel(t, e)
	e.eng.setState(string(m.ID), engine.ModelStatus{State: engine.StateDownloading, BytesDone: 3_456_789})
	tt := e.hub(t)
	tt.SetSize(HubWidth, 4000)
	tt.Frame()
	if !hasExactText(tt, models.FormatSize(3_456_789)) || hasExactText(tt, "0%") {
		t.Fatalf("want the bytes so far, not 0%%: %q", tt.Texts())
	}
	progressBar(t, tt, i18n.T("models.a11y.downloading", "name", m.Name))
}

// cleanupServer serves a small model file: half of it in pieces, slower
// than progress is reported, then the rest once release closes. Anything
// else is a license file.
func cleanupServer(t *testing.T, content []byte, release <-chan struct{}, hits *atomic.Int32) *httptest.Server {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, ".gguf") {
			w.Write([]byte("MIT"))
			return
		}
		hits.Add(1)
		w.Header().Set("Content-Length", fmt.Sprint(len(content)))
		half := len(content) / 2
		for i := range 4 {
			w.Write(content[i*half/4 : (i+1)*half/4])
			w.(http.Flusher).Flush()
			time.Sleep(200 * time.Millisecond)
		}
		<-release
		w.Write(content[len(content)/2:])
	}))
	t.Cleanup(srv.Close)
	return srv
}

// postQueue makes a.post run on the test goroutine, in runUntil.
type postQueue chan func()

func (q postQueue) runUntil(t *testing.T, done func() bool) {
	t.Helper()
	deadline := time.After(10 * time.Second)
	for !done() {
		select {
		case fn := <-q:
			fn()
		case <-deadline:
			t.Fatal("timed out")
		}
	}
}

func TestACleanupModelDownloadShowsItsProgressInProviders(t *testing.T) {
	e := newEnv(t)
	posts := make(postQueue, 256)
	e.a.d.Post = func(fn func()) { posts <- fn }
	content := []byte(strings.Repeat("gguf", 4096))
	release := make(chan struct{})
	var hits atomic.Int32
	t.Setenv("SAYSO_HF_ENDPOINT", cleanupServer(t, content, release, &hits).URL)
	m := enhance.CleanupCatalog()[0]
	sum := sha256.Sum256(content)
	m.SizeBytes, m.SHA256 = uint64(len(content)), hex.EncodeToString(sum[:])

	e.a.OpenSettings(SecProviders)
	tt := e.hub(t)
	e.a.downloadCleanup(m)
	posts.runUntil(t, func() bool { return e.a.set.cleanupDl[m.ID].progress.BytesDone > 0 })
	tt.Frame()
	got := e.a.set.cleanupDl[m.ID].progress
	if got.BytesTotal != m.SizeBytes {
		t.Fatalf("progress %+v", got)
	}
	want := i18n.T("settings.providers.cleanup_downloading", "percent", int(got.BytesDone*100/got.BytesTotal))
	if !tt.HasText(want) {
		t.Fatalf("no %q: %q", want, tt.Texts())
	}
	progressBar(t, tt, i18n.T("models.a11y.downloading", "name", m.Name))
	e.a.downloadCleanup(m)
	close(release)
	posts.runUntil(t, func() bool { _, on := e.a.set.cleanupDl[m.ID]; return !on })
	if hits.Load() != 1 {
		t.Fatalf("a second start downloaded again: %d requests", hits.Load())
	}
}

func TestAFailedCleanupDownloadSaysSoAndOffersItAgain(t *testing.T) {
	e := newEnv(t)
	posts := make(postQueue, 256)
	e.a.d.Post = func(fn func()) { posts <- fn }
	srv := httptest.NewServer(http.NotFoundHandler())
	t.Cleanup(srv.Close)
	t.Setenv("SAYSO_HF_ENDPOINT", srv.URL)
	m := enhance.CleanupCatalog()[0]
	e.a.OpenSettings(SecProviders)
	tt := e.hub(t)
	e.a.downloadCleanup(m)
	posts.runUntil(t, func() bool { return e.a.set.cleanupDl[m.ID].failed })
	tt.Frame()
	if !tt.HasText(i18n.T("settings.providers.cleanup_download_failed")) {
		t.Fatalf("no failure: %q", tt.Texts())
	}
	if _, ok := controlBounds(tt, i18n.T("settings.providers.download")); !ok {
		t.Fatal("no Download button to try again")
	}
}

// unknownSize is a download whose size the server did not send.
var unknownSize = engine.ModelStatus{State: engine.StateDownloading, BytesDone: 3_456_789}

func TestHomeShowsTheModelDownloadNextToOpenModels(t *testing.T) {
	for _, c := range []struct {
		name string
		st   engine.ModelStatus
		want string
	}{
		{"known size", engine.ModelStatus{State: engine.StateDownloading, Fraction: 0.42, BytesDone: 42e6, BytesTotal: 100e6}, "Downloading 42%"},
		{"unknown size", unknownSize, "Downloading " + models.FormatSize(3_456_789)},
	} {
		t.Run(c.name, func(t *testing.T) {
			e := newEnv(t)
			e.a.SetPage(PageHome)
			e.eng.setState(string(e.a.FinalModel()), c.st)
			tt := e.hub(t)
			tt.Frame()
			if !hasExactText(tt, c.want) || hasExactText(tt, "Downloading 0%") {
				t.Fatalf("want %q: %q", c.want, tt.Texts())
			}
			press(t, tt, i18n.T("home.checklist.open_models"))
			tt.Frame()
			if e.a.page != PageModels {
				t.Fatalf("Open Models went to page %v", e.a.page)
			}
		})
	}
}

func TestOnboardingShowsTheBytesOfADownloadOfUnknownSize(t *testing.T) {
	amount := models.FormatSize(3_456_789)
	noZero := func(t *testing.T, tt *mu.Tester) {
		t.Helper()
		for _, x := range tt.Texts() {
			if strings.Contains(x, "0%") {
				t.Fatalf("a false 0%%: %q", tt.Texts())
			}
		}
	}

	t.Run("model step footer", func(t *testing.T) {
		e := newEnv(t)
		e.eng.setState(string(e.a.FinalModel()), unknownSize)
		e.a.goToStep(stepModel)
		tt := onboardingTester(e)
		tt.Frame()
		if want := "Downloading " + amount; footerLabel(e) != want || !tt.HasText(want) {
			t.Fatalf("footer %q, want %q: %q", footerLabel(e), want, tt.Texts())
		}
		noZero(t, tt)
	})

	t.Run("permissions step", func(t *testing.T) {
		e := newEnv(t)
		e.eng.setState(string(e.a.FinalModel()), unknownSize)
		e.a.goToStep(stepPermissions)
		tt := onboardingTester(e)
		tt.Frame()
		if !hasExactText(tt, amount) {
			t.Fatalf("no %q: %q", amount, tt.Texts())
		}
		noZero(t, tt)
	})

	t.Run("practice step", func(t *testing.T) {
		e := newEnv(t)
		e.a.cfg.Dictation.LivePreview = true
		e.a.cfg.Dictation.PreviewModel.Model = otherModel(t, e).ID
		pid, _ := e.a.previewModel()
		e.eng.setState(string(e.a.FinalModel()), unknownSize)
		e.eng.setState(string(pid), unknownSize)
		e.a.goToStep(stepPractice)
		tt := onboardingTester(e)
		tt.Frame()
		for _, want := range []string{"Final pass downloading · " + amount, "Live preview downloading · " + amount} {
			if !tt.HasText(want) {
				t.Fatalf("no %q: %q", want, tt.Texts())
			}
		}
		noZero(t, tt)
	})

	t.Run("status text", func(t *testing.T) {
		e := newEnv(t)
		id := e.a.FinalModel()
		e.eng.setState(string(id), unknownSize)
		if got, want := e.a.statusText(id), e.a.modelName(id)+" · "+amount; got != want {
			t.Fatalf("%q, want %q", got, want)
		}
	})
}
