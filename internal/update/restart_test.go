package update

import (
	"testing"
	"time"
)

func nextStatus(t *testing.T, statuses chan Status) Status {
	t.Helper()
	select {
	case s := <-statuses:
		return s
	case <-time.After(5 * time.Second):
		t.Fatal("no status")
	}
	return Status{}
}

// Restart swaps the prepared update in and says Sayso must quit; a busy or
// failed swap says why.
func TestRestartSwapsThePreparedUpdate(t *testing.T) {
	c, f, lock, k := setup(t)
	statuses := make(chan Status, 8)
	u := Spawn(Options{Context: *c, Installer: f, Lock: lock}, func(s Status) { statuses <- s })
	defer u.Stop()

	f.staged = true
	stage(f, k, "0.3.0")
	u.Restart()
	if s := nextStatus(t, statuses); s.Kind != StatusRestarting || s.Version.String() != "0.3.0" || f.exchanges != 1 || f.relaunches != 1 {
		t.Fatalf("%+v, exchanges %d, relaunches %d", s, f.exchanges, f.relaunches)
	}

	f.staged = true
	stage(f, k, "0.3.0")
	rel, _ := lock.TryExclusive()
	u.Restart()
	if s := nextStatus(t, statuses); s.Kind != StatusFailed || s.Failure.Kind != "busy" {
		t.Fatalf("busy: %+v", s)
	}
	rel()

	DeleteRecord(f.RecordPath())
	u.Restart()
	if s := nextStatus(t, statuses); s.Kind != StatusFailed || s.Failure.Kind != "nothing_staged" {
		t.Fatalf("nothing staged: %+v", s)
	}
}

func TestRestartWithoutAnInstallerFails(t *testing.T) {
	statuses := make(chan Status, 1)
	u := Spawn(Options{}, func(s Status) { statuses <- s })
	defer u.Stop()
	u.Restart()
	if s := nextStatus(t, statuses); s.Kind != StatusFailed || s.Failure.Kind != "cannot_update_itself" {
		t.Fatalf("%+v", s)
	}
}
