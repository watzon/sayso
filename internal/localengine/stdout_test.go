package localengine

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"
)

const guardChildEnv = "SAYSO_STDOUT_GUARD_CHILD"

// In the child: install the guard, then print the way Go code and a native
// library would. Only the protocol line may reach the real stdout.
func init() {
	if os.Getenv(guardChildEnv) != "1" {
		return
	}
	protocol := protocolStdout()
	fmt.Println("stray go print")
	writeRawStdout("stray native print\n")
	fmt.Fprintln(protocol, "protocol line")
	os.Exit(0)
}

func TestTheStdoutGuardKeepsStrayPrintsOffTheProtocolStream(t *testing.T) {
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(exe, "-test.run=^$")
	cmd.Env = append(os.Environ(), guardChildEnv+"=1")
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	if err := cmd.Run(); err != nil {
		t.Fatalf("%v: %s", err, errb.String())
	}
	if out.String() != "protocol line\n" {
		t.Fatalf("stdout %q", out.String())
	}
	if !strings.Contains(errb.String(), "stray go print") || !strings.Contains(errb.String(), "stray native print") {
		t.Fatalf("stderr %q", errb.String())
	}
}
