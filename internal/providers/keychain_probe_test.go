package providers

import (
	"os"
	"testing"
)

// TestKeychainProbe reads an account that does not exist from the real OS
// store. Opt in with SAYSO_KEYCHAIN_PROBE=1; it never writes.
func TestKeychainProbe(t *testing.T) {
	if os.Getenv("SAYSO_KEYCHAIN_PROBE") != "1" {
		t.Skip("set SAYSO_KEYCHAIN_PROBE=1 to read the OS secret store")
	}
	s, err := osGet("sayso-go-port-probe-does-not-exist")
	if err != nil || s != "" {
		t.Fatalf("%q %v", s, err)
	}
}
