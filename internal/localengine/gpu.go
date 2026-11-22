package localengine

// gpuInfo is one GPU in ggml's device list.
type gpuInfo struct {
	dedicated bool
	vram      uint64
}

// bestGPU is the index of the best GPU: a dedicated one before an integrated
// one, then the most memory, the first on a tie (transcribe-rs
// auto_select_gpu_device). ok is false for an empty list.
func bestGPU(gpus []gpuInfo) (index int32, ok bool) {
	if len(gpus) == 0 {
		return 0, false
	}
	best := 0
	for i, g := range gpus[1:] {
		b := gpus[best]
		if g.dedicated && !b.dedicated || g.dedicated == b.dedicated && g.vram > b.vram {
			best = i + 1
		}
	}
	return int32(best), true
}

// firstGPUIsDedicated: parakeet.cpp picks the first GPU or IGPU in ggml's list.
// It must not be an integrated GPU (Mesa can list a laptop's integrated GPU
// first; the app's GGML_VK_VISIBLE_DEVICES leaves only the discrete one).
// types holds the ggml device type of each device.
func firstGPUIsDedicated(types []int32) bool {
	for _, t := range types {
		if t == ggmlDeviceGPU || t == ggmlDeviceIGPU {
			return t == ggmlDeviceGPU
		}
	}
	return false
}

// `enum ggml_backend_dev_type`.
const (
	ggmlDeviceGPU  = 1
	ggmlDeviceIGPU = 2
)
