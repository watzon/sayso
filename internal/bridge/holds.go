package bridge

import "sync"

// Work is what an app holds the engine for.
type Work int

const (
	WorkLiveSession Work = iota
	WorkJob
)

// Hold is an app that holds the engine.
type Hold struct {
	AppID string
	Name  string
	Work  Work
}

func holds(s *shared) []Hold {
	type found struct {
		appID string
		work  Work
	}
	var all []found
	if s.live != nil {
		if info, ok := s.live.Current(); ok {
			all = append(all, found{info.AppID, WorkLiveSession})
		}
	}
	if s.jobs != nil {
		owners := s.jobs.Connections()
		s.connMu.Lock()
		for _, c := range owners {
			if e, ok := s.connections[c]; ok && e.appID != "" {
				all = append(all, found{e.appID, WorkJob})
			}
		}
		s.connMu.Unlock()
	}
	apps := s.apps.Current()
	out := []Hold{}
	for _, f := range all {
		dup := false
		for _, h := range out {
			if h.AppID == f.appID && h.Work == f.work {
				dup = true
			}
		}
		if dup {
			continue
		}
		name := VisibleText(f.appID)
		if r := apps.Record(f.appID); r != nil {
			name = r.ShownName()
		}
		out = append(out, Hold{AppID: f.appID, Name: name, Work: f.work})
	}
	return out
}

type observers struct {
	mu    sync.Mutex
	live  func(*LiveInfo)
	holds func()
}

func (o *observers) setLive(f func(*LiveInfo)) { o.mu.Lock(); o.live = f; o.mu.Unlock() }
func (o *observers) setHolds(f func())         { o.mu.Lock(); o.holds = f; o.mu.Unlock() }

func (o *observers) holdsChanged() {
	o.mu.Lock()
	f := o.holds
	o.mu.Unlock()
	if f != nil {
		f()
	}
}

func (o *observers) liveChanged(info *LiveInfo) {
	o.mu.Lock()
	f := o.live
	o.mu.Unlock()
	if f != nil {
		f(info)
	}
	o.holdsChanged()
}
