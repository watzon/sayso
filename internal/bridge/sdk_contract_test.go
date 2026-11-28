package bridge

import (
	"encoding/json"
	"fmt"
	"slices"
	"testing"

	sdk "github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/protocol/corpus"
)

func hostEncode(line string) (string, error) {
	var head struct{ Type string }
	json.Unmarshal([]byte(line), &head)
	var decoded any
	var err error
	if slices.Contains(RequestTypes, head.Type) {
		decoded, err = DecodeRequest(line)
	} else {
		decoded, err = DecodeHost(line)
	}
	if err != nil {
		return "", err
	}
	switch d := decoded.(type) {
	case *Message[Request]:
		return Encode(d.ID, d.Body), nil
	case *Message[any]:
		switch body := d.Body.(type) {
		case OKFields:
			return Encode(d.ID, ReplyOK(map[string]json.RawMessage(body))), nil
		case *ErrorReply:
			return Encode(d.ID, body), nil
		case Event:
			return Encode(d.ID, body), nil
		}
	case *Unknown:
		return "unknown " + d.Kind, nil
	}
	return "", fmt.Errorf("decoded to %T", decoded)
}

func sdkEncode(line string) (string, error) {
	var head struct{ Type string }
	json.Unmarshal([]byte(line), &head)
	var decoded any
	var err error
	if sdk.IsRequestType(head.Type) {
		decoded, err = sdk.DecodeRequest(line)
	} else {
		decoded, err = sdk.DecodeHost(line)
	}
	if err != nil {
		return "", err
	}
	switch d := decoded.(type) {
	case *sdk.Message[sdk.Request]:
		return sdk.Encode(d.ID, d.Body), nil
	case *sdk.Message[any]:
		switch body := d.Body.(type) {
		case sdk.OKFields:
			return sdk.Encode(d.ID, sdk.ReplyOK(map[string]json.RawMessage(body))), nil
		case *sdk.ErrorReply:
			return sdk.Encode(d.ID, body), nil
		case sdk.Event:
			return sdk.Encode(d.ID, body), nil
		}
	case *sdk.Unknown:
		return "unknown " + d.Kind, nil
	}
	return "", fmt.Errorf("decoded to %T", decoded)
}

func TestTheHostAndTheSDKReadTheCorpusAlike(t *testing.T) {
	cases, err := corpus.Read("host", "")
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		t.Run(c.String(), func(t *testing.T) {
			host, herr := hostEncode(c.Message)
			lib, serr := sdkEncode(c.Message)
			if (herr == nil) != (serr == nil) {
				t.Fatalf("host error %v, sdk error %v", herr, serr)
			}
			if herr != nil {
				return
			}
			if host == lib {
				return
			}
			same, err := corpus.Same(host, lib)
			if err != nil || !same {
				t.Fatalf("\n host %s\n sdk  %s\n%v", host, lib, err)
			}
		})
	}
}

func strs[T ~string](v []T) []string {
	out := make([]string, len(v))
	for i, s := range v {
		out[i] = string(s)
	}
	return out
}

func TestTheHostAndTheSDKKnowTheSameNames(t *testing.T) {
	for _, c := range []struct {
		name      string
		host, sdk []string
	}{
		{"request types", RequestTypes, sdk.RequestTypes},
		{"event types", EventTypes, sdk.EventTypes},
		{"capabilities", strs(AllCapabilities), strs(sdk.AllCapabilities)},
		{"error codes", strs([]ErrorCode{CodeNotApproved, CodeCapability, CodeBusy, CodeDeclined, CodeModelNotDownloaded, CodeWrongRole, CodeUnsupported, CodeBadRequest, CodeTooManyStreams, CodeUnknownModel, CodeModelRetired, CodeUnavailable, CodeRefused, CodeTimeout, CodePermission, CodeHostNotRunning, CodeConnectionLost, CodeHostClosing}), strs(sdk.ErrorCodes)},
		{"roles", strs([]Role{RoleFinal, RoleStreaming, RoleVad, RoleDiarization}), strs(sdk.Roles)},
		{"model states", strs([]ModelState{StateNotDownloaded, StateDownloading, StateDownloaded, StateOptimizing, StateReady, StateFailed}), strs(sdk.ModelStates)},
		{"engine features of hello", engineFeatures, sdk.HostEngineFeatures},
	} {
		if !slices.Equal(c.host, c.sdk) {
			t.Errorf("%s: host %v, sdk %v", c.name, c.host, c.sdk)
		}
	}
	if Protocol != sdk.Protocol {
		t.Errorf("protocol: host %d, sdk %d", Protocol, sdk.Protocol)
	}
	for _, f := range append(slices.Clone(alwaysFeatures), "mic_capture", "system_capture", "echo_rule") {
		if !slices.Contains(sdk.HostFeatures, f) {
			t.Errorf("the host names feature %q, which the SDK does not know", f)
		}
	}
}
