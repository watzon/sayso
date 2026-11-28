package device

// State is the answer of the system about a permission.
type State string

const (
	Granted State = "granted"
	Denied  State = "denied"
	// Unknown: the user was not asked yet, or the system does not say.
	Unknown State = "unknown"
)

// MicPermission is the state of the microphone permission of this process.
func MicPermission() State { return permission() }

// RequestMicPermission asks the user when the system has not asked yet, and
// waits for the answer. It gives the state after it.
func RequestMicPermission() State { return requestPermission() }
