package speech

import (
	"sync"
)

// Bus passes the events of the engine on to each watcher (bus.rs).
type Bus struct {
	mu       sync.Mutex
	watchers []chan EngineEvent
	stopEng  func()
	done     chan struct{}
	syncs    chan chan struct{}
	quit     chan struct{}
	stopOnce sync.Once
}

// StartBus passes the events of eng on until Stop.
func StartBus(eng Engine) *Bus {
	b := &Bus{quit: make(chan struct{})}
	if eng == nil {
		return b
	}
	events, stop := eng.Subscribe()
	b.stopEng = stop
	b.done = make(chan struct{})
	b.syncs = make(chan chan struct{})
	go b.fanOut(events)
	return b
}

func (b *Bus) passOn(e EngineEvent) {
	b.mu.Lock()
	defer b.mu.Unlock()
	for _, w := range b.watchers {
		select {
		case w <- e:
		default:
			// A watcher that does not read loses events rather than
			// holding the engine.
		}
	}
}

func (b *Bus) fanOut(events <-chan EngineEvent) {
	defer close(b.done)
	for {
		select {
		case e, ok := <-events:
			if !ok {
				return
			}
			b.passOn(e)
		case ack := <-b.syncs:
		drain:
			for {
				select {
				case e, ok := <-events:
					if !ok {
						close(ack)
						return
					}
					b.passOn(e)
				default:
					break drain
				}
			}
			close(ack)
		case <-b.quit:
			return
		}
	}
}

// watch gives the events from now on, until unwatch runs.
func (b *Bus) watch() (<-chan EngineEvent, func()) {
	ch := make(chan EngineEvent, 4096)
	b.mu.Lock()
	b.watchers = append(b.watchers, ch)
	b.mu.Unlock()
	return ch, func() {
		b.mu.Lock()
		defer b.mu.Unlock()
		for i, w := range b.watchers {
			if w == ch {
				b.watchers = append(b.watchers[:i], b.watchers[i+1:]...)
				return
			}
		}
	}
}

// sync passes on each event that the engine sent before this call.
func (b *Bus) sync() {
	if b.syncs == nil {
		return
	}
	ack := make(chan struct{})
	select {
	case b.syncs <- ack:
		<-ack
	case <-b.done:
	}
}

func (b *Bus) Stop() {
	b.stopOnce.Do(func() {
		close(b.quit)
		if b.done != nil {
			<-b.done
		}
		if b.stopEng != nil {
			b.stopEng()
		}
		b.mu.Lock()
		b.watchers = nil
		b.mu.Unlock()
	})
}
