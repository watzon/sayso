package system

import (
	"errors"
	"runtime"
	"testing"
)

func TestAnUnsupportedSystemSaysSo(t *testing.T) {
	if Supported() {
		t.Skip("this system can capture its sound")
	}
	if _, err := Open(Options{}); !errors.Is(err, ErrUnsupported) {
		t.Fatal(err)
	}
	if Access() != Unknown {
		t.Fatal(Access())
	}
	if runtime.GOOS == "windows" {
		t.Fatal("Windows always has a loopback")
	}
}
