package device

import "os/exec"

func NewMic() Backend { return newLinuxAudio() }

// NewMonitor captures what the default output plays: the monitor of
// PulseAudio, or a sink capture of PipeWire.
func NewMonitor() Backend { return &linuxAudio{lookPath: exec.LookPath, monitor: true} }
