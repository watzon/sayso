package device

// Linux has no microphone permission outside a sandbox.
func permission() State        { return Granted }
func requestPermission() State { return Granted }
