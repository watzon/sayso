package device

// The C and Objective-C calls of the macOS services, loaded at run time with
// purego, so the build needs no cgo (CGO_ENABLED=0).

import (
	"fmt"
	"runtime"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
	"github.com/ebitengine/purego/objc"
)

const (
	pathCoreFoundation = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"
	pathAudioToolbox   = "/System/Library/Frameworks/AudioToolbox.framework/AudioToolbox"
	pathCoreAudio      = "/System/Library/Frameworks/CoreAudio.framework/CoreAudio"
	pathFoundation     = "/System/Library/Frameworks/Foundation.framework/Foundation"
	pathAVFoundation   = "/System/Library/Frameworks/AVFoundation.framework/AVFoundation"
	pathLibObjC        = "/usr/lib/libobjc.A.dylib"
)

// CoreFoundation.
var (
	cfRelease                     func(cf uintptr)
	cfStringCreateWithCString     func(alloc uintptr, s string, encoding uint32) uintptr
	cfStringGetLength             func(s uintptr) int64
	cfStringGetMaximumSizeForEnc  func(length int64, encoding uint32) int64
	cfStringGetCString            func(s uintptr, buf *byte, size int64, encoding uint32) bool
	cfGetTypeID                   func(cf uintptr) uint64
	cfStringGetTypeID             func() uint64
	cfDictionaryCreate            func(alloc uintptr, keys *uintptr, values *uintptr, n int64, keyCB uintptr, valueCB uintptr) uintptr
	cfRunLoopGetCurrent           func() uintptr
	cfRunLoopRun                  func()
	cfRunLoopStop                 func(rl uintptr)
	cfRunLoopAddSource            func(rl uintptr, src uintptr, mode uintptr)
	cfMachPortCreateRunLoopSource func(alloc uintptr, port uintptr, order int64) uintptr
	cfMachPortInvalidate          func(port uintptr)

	kCFBooleanTrue                uintptr
	kCFRunLoopCommonModes         uintptr
	kCFTypeDictionaryKeyCallBacks uintptr
	kCFTypeDictionaryValueCBs     uintptr
)

const kCFStringEncodingUTF8 = 0x08000100

// AudioToolbox and CoreAudio.
var (
	audioQueueNewInput             func(format *audioStreamBasicDescription, cb uintptr, user uintptr, runLoop uintptr, mode uintptr, flags uint32, out *uintptr) int32
	audioQueueAllocateBuffer       func(q uintptr, size uint32, out *uintptr) int32
	audioQueueEnqueueBuffer        func(q uintptr, buf uintptr, n uint32, desc uintptr) int32
	audioQueueStart                func(q uintptr, start uintptr) int32
	audioQueueStop                 func(q uintptr, immediate bool) int32
	audioQueueDispose              func(q uintptr, immediate bool) int32
	audioQueueSetProperty          func(q uintptr, id uint32, data uintptr, size uint32) int32
	audioObjectGetPropertyDataSize func(obj uint32, addr *audioObjectPropertyAddress, qualSize uint32, qual uintptr, out *uint32) int32
	audioObjectGetPropertyData     func(obj uint32, addr *audioObjectPropertyAddress, qualSize uint32, qual uintptr, size *uint32, out unsafe.Pointer) int32
)

// Objective-C runtime helpers.
var (
	autoreleasePoolPush func() uintptr
	autoreleasePoolPop  func(pool uintptr)
)

// Values of AVFoundation and AppKit globals.
var (
	avMediaTypeAudio objc.ID
)

var (
	darwinOnce sync.Once
	darwinErr  error
)

// loadDarwin opens the frameworks and binds every call once.
func loadDarwin() error {
	darwinOnce.Do(func() {
		defer func() {
			// RegisterLibFunc panics on a missing symbol.
			if r := recover(); r != nil {
				darwinErr = fmt.Errorf("macOS frameworks: %v", r)
			}
		}()
		darwinErr = bindDarwin()
	})
	return darwinErr
}

func openLib(path string) uintptr {
	h, err := purego.Dlopen(path, purego.RTLD_NOW|purego.RTLD_GLOBAL)
	if err != nil {
		panic(fmt.Sprintf("open %s: %v", path, err))
	}
	return h
}

func symbol(lib uintptr, name string) uintptr {
	p, err := purego.Dlsym(lib, name)
	if err != nil {
		panic(fmt.Sprintf("symbol %s: %v", name, err))
	}
	return p
}

// global reads the value of a C global that holds a pointer.
func global(lib uintptr, name string) uintptr {
	p := symbol(lib, name)
	return **(**uintptr)(unsafe.Pointer(&p))
}

