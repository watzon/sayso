// Package permission reads and asks for the permissions of audio capture:
// the microphone, and the sound that the computer plays.
package permission

import (
	"github.com/watzon/sayso/sdk/go/capture/mic"
	"github.com/watzon/sayso/sdk/go/capture/system"
)

// State is the answer of the system about a permission.
type State = mic.State

const (
	Granted = mic.Granted
	Denied  = mic.Denied
	Unknown = mic.Unknown
)

// Microphone is the state of the microphone permission.
func Microphone() State { return mic.Permission() }

// RequestMicrophone asks the user when the system has not asked yet.
func RequestMicrophone() State { return mic.RequestPermission() }

// SystemAudio is the state of the permission to capture the system audio.
// It is Unknown where the system cannot capture it.
func SystemAudio() State { return system.Access() }

// RequestSystemAudio asks for it where the system has such a question.
func RequestSystemAudio() State { return system.Request() }
