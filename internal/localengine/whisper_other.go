//go:build !windows

package localengine

import "fmt"

// whisper.cpp is the Windows final pass for whisper_cpp models; the Linux
// catalog does not list them.
func loadWhisperCpp(file string) (finalPass, error) {
	return nil, fmt.Errorf("%s needs whisper.cpp, and this engine runs it on Windows only", file)
}
