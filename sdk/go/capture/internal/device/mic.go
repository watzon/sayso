// Package device is the capture of the audio devices of each OS, with no
// cgo: AudioQueue and Core Audio on macOS, WASAPI on Windows, and parec or
// pw-record on Linux. The packages mic and system are its public face.
package device

import (
	"context"
	"errors"
	"strings"
	"time"
)

// Device is a microphone of the system.
type Device struct {
	ID        string
	Name      string
	IsDefault bool
}

// Chunk is 16 kHz mono audio from -1 to 1, its level from 0 to 1, and the
// time of its first sample.
type Chunk struct {
	Samples []float32
	RMS     float32
	Time    time.Time
}

// ErrDenied is the error of a start that the user or the system refused.
var ErrDenied = errors.New("microphone access is denied")

// Backend is the capture of one OS. It runs one capture at a time.
type Backend interface {
	ListDevices() ([]Device, error)
	Start(ctx context.Context, deviceID string, onChunk func(Chunk)) error
	Stop() error
}

// cpalHosts are the host names that cpal writes before a device id
// ("<host>:<device>").
var cpalHosts = []string{"coreaudio", "wasapi", "asio", "alsa", "jack", "pulse", "pulseaudio", "pipewire"}

// NormalizeDeviceID strips the cpal host prefix of a device id
// ("coreaudio:<UID>" becomes "<UID>"). Only the first part is a host: an
// ALSA id such as "alsa:hw:0,0" becomes "hw:0,0".
func NormalizeDeviceID(id string) string {
	host, rest, ok := strings.Cut(id, ":")
	if !ok {
		return id
	}
	for _, h := range cpalHosts {
		if strings.EqualFold(host, h) {
			return rest
		}
	}
	return id
}
