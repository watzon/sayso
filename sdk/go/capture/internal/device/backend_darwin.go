package device

func NewMic() Backend { return &macAudio{} }
