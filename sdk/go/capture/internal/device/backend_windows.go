package device

func NewMic() Backend { return &windowsAudio{} }

// NewLoopback captures what the default output of Windows plays.
func NewLoopback() Backend { return &windowsAudio{loopback: true} }