func bindDarwin() error {
	cf := openLib(pathCoreFoundation)
	purego.RegisterLibFunc(&cfRelease, cf, "CFRelease")
	purego.RegisterLibFunc(&cfStringCreateWithCString, cf, "CFStringCreateWithCString")
	purego.RegisterLibFunc(&cfStringGetLength, cf, "CFStringGetLength")
	purego.RegisterLibFunc(&cfStringGetMaximumSizeForEnc, cf, "CFStringGetMaximumSizeForEncoding")
	purego.RegisterLibFunc(&cfStringGetCString, cf, "CFStringGetCString")
	purego.RegisterLibFunc(&cfGetTypeID, cf, "CFGetTypeID")
	purego.RegisterLibFunc(&cfStringGetTypeID, cf, "CFStringGetTypeID")
	purego.RegisterLibFunc(&cfDictionaryCreate, cf, "CFDictionaryCreate")
	purego.RegisterLibFunc(&cfRunLoopGetCurrent, cf, "CFRunLoopGetCurrent")
	purego.RegisterLibFunc(&cfRunLoopRun, cf, "CFRunLoopRun")
	purego.RegisterLibFunc(&cfRunLoopStop, cf, "CFRunLoopStop")
	purego.RegisterLibFunc(&cfRunLoopAddSource, cf, "CFRunLoopAddSource")
	purego.RegisterLibFunc(&cfMachPortCreateRunLoopSource, cf, "CFMachPortCreateRunLoopSource")
	purego.RegisterLibFunc(&cfMachPortInvalidate, cf, "CFMachPortInvalidate")
	kCFBooleanTrue = global(cf, "kCFBooleanTrue")
	kCFRunLoopCommonModes = global(cf, "kCFRunLoopCommonModes")
	// These two are structs: the call takes their address.
	kCFTypeDictionaryKeyCallBacks = symbol(cf, "kCFTypeDictionaryKeyCallBacks")
	kCFTypeDictionaryValueCBs = symbol(cf, "kCFTypeDictionaryValueCallBacks")

	at := openLib(pathAudioToolbox)
	purego.RegisterLibFunc(&audioQueueNewInput, at, "AudioQueueNewInput")
	purego.RegisterLibFunc(&audioQueueAllocateBuffer, at, "AudioQueueAllocateBuffer")
	purego.RegisterLibFunc(&audioQueueEnqueueBuffer, at, "AudioQueueEnqueueBuffer")
	purego.RegisterLibFunc(&audioQueueStart, at, "AudioQueueStart")
	purego.RegisterLibFunc(&audioQueueStop, at, "AudioQueueStop")
	purego.RegisterLibFunc(&audioQueueDispose, at, "AudioQueueDispose")
	purego.RegisterLibFunc(&audioQueueSetProperty, at, "AudioQueueSetProperty")

	ca := openLib(pathCoreAudio)
	purego.RegisterLibFunc(&audioObjectGetPropertyDataSize, ca, "AudioObjectGetPropertyDataSize")
	purego.RegisterLibFunc(&audioObjectGetPropertyData, ca, "AudioObjectGetPropertyData")

	lib := openLib(pathLibObjC)
	purego.RegisterLibFunc(&autoreleasePoolPush, lib, "objc_autoreleasePoolPush")
	purego.RegisterLibFunc(&autoreleasePoolPop, lib, "objc_autoreleasePoolPop")

	// Loading the frameworks registers their Objective-C classes.
	openLib(pathFoundation)
	av := openLib(pathAVFoundation)
	avMediaTypeAudio = objc.ID(global(av, "AVMediaTypeAudio"))
	return nil
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

var selCache sync.Map

// sel returns the cached selector for name.
func sel(name string) objc.SEL {
	if s, ok := selCache.Load(name); ok {
		return s.(objc.SEL)
	}
	s := objc.RegisterName(name)
	selCache.Store(name, s)
	return s
}

func class(name string) objc.ID { return objc.ID(objc.GetClass(name)) }

// withPool runs fn inside an autorelease pool. Code off the main thread has
// no pool, so autoreleased objects would leak without one. Pools belong to a
// thread, so the goroutine stays on its thread until the pop (the lock nests).
func withPool(fn func()) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	pool := autoreleasePoolPush()
	defer autoreleasePoolPop(pool)
	fn()
}

// cfString makes a retained CFString (an NSString too). Release it with cfRelease.
func cfString(s string) uintptr {
	return cfStringCreateWithCString(0, s, kCFStringEncodingUTF8)
}

// goStringFromCF copies a CFString or NSString into a Go string.
func goStringFromCF(s uintptr) string {
	if s == 0 {
		return ""
	}
	n := cfStringGetMaximumSizeForEnc(cfStringGetLength(s), kCFStringEncodingUTF8) + 1
	buf := make([]byte, n)
	if !cfStringGetCString(s, &buf[0], n, kCFStringEncodingUTF8) {
		return ""
	}
	for i, b := range buf {
		if b == 0 {
			return string(buf[:i])
		}
	}
	return string(buf)
}

// nsString makes an autoreleased NSString. Call it inside withPool.
func nsString(s string) objc.ID {
	b := append([]byte(s), 0)
	id := class("NSString").Send(sel("stringWithUTF8String:"), unsafe.Pointer(&b[0]))
	runtime.KeepAlive(b)
	return id
}

// nsData makes an autoreleased NSData with a copy of b.
func nsData(b []byte) objc.ID {
	if len(b) == 0 {
		return class("NSData").Send(sel("data"))
	}
	id := class("NSData").Send(sel("dataWithBytes:length:"), unsafe.Pointer(&b[0]), uint64(len(b)))
	runtime.KeepAlive(b)
	return id
}

// goBytes copies the bytes of an NSData.
func goBytes(data objc.ID) []byte {
	if data == 0 {
		return nil
	}
	n := objc.Send[uint64](data, sel("length"))
	if n == 0 {
		return []byte{}
	}
	p := objc.Send[unsafe.Pointer](data, sel("bytes"))
	return append([]byte(nil), unsafe.Slice((*byte)(p), n)...)
}

// cfBoolDict makes a retained {key: kCFBooleanTrue} dictionary.
func cfBoolDict(key string) uintptr {
	k := cfString(key)
	defer cfRelease(k)
	v := kCFBooleanTrue
	return cfDictionaryCreate(0, &k, &v, 1, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCBs)
}
