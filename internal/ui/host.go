package ui

import (
	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
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
