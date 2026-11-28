package cleanup_test

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/cleanup"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
)

func TestALongTextGoesToTheRuntimeInPartsAndComesBackAsOneText(t *testing.T) {
	testbin.Passed(t, "C04")
	c := fakeRuntime(t, "")
	dir, _ := readyModel(t, "spokenly-cleanup-1")
	cl := &cleanup.Cleaner{Runtime: c, ModelsDir: dir}
	text := strings.Repeat("um so we met on monday and we talked about the plan for the release. ", 160)
	resp, err := cl.Clean(context.Background(), cleanup.Request{Model: "spokenly-cleanup-1", Text: text, Timeout: 60 * time.Second})
	if err != nil {
		t.Fatal(err)
	}
	if resp.Parts < 2 || resp.Model != "spokenly-cleanup-1" || resp.TokensIn == 0 || resp.TokensOut == 0 {
		t.Fatalf("parts %d, model %s, tokens %d/%d", resp.Parts, resp.Model, resp.TokensIn, resp.TokensOut)
	}
	if strings.Contains(resp.Text, "um so") || !strings.Contains(resp.Text, "we met on monday") {
		t.Fatal(resp.Text[:min(200, len(resp.Text))])
	}
	short, err := cl.Clean(context.Background(), cleanup.Request{Model: "spokenly-cleanup-1", Text: "um hello there", Timeout: 60 * time.Second})
	if err != nil || short.Parts != 1 || short.LoadMs != 0 || strings.TrimSpace(short.Text) != "Hello there." {
		t.Fatalf("%+v %v", short, err)
	}
}
