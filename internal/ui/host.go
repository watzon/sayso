package ui

import (
	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/sdk/go/capture/system"
)

// HostServices is what the Host API (connected apps) gets from the app:
// the engine, the config, the styles, and the user's answers.
type HostServices struct {
	bridge.BaseServices
	App    *App
	Speech *EngineAdapter // nil: no engine
}

var _ bridge.Services = (*HostServices)(nil)

// Engine is the speech engine, or nil.
func (h *HostServices) Engine() bridge.Engine {
	if h.Speech == nil {
		return nil
	}
	return h.Speech.BridgeEngine()
}

// Config is the current config.
func (h *HostServices) Config() *config.Config { return h.App.ControllerConfig() }

// Styles are the user's styles.
func (h *HostServices) Styles() *enhance.StyleLibrary { return h.App.ControllerStyles() }

// AskDownload asks the user in the approval window.
func (h *HostServices) AskDownload(q bridge.DownloadQuestion, withdrawn func() bool) bool {
	return h.App.AskDownload(q, withdrawn)
}

// AskApproval asks the user in the approval window.
func (h *HostServices) AskApproval(q bridge.ApprovalQuestion, withdrawn func() bool) (bool, bool) {
	return h.App.AskApproval(q, withdrawn)
}

// Microphone is the microphone of live sessions, or nil where the app has no
// capture for them.
func (h *HostServices) Microphone() bridge.Microphone {
	if h.App.d.Platform == nil || h.App.d.Platform.LiveMic == nil {
		return nil
	}
	return &liveMic{capture: h.App.d.Platform.LiveMic, device: h.inputDevice}
}

// SystemAudio is the capture of the sound that the computer plays, or nil
// where sdk/go/capture/system has no backend (macOS for now).
func (h *HostServices) SystemAudio() bridge.SystemAudio {
	if !system.Supported() {
		return nil
	}
	return &liveSystem{open: openSystemAudio}
}

func (h *HostServices) inputDevice() string {
	if cfg := h.App.ControllerConfig(); cfg != nil && cfg.Audio.InputDevice != nil {
		return *cfg.Audio.InputDevice
	}
	return ""
}
