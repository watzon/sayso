package ui

import (
	"strings"
	"sync"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/platform"
)

// Command is something the command line asks the running app to do:
// "sayso --toggle" from a desktop shortcut, or a second launch.
type Command struct {
	Kind  CommandKind
	Style string // CmdStyle
}

// CommandKind is the kind of a Command.
type CommandKind int

const (
	CmdNone CommandKind = iota
	CmdOpen
	CmdToggle
	CmdCancel
	CmdPasteLast
	CmdCycleStyle
	CmdIncognito
	CmdStyle
	CmdPushToTalkDown
	CmdPushToTalkUp
	CmdPushToTalkUpSubmit
)

var commandFlags = map[string]CommandKind{
	"--open": CmdOpen, "--toggle": CmdToggle, "--cancel": CmdCancel, "--paste-last": CmdPasteLast,
	"--cycle-style": CmdCycleStyle, "--incognito": CmdIncognito, "--push-to-talk-down": CmdPushToTalkDown,
	"--push-to-talk-up": CmdPushToTalkUp, "--push-to-talk-up-submit": CmdPushToTalkUpSubmit,
}

// ParseCommand finds the command in a command line (without the program
// name). The first command flag wins; other arguments are ignored.
func ParseCommand(args []string) Command {
	for _, a := range args {
		if k, ok := commandFlags[a]; ok {
			return Command{Kind: k}
		}
		if id, ok := strings.CutPrefix(a, "--style="); ok && id != "" {
			return Command{Kind: CmdStyle, Style: id}
		}
	}
	return Command{}
}

// RunCommand does a command in the running app.
func (a *App) RunCommand(cmd Command) {
	ctl := a.d.Dictation
	switch cmd.Kind {
	case CmdOpen:
		if a.d.Shell != nil {
			a.d.Shell.ShowHub()
		}
	case CmdToggle:
		a.Toggle()
	case CmdCancel:
		a.CancelDictation()
	case CmdPasteLast:
		a.PasteLast()
	case CmdCycleStyle:
		a.CycleStyle()
	case CmdIncognito:
		a.SetIncognito(!a.incognito)
	case CmdStyle:
		if ctl != nil {
			ctl.PickStyle(cmd.Style)
		}
	case CmdPushToTalkDown:
		if ctl != nil {
			ctl.PushToTalkDown()
		}
	case CmdPushToTalkUp, CmdPushToTalkUpSubmit:
		if ctl != nil {
			ctl.PushToTalkUp()
		}
	}
}

// HandleHotkey does what a global hotkey asks.
func (a *App) HandleHotkey(ev platform.HotkeyEvent) {
	ctl := a.d.Dictation
	switch ev.Kind {
	case platform.HotkeyToggle:
		a.Toggle()
	case platform.HotkeyPushToTalkDown:
		if ctl != nil {
			ctl.PushToTalkDown()
		}
	case platform.HotkeyPushToTalkUp, platform.HotkeyEnterSend:
		if ctl != nil {
			if a.state.Kind == dictation.StateRecording && a.state.Trigger == dictation.TriggerToggle {
				ctl.Toggle()
			} else {
				ctl.PushToTalkUp()
			}
		}
	case platform.HotkeyCancel:
		a.CancelDictation()
	case platform.HotkeyPasteLast:
		a.PasteLast()
	case platform.HotkeyCycleStyle:
		a.CycleStyle()
	case platform.HotkeyToggleIncognito:
		a.SetIncognito(!a.incognito)
	case platform.HotkeyStyle:
		if ctl != nil {
			ctl.PickStyle(ev.StyleID)
		}
	}
}

// snapshot is what the dictation controller reads from its own goroutine:
// copies that the UI thread replaces whole under a lock.
type snapshot struct {
	mu        sync.Mutex
	cfg       *config.Config
	styles    *enhance.StyleLibrary
	replacer  *enhance.Replacer
	vocab     []string
	incognito bool
}

// publish makes the current model visible to the controller.
func (a *App) publish() {
	cfg := a.cfg
	a.loadDictionary()
	vocab := make([]string, len(a.dict.words))
	for i, w := range a.dict.words {
		vocab[i] = w.Text
	}
	r := enhance.NewReplacer(a.dict.reps)
	a.snap.mu.Lock()
	a.snap.cfg, a.snap.styles, a.snap.replacer, a.snap.vocab, a.snap.incognito = &cfg, a.styles, r, vocab, a.incognito
	a.snap.mu.Unlock()
	if a.onConfig != nil {
		a.onConfig(&cfg)
	}
}

// ControllerConfig is the config for the controller (any goroutine).
func (a *App) ControllerConfig() *config.Config {
	a.snap.mu.Lock()
	defer a.snap.mu.Unlock()
	return a.snap.cfg
}

// ControllerStyles is the style library for the controller.
func (a *App) ControllerStyles() *enhance.StyleLibrary {
	a.snap.mu.Lock()
	defer a.snap.mu.Unlock()
	return a.snap.styles
}

// ControllerReplacer is the dictionary's replacer for the controller.
func (a *App) ControllerReplacer() *enhance.Replacer {
	a.snap.mu.Lock()
	defer a.snap.mu.Unlock()
	return a.snap.replacer
}

// ControllerVocabulary is the dictionary's words for the controller.
func (a *App) ControllerVocabulary() []string {
	a.snap.mu.Lock()
	defer a.snap.mu.Unlock()
	return a.snap.vocab
}

// ControllerIncognito is incognito for the controller.
func (a *App) ControllerIncognito() bool {
	a.snap.mu.Lock()
	defer a.snap.mu.Unlock()
	return a.snap.incognito
}
