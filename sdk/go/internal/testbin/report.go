package testbin

import (
	"os"
	"testing"
)

// Passed writes the id of a scenario of sdk/conformance/scenarios.json to
// the file SAYSO_SCENARIO_REPORT names, when the test ends without a
// failure or a skip.
func Passed(t testing.TB, id string) {
	t.Helper()
	report := os.Getenv("SAYSO_SCENARIO_REPORT")
	if report == "" {
		return
	}
	t.Cleanup(func() {
		if t.Failed() || t.Skipped() {
			return
		}
		f, err := os.OpenFile(report, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
		if err != nil {
			t.Errorf("scenario report: %v", err)
			return
		}
		defer f.Close()
		if _, err := f.WriteString(id + "\n"); err != nil {
			t.Errorf("scenario report: %v", err)
		}
	})
}
