package localengine

import "unsafe"

// cMem is a pointer to memory that a C library owns, from the uintptr that a
// native call returned. The Go collector never moves or frees that memory.
func cMem(p uintptr) unsafe.Pointer { return *(*unsafe.Pointer)(unsafe.Pointer(&p)) }
