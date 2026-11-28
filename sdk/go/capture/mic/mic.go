// Package mic captures the microphone as 16 kHz mono audio, with no cgo:
// AudioQueue on macOS, WASAPI on Windows, and parec, pw-record, or arecord
// on Linux. A Stream starts the capture again when the device goes, and
// reports the audio that it lost.
package mic

import "github.com/watzon/sayso/sdk/go/capture/internal/device"

type (
	// Device is a microphone of the system.
	Device = device.Device
	// Chunk is 20 ms of 16 kHz mono audio from -1 to 1, with its level and
	// the time of its first sample.
	Chunk = device.Chunk
	// Options of Open.
	Options = device.Options
	// Stream is a capture that runs.
	Stream = device.Stream
	// State is the answer of the system about a permission.
	State = device.State
)

const (
	Granted = device.Granted
	Denied  = device.Denied
	Unknown = device.Unknown
)

// ErrDenied is wrapped by the error of a start that the system refused.
var ErrDenied = device.ErrDenied

// Devices are the microphones of the system.
func Devices() ([]Device, error) { return device.NewMic().ListDevices() }

// Open starts the microphone opts.Device ("" for the default one).
func Open(opts Options) (*Stream, error) {
	return device.Open(device.NewMic(), opts, device.StallAfter)
}

// NormalizeDeviceID strips the cpal host prefix of a device id
// ("coreaudio:<UID>" becomes "<UID>").
func NormalizeDeviceID(id string) string { return device.NormalizeDeviceID(id) }

// Permission is the state of the microphone permission of this process.
func Permission() State { return device.MicPermission() }

// RequestPermission asks the user when the system has not asked yet, and
// waits for the answer.
func RequestPermission() State { return device.RequestMicPermission() }
