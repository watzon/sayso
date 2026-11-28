package audio

import (
	"errors"
	"fmt"
	"runtime"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

// AAC, M4A, and the other formats of Core Audio, with ExtAudioFile. It
// converts to 16 kHz mono float32 itself.

const systemFormats = ", and AAC and M4A"

type extASBD struct {
	SampleRate       float64
	FormatID         uint32
	FormatFlags      uint32
	BytesPerPacket   uint32
	FramesPerPacket  uint32
	BytesPerFrame    uint32
	ChannelsPerFrame uint32
	BitsPerChannel   uint32
	Reserved         uint32
}

// extBufferList is an AudioBufferList with one buffer; the buffer is at
// offset 8 because it holds a pointer.
type extBufferList struct {
	count    uint32
	_        uint32
	channels uint32
	size     uint32
	data     unsafe.Pointer
}

const (
	kExtAudioFilePropertyClientDataFormat = 0x63666d74 // 'cfmt'
	kAudioFormatLinearPCMID               = 0x6C70636D // 'lpcm'
	extFloatPacked                        = 1 | 8
)

var (
	extOnce sync.Once
	extErr  error

	cfURLCreateFromFileSystemRepresentation func(alloc uintptr, path *byte, n int64, isDir bool) uintptr
	cfReleaseURL                            func(uintptr)
	extAudioFileOpenURL                     func(url uintptr, out *uintptr) int32
	extAudioFileSetProperty                 func(ref uintptr, id uint32, size uint32, data unsafe.Pointer) int32
	extAudioFileRead                        func(ref uintptr, frames *uint32, list unsafe.Pointer) int32
	extAudioFileDispose                     func(ref uintptr) int32
)

func loadExt() error {
	extOnce.Do(func() {
		defer func() {
			if r := recover(); r != nil {
				extErr = fmt.Errorf("AudioToolbox: %v", r)
			}
		}()
		cf, err := purego.Dlopen("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			extErr = err
			return
		}
		at, err := purego.Dlopen("/System/Library/Frameworks/AudioToolbox.framework/AudioToolbox", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			extErr = err
			return
		}
		purego.RegisterLibFunc(&cfURLCreateFromFileSystemRepresentation, cf, "CFURLCreateFromFileSystemRepresentation")
		purego.RegisterLibFunc(&cfReleaseURL, cf, "CFRelease")
		purego.RegisterLibFunc(&extAudioFileOpenURL, at, "ExtAudioFileOpenURL")
		purego.RegisterLibFunc(&extAudioFileSetProperty, at, "ExtAudioFileSetProperty")
		purego.RegisterLibFunc(&extAudioFileRead, at, "ExtAudioFileRead")
		purego.RegisterLibFunc(&extAudioFileDispose, at, "ExtAudioFileDispose")
	})
	return extErr
}

func readSystem(path string) ([]float32, error) {
	if err := loadExt(); err != nil {
		return nil, errors.Join(ErrUnsupported, err)
	}
	b := append([]byte(path), 0)
	url := cfURLCreateFromFileSystemRepresentation(0, &b[0], int64(len(path)), false)
	if url == 0 {
		return nil, fmt.Errorf("the path %s is not a file URL", path)
	}
	defer cfReleaseURL(url)
	var ref uintptr
	if st := extAudioFileOpenURL(url, &ref); st != 0 {
		return nil, fmt.Errorf("ExtAudioFileOpenURL: OSStatus %d", st)
	}
	defer extAudioFileDispose(ref)
	format := extASBD{SampleRate: SampleRate, FormatID: kAudioFormatLinearPCMID, FormatFlags: extFloatPacked, BytesPerPacket: 4, FramesPerPacket: 1, BytesPerFrame: 4, ChannelsPerFrame: 1, BitsPerChannel: 32}
	if st := extAudioFileSetProperty(ref, kExtAudioFilePropertyClientDataFormat, uint32(unsafe.Sizeof(format)), unsafe.Pointer(&format)); st != 0 {
		return nil, fmt.Errorf("ExtAudioFileSetProperty: OSStatus %d", st)
	}
	const frames = 16384
	buf := make([]float32, frames)
	var pin runtime.Pinner
	pin.Pin(&buf[0])
	defer pin.Unpin()
	list := &extBufferList{count: 1, channels: 1}
	pin.Pin(list)
	var out []float32
	for {
		n := uint32(frames)
		list.size, list.data = frames*4, unsafe.Pointer(&buf[0])
		if st := extAudioFileRead(ref, &n, unsafe.Pointer(list)); st != 0 {
			return nil, fmt.Errorf("ExtAudioFileRead: OSStatus %d", st)
		}
		if n == 0 {
			return out, nil
		}
		out = append(out, buf[:n]...)
	}
}
