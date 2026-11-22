package platform

// The MediaRemote calls for macOS before 15.4, which has no
// MRNowPlayingRequest for media_darwin.js but answers any process. Sayso
// calls the framework itself with purego. When the framework or a function
// is missing, media pauses nothing.

import (
	"fmt"
	"log/slog"
	"strconv"
	"time"

	"github.com/ebitengine/purego"
	"github.com/ebitengine/purego/objc"
)

const (
	pathMediaRemote = "/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote"
	// directMediaWait stops a read whose callback never comes.
	directMediaWait = 2 * time.Second
)

type directMedia struct {
	isPlaying   func(queue uintptr, block objc.Block)
	playerPID   func(queue uintptr, block objc.Block)
	sendCommand func(command int32, options uintptr) bool
	queue       uintptr
}

func newDirectMedia() mediaBackend {
	d, err := loadDirectMedia()
	if err != nil {
		slog.Warn("media: MediaRemote is not usable, media will not pause", "err", err)
		return noMedia{}
	}
	return d
}

func loadDirectMedia() (d *directMedia, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("%v", r)
		}
	}()
	if _, err := purego.Dlopen(pathLibObjC, purego.RTLD_NOW|purego.RTLD_GLOBAL); err != nil {
		return nil, err
	}
	lib, err := purego.Dlopen(pathMediaRemote, purego.RTLD_NOW|purego.RTLD_GLOBAL)
	if err != nil {
		return nil, err
	}
	d = &directMedia{}
	purego.RegisterLibFunc(&d.isPlaying, lib, "MRMediaRemoteGetNowPlayingApplicationIsPlaying")
	purego.RegisterLibFunc(&d.playerPID, lib, "MRMediaRemoteGetNowPlayingApplicationPID")
	purego.RegisterLibFunc(&d.sendCommand, lib, "MRMediaRemoteSendCommand")
	system := openLib("/usr/lib/libSystem.B.dylib")
	var globalQueue func(identifier int64, flags uint64) uintptr
	purego.RegisterLibFunc(&globalQueue, system, "dispatch_get_global_queue")
	d.queue = globalQueue(0, 0)
	return d, nil
}

// directAsk calls get with a block and waits for its answer. A block whose answer
// comes too late stays allocated, so its late call is safe.
func directAsk[T any](get func(uintptr, objc.Block), queue uintptr) (T, bool) {
	answer := make(chan T, 1)
	block := objc.NewBlock(func(_ objc.Block, v T) { answer <- v })
	get(queue, block)
	select {
	case v := <-answer:
		block.Release()
		return v, true
	case <-time.After(directMediaWait):
		var zero T
		return zero, false
	}
}

func (d *directMedia) nowPlaying() (*nowPlaying, error) {
	pid, ok := directAsk[int32](d.playerPID, d.queue)
	if !ok || pid <= 0 {
		return nil, nil
	}
	playing, ok := directAsk[bool](d.isPlaying, d.queue)
	if !ok {
		return nil, nil
	}
	return &nowPlaying{player: strconv.Itoa(int(pid)), playing: playing}, nil
}

func (d *directMedia) send(command int) error {
	if !d.sendCommand(int32(command), 0) {
		return fmt.Errorf("media: MediaRemote refused command %d", command)
	}
	return nil
}

// noMedia sees no now-playing app, so media never pauses.
type noMedia struct{}

func (noMedia) nowPlaying() (*nowPlaying, error) { return nil, nil }
func (noMedia) send(int) error                   { return nil }
