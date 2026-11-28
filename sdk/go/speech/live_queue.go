package speech

import "sync"

// liveQueue is an unbounded queue with one reader. A push never waits, so a
// capture or the intake of a source never waits for the reader (the
// unbounded crossbeam channels of sayso-speech/src/live).
type liveQueue[T any] struct {
	mu     sync.Mutex
	items  []T
	closed bool
	signal chan struct{}
}

func newLiveQueue[T any]() *liveQueue[T] {
	return &liveQueue[T]{signal: make(chan struct{}, 1)}
}

func (q *liveQueue[T]) notify() {
	select {
	case q.signal <- struct{}{}:
	default:
	}
}

// push adds v. It is false after close.
func (q *liveQueue[T]) push(v T) bool {
	q.mu.Lock()
	if q.closed {
		q.mu.Unlock()
		return false
	}
	q.items = append(q.items, v)
	q.mu.Unlock()
	q.notify()
	return true
}

// close ends the queue: the reader gets what is in it, then the end.
func (q *liveQueue[T]) close() {
	q.mu.Lock()
	q.closed = true
	q.mu.Unlock()
	q.notify()
}

// tryPop gives the first item. done is true when the queue is closed and
// empty.
func (q *liveQueue[T]) tryPop() (v T, ok, done bool) {
	q.mu.Lock()
	defer q.mu.Unlock()
	if len(q.items) > 0 {
		v = q.items[0]
		var zero T
		q.items[0] = zero
		q.items = q.items[1:]
		return v, true, false
	}
	return v, false, q.closed
}

// pop waits for the first item. It is false at the end.
func (q *liveQueue[T]) pop() (T, bool) {
	for {
		v, ok, done := q.tryPop()
		if ok {
			return v, true
		}
		if done {
			return v, false
		}
		<-q.signal
	}
}
