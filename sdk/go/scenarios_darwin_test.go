package sayso_test

import (
	"errors"
	"testing"

	sayso "github.com/watzon/sayso/sdk/go"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
)

func TestLocalModeOnMacOSHasNoSystemAudio(t *testing.T) {
	testbin.Passed(t, "AU01")
	s := connect(t, sayso.Options{Mode: sayso.Local})
	if f := s.Features(); f.SystemCapture {
		t.Fatalf("%+v", f)
	}
	if err := s.DownloadModel(ctx(t), "fake-stream", "tests", nil); err != nil {
		t.Fatal(err)
	}
	if _, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{sayso.SourceSystem}, Model: "fake-stream"}); !errors.Is(err, sayso.ErrUnsupported) {
		t.Fatal(err)
	}
}
