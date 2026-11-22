//go:build !linux

package localengine

import "fmt"

// parakeet.cpp runs on Linux only (Vulkan GPUs); no other catalog lists a GGUF model.
func loadParakeet(file string) (finalPass, error) {
	return nil, fmt.Errorf("%s needs parakeet.cpp, and this engine runs it on Linux only", file)
}
