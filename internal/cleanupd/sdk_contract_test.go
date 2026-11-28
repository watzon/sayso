package cleanupd

import (
	"testing"

	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/sdk/go/cleanup"
)

// The runtime, the client of the app, and the client of the SDK must name
// the protocol alike.
func TestTheRuntimeAndTheSDKShareTheCleanupProtocol(t *testing.T) {
	if ProtocolVersion != cleanup.ProtocolVersion || engine.CleanupProtocolVersion != cleanup.ProtocolVersion {
		t.Fatal("protocol", ProtocolVersion, engine.CleanupProtocolVersion, cleanup.ProtocolVersion)
	}
	for _, c := range [][3]string{
		{BadRequest, engine.CleanupBadRequest, cleanup.CodeBadRequest},
		{NotLoaded, engine.CleanupNotLoaded, cleanup.CodeNotLoaded},
		{WrongModel, engine.CleanupWrongModel, cleanup.CodeWrongModel},
		{LoadFailed, engine.CleanupLoadFailed, cleanup.CodeLoadFailed},
		{DecodeFailed, engine.CleanupDecodeFailed, cleanup.CodeDecodeFailed},
		{StopEnd, engine.StopEnd, cleanup.StopEnd},
		{StopLimit, engine.StopLimit, cleanup.StopLimit},
		{StopContext, engine.StopContext, cleanup.StopContext},
	} {
		if c[0] != c[1] || c[0] != c[2] {
			t.Errorf("runtime %q, app %q, SDK %q", c[0], c[1], c[2])
		}
	}
}
