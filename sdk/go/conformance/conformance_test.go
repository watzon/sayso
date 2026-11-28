package conformance_test

import (
	"context"
	"os"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/conformance"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
)

func TestMain(m *testing.M) {
	code := m.Run()
	testbin.Cleanup()
	os.Exit(code)
}

func check(t *testing.T, model string, env ...string) conformance.Report {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	report, err := conformance.Check(ctx, conformance.Options{
		Program: testbin.Path(t, testbin.FakeEngine),
		Env:     env,
		Levels:  []conformance.Level{conformance.Core, conformance.Model, conformance.Store},
		Model:   model,
	})
	if err != nil {
		t.Fatal(err)
	}
	return report
}

func TestTheFakeEnginePassesEveryLevel(t *testing.T) {
	for _, model := range []string{"fake-final", "fake-stream"} {
		t.Run(model, func(t *testing.T) {
			report := check(t, model)
			if failed := report.Failed(); len(failed) > 0 {
				t.Fatalf("failed: %+v", failed)
			}
			ran := 0
			for _, r := range report.Results {
				if !r.Skip {
					ran++
				}
			}
			if ran < 5 {
				t.Fatalf("only %d checks ran: %+v", ran, report.Results)
			}
		})
	}
}

func TestAnEngineThatCannotLoadFails(t *testing.T) {
	report := check(t, "fake-final", `SAYSO_FAKE_SCRIPT={"fail":{"load":{"message":"no","reason":"load_failed"}}}`)
	if len(report.Failed()) == 0 {
		t.Fatalf("no check failed: %+v", report.Results)
	}
}
