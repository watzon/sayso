package platform

import "testing"

func TestNormalizeDeviceIDStripsCpalHosts(t *testing.T) {
	for in, want := range map[string]string{
		"coreaudio:BuiltInMicrophoneDevice": "BuiltInMicrophoneDevice",
		"CoreAudio:BuiltInMicrophoneDevice": "BuiltInMicrophoneDevice",
		"wasapi:{0.0.1.00000000}.{abc}":     "{0.0.1.00000000}.{abc}",
		"alsa:hw:0,0":                       "hw:0,0",
		"pulse:alsa_input.usb-mic":          "alsa_input.usb-mic",
		"pulseaudio:alsa_input.usb-mic":     "alsa_input.usb-mic",
		"pipewire:42":                       "42",
		"BuiltInMicrophoneDevice":           "BuiltInMicrophoneDevice",
		// A CoreAudio UID may itself hold colons; only a cpal host is a prefix.
		"AppleUSBAudioEngine:Vendor:Mic:1": "AppleUSBAudioEngine:Vendor:Mic:1",
		"":                                 "",
	} {
		if got := NormalizeDeviceID(in); got != want {
			t.Errorf("NormalizeDeviceID(%q) = %q, want %q", in, got, want)
		}
	}
}
