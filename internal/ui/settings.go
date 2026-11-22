package ui

import (
	"fmt"
	"log"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
	"github.com/watzon/sayso/internal/update"
)

type settingsState struct {
	name         string
	enginePath   string
	engineEdited bool   // the engine path changed since launch
	recording    string // the key of the recorder that listens
	confirmClear bool
	// confirmAudio asks once more before Delete audio deletes.
	confirmAudio bool
	// storage is what History, audio and models take on disk, read off the
	// UI thread.
	storage     storageCache
	customInk   string
	inkErr      string
	devices     []platform.AudioDevice
	devicesRead bool
	importMsg   string
	provName    string
	provURL     string
	provKey     string
	provModel   string
	provErr     string
	keyEdit     map[string]string
	provSel     string // the provider Settings › Providers shows
	customOpen  bool   // Appearance shows the custom ink field
	customField mu.Handle
}

func (a *App) settingsPage(c *mu.Context, p theme.Colors) {
	s := a.section
	sub := map[Section]string{
		SecGeneral: i18n.TOS("settings.general.subtitle", nil), SecDictation: i18n.T("settings.dictation.subtitle"),
		SecAudio: i18n.T("settings.audio.subtitle"), SecProviders: i18n.T("settings.providers.subtitle"),
		SecOverlay: i18n.T("settings.overlay.subtitle"), SecAppearance: i18n.T("settings.appearance.subtitle"),
		SecHistory: i18n.TOS("settings.history.subtitle", nil), SecApps: i18n.T("apps.settings.subtitle"),
		SecUpdates: i18n.T("settings.general.check_updates_detail"), SecAdvanced: i18n.T("settings.advanced.subtitle"),
		SecPermissions: i18n.TOS("settings.permissions.subtitle", nil),
	}[s]
	if s == SecProviders {
		a.providersSection(c, p, sub) // its header has the AI switch
		return
	}
	pageHeader(c, p, s.title(), sub)
	switch s {
	case SecGeneral:
		a.generalSection(c, p)
	case SecDictation:
		a.dictationSection(c, p)
	case SecAudio:
		a.audioSection(c, p)
	case SecOverlay:
		a.overlaySection(c, p)
	case SecAppearance:
		a.appearanceSection(c, p)
	case SecHistory:
		a.historySection(c, p)
	case SecPermissions:
		a.permissionsSection(c, p)
	case SecApps:
		a.appsSection(c, p)
	case SecUpdates:
		a.updatesSection(c, p)
	case SecAdvanced:
		a.advancedSection(c, p)
	}
}

// ---- General ----

func (a *App) generalSection(c *mu.Context, p theme.Colors) {
	if n := len(a.d.Issues); n > 0 {
		uiText(c, i18n.TP("settings.general.config_problems", n, nil), 13, 500, p.Danger).
			Padding(10, 14).Radius(10).Background(p.DangerWash)
	}
	a.group(c, p, i18n.T("settings.general.language"), func() {
		a.row(c, p, i18n.T("settings.general.interface_language"), i18n.T("settings.general.interface_language_detail"), func() {
			a.languageSelect(c, "general.language")
		})
	})
	a.group(c, p, i18n.T("settings.general.you"), func() {
		a.row(c, p, i18n.T("settings.general.name"), i18n.TOS("settings.general.name_detail", nil), func() {
			a.fieldW(c, p, "general.name", i18n.T("settings.general.name"), i18n.T("settings.general.name_placeholder"), &a.set.name, 220).
				OnChange(func() {
					if v := strings.TrimSpace(a.set.name); v == "" {
						a.cfg.General.Name = nil
					} else {
						a.cfg.General.Name = ptr(v)
					}
					a.saveConfig()
				})
		})
	})
	a.group(c, p, i18n.T("settings.general.startup"), func() {
		a.toggleRow(c, p, "general.launch_at_login", i18n.T("settings.general.launch_at_login"), i18n.TOS("settings.general.launch_detail", nil),
			&a.cfg.General.LaunchAtLogin, func() {
				a.saveConfig()
				if a.d.Platform != nil && a.d.Platform.LoginItem != nil {
					if err := a.d.Platform.LoginItem.SetEnabled(a.cfg.General.LaunchAtLogin); err != nil {
						a.flash = i18n.TOS("settings.general.login_item_failed", nil)
					}
				}
			})
		if runtime.GOOS == "darwin" {
			a.toggleRow(c, p, "general.dock_icon", i18n.T("settings.general.dock_icon"), i18n.T("settings.general.dock_icon_detail"),
				&a.cfg.General.DockIconWithHub, func() {
					a.saveConfig()
					if a.d.Platform != nil && a.d.Platform.Dock != nil {
						a.d.Platform.Dock.SetShowInDock(a.cfg.General.DockIconWithHub)
					}
				})
		}
	})
	a.group(c, p, i18n.T("settings.general.updates"), func() {
		if !update.IsOfficialBuild() {
			a.row(c, p, i18n.T("settings.general.no_self_update"), i18n.T("settings.general.no_self_update_detail", "version", a.d.Version), nil)
			return
		}
		a.row(c, p, a.updateHeadline(), a.updateDetail(), func() {
			if a.btn(c, p, "general.check_now", i18n.T("update.action.check"), btnSecondary) {
				a.CheckForUpdates()
			}
		})
	})
	a.group(c, p, i18n.T("settings.general.about"), func() {
		a.row(c, p, "Sayso", i18n.T("settings.general.version", "version", a.d.Version), func() {
			uiText(c, i18n.TOS("settings.general.tagline", map[string]any{"os": osName()}), 13, 400, p.Graphite)
		})
		a.row(c, p, i18n.T("settings.general.whats_new"), "", func() {
			if a.btn(c, p, "general.whats_new", i18n.T("settings.general.whats_new"), btnSecondary) {
				a.OpenWhatsNew()
			}
		})
		a.row(c, p, i18n.T("settings.history.import_pindrop"), a.pindropLine(), func() {
			if a.btn(c, p, "general.import_pindrop", i18n.T("settings.history.import"), btnSecondary) {
				a.importPindrop()
			}
		})
		a.row(c, p, i18n.T("settings.general.config_file"), tildePath(a.d.ConfigPath), func() {
			if a.btn(c, p, "general.open_config", i18n.T("settings.general.open_config_file"), btnGhost) {
				a.openPath(a.d.ConfigPath)
			}
		})
	})
}

func osName() string {
	switch runtime.GOOS {
	case "darwin":
		return "macOS"
	case "windows":
		return "Windows"
	}
	return "Linux"
}

func (a *App) languageSelect(c *mu.Context, key string) {
	options := []string{i18n.T("settings.general.language_system")}
	codes := []string{i18n.System}
	for _, l := range i18n.Available() {
		options, codes = append(options, l.Name), append(codes, l.Code)
	}
	if i18n.HasPseudo {
		options, codes = append(options, "Pseudo (en-XA)"), append(codes, i18n.Pseudo)
	}
	sel := options[0]
	for i, code := range codes {
		if code == a.cfg.General.Language {
			sel = options[i]
		}
	}
	a.selectBtn(c, a.colors(c), key, i18n.T("settings.general.interface_language"), &sel, options, 0).OnChange(func() {
		for i, o := range options {
			if o == sel {
				a.SetLanguage(codes[i])
			}
		}
	})
}

func pindropDir() string {
	home, _ := os.UserHomeDir()
	if runtime.GOOS == "darwin" {
		return filepath.Join(home, "Library", "Application Support", "Pindrop")
	}
	return filepath.Join(home, ".local", "share", "pindrop")
}

func (a *App) pindropLine() string {
	if a.set.importMsg != "" {
		return a.set.importMsg
	}
	return i18n.T("app.pindrop.idle")
}

func (a *App) importPindrop() {
	if a.d.Store == nil {
		return
	}
	dir := pindropDir()
	if !store.PindropFound(dir) {
		a.set.importMsg = i18n.T("app.pindrop.failed", "reason", dir)
		return
	}
	a.set.importMsg = i18n.T("app.pindrop.running")
	db := a.d.Store
	go func() {
		data, err := store.ReadPindrop(dir)
		var rep store.ImportReport
		if err == nil {
			rep, err = db.ImportPindrop(data)
		}
		a.post(func() {
			switch {
			case err != nil:
				a.set.importMsg = i18n.T("app.pindrop.failed", "reason", err.Error())
			case rep.Words+rep.Replacements+rep.Dictations == 0:
				a.set.importMsg = i18n.T("app.pindrop.nothing_new")
			default:
				a.set.importMsg = i18n.T("app.pindrop.added", "items", fmt.Sprintf("%s, %s, %s",
					i18n.TP("app.pindrop.dictations", rep.Dictations, map[string]any{"number": rep.Dictations}),
					i18n.TP("app.pindrop.words", rep.Words, map[string]any{"number": rep.Words}),
					i18n.TP("app.pindrop.replacements", rep.Replacements, map[string]any{"number": rep.Replacements})))
			}
			a.dataChanged()
			a.invalidate()
		})
	}()
}

// ---- Dictation ----

func (a *App) dictationSection(c *mu.Context, p theme.Colors) {
	h := &a.cfg.Hotkeys
	setHK := func(dst **config.Hotkey) func(*config.Hotkey) {
		return func(nh *config.Hotkey) {
			*dst = nh
			a.saveConfig()
			a.applyHotkeys()
		}
	}
	a.group(c, p, i18n.T("settings.dictation.language"), func() {
		// The languages the active model understands (dictation.rs
		// language_group); a one-language model shows its language, disabled.
		langs := []string{"auto", "en", "es", "fr", "de", "it", "pt", "ja", "zh", "ko", "ru", "nl"}
		detail, single := "", false
		if m, ok := models.Find(a.cfg.Dictation.Model); ok && len(m.Languages) > 0 {
			current := m.LanguageFor(a.cfg.Dictation.Language)
			codes := slices.Clone(m.Languages)
			sort.SliceStable(codes, func(i, j int) bool { return i18n.LanguageName(codes[i]) < i18n.LanguageName(codes[j]) })
			if m.IsMultilingual() {
				codes = append([]string{"auto"}, codes...)
				detail = i18n.T("settings.dictation.language_detail_multi", "model", m.Name,
					"languages", strings.ToLower(i18n.TP("core.models.languages", len(m.Languages), nil)))
			} else {
				single = true
				detail = i18n.T("settings.dictation.language_detail_single", "model", m.Name, "language", i18n.LanguageName(current))
			}
			langs = codes
		}
		a.row(c, p, i18n.T("settings.dictation.dictation_language"), detail, func() {
			labels := make([]string, len(langs))
			sel := langs[0]
			for i, l := range langs {
				labels[i] = i18n.LanguageName(l)
				if l == "auto" {
					labels[i] = i18n.T("models.setup.automatic")
				}
				if l == a.cfg.Dictation.Language {
					sel = labels[i]
				}
			}
			if a.cfg.Dictation.Language == "" || !slices.Contains(langs, a.cfg.Dictation.Language) {
				sel = labels[0]
			}
			b := a.selectBtn(c, p, "dictation.language", i18n.T("settings.dictation.dictation_language"), &sel, labels, 0)
			if single {
				b.Disabled(true).Opacity(0.55)
			}
			b.OnChange(func() {
				for i, l := range labels {
					if l == sel {
						a.cfg.Dictation.Language = langs[i]
					}
				}
				a.saveConfig()
			})
		})
		a.toggleRow(c, p, "dictation.spoken_punctuation", i18n.T("settings.dictation.spoken_punctuation"), i18n.T("settings.dictation.spoken_punctuation_detail"), &a.cfg.Dictation.SpokenPunctuation, a.saveConfig)
	})
	a.group(c, p, i18n.T("settings.dictation.hotkeys"), func() {
		a.row(c, p, i18n.T("settings.dictation.toggle"), i18n.T("settings.dictation.toggle_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.toggle", i18n.T("settings.dictation.toggle"), h.Toggle, setHK(&h.Toggle))
		})
		a.row(c, p, i18n.T("settings.dictation.push_to_talk"), i18n.T("settings.dictation.push_to_talk_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.push_to_talk", i18n.T("settings.dictation.push_to_talk"), h.PushToTalk, setHK(&h.PushToTalk))
		})
		a.row(c, p, i18n.T("settings.dictation.cancel"), i18n.T("settings.dictation.cancel_detail"), func() {
			modes := []config.CancelMode{config.CancelDoubleEscape, config.CancelSingleEscape}
			chosen := 0
			if h.Cancel == config.CancelSingleEscape {
				chosen = 1
			}
			if i := a.choice(c, p, []string{"dictation.cancel.double", "dictation.cancel.single"},
				[]string{i18n.T("settings.dictation.cancel_double"), i18n.T("settings.dictation.cancel_single")}, chosen, false); i >= 0 {
				h.Cancel = modes[i]
				a.saveConfig()
				a.applyHotkeys()
			}
		})
		a.toggleRow(c, p, "dictation.take_escape", i18n.T("settings.dictation.take_escape"), i18n.T("settings.dictation.take_escape_detail"), &h.TakeEscape, a.hotkeysChanged)
		a.toggleRow(c, p, "dictation.enter_sends", i18n.T("settings.dictation.enter_sends"), i18n.T("settings.dictation.enter_sends_detail"), &h.EnterSends, a.hotkeysChanged)
		a.row(c, p, i18n.T("settings.dictation.paste_last"), i18n.T("settings.dictation.paste_last_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.paste_last", i18n.T("settings.dictation.paste_last"), h.PasteLast, setHK(&h.PasteLast))
		})
		a.row(c, p, i18n.T("settings.dictation.next_style"), i18n.T("settings.dictation.next_style_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.cycle_style", i18n.T("settings.dictation.next_style"), h.CycleStyle, setHK(&h.CycleStyle))
		})
		a.row(c, p, i18n.T("settings.dictation.incognito"), i18n.T("settings.dictation.incognito_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.incognito", i18n.T("settings.dictation.incognito"), h.Incognito, setHK(&h.Incognito))
		})
	})
	a.group(c, p, i18n.T("settings.dictation.inserting"), func() {
		a.toggleRow(c, p, "dictation.restore_clipboard", i18n.T("settings.dictation.restore_clipboard"), i18n.T("settings.dictation.restore_clipboard_detail"), &a.cfg.Insertion.RestoreClipboard, a.saveConfig)
		a.toggleRow(c, p, "dictation.trailing_space", i18n.T("settings.dictation.trailing_space"), i18n.T("settings.dictation.trailing_space_detail"), &a.cfg.Insertion.TrailingSpace, a.saveConfig)
		a.row(c, p, tr("settings.dictation.min_duration", "Minimum length"), "", func() {
			a.uintSelect(c, "dictation.min_duration", tr("settings.dictation.min_duration", "Minimum length"), &a.cfg.Dictation.MinDurationMs,
				[]uint64{0, 200, 300, 500, 1000}, func(v uint64) string { return fmt.Sprintf("%d ms", v) })
		})
		a.toggleRow(c, p, "dictation.preload", i18n.T("settings.advanced.preload"), i18n.T("settings.advanced.preload_detail"), &a.cfg.Dictation.PreloadModel, a.saveConfig)
		a.row(c, p, i18n.T("settings.advanced.keep"), i18n.T("settings.advanced.keep_detail"), func() { a.keepModelSelect(c) })
	})
	a.group(c, p, i18n.T("settings.dictation.styles"), func() {
		a.toggleRow(c, p, "dictation.styles_by_app", i18n.T("settings.dictation.styles_by_app_site"), i18n.T("settings.dictation.styles_by_app_site_detail"),
			&a.cfg.AI.StyleRules.Enabled, a.saveConfig)
	})
}

func (a *App) hotkeysChanged() {
	a.saveConfig()
	a.applyHotkeys()
}

// uintSelect picks one of values for *v.
func (a *App) uintSelect(c *mu.Context, key, label string, v *uint64, values []uint64, show func(uint64) string) {
	labels := make([]string, len(values))
	sel := ""
	for i, x := range values {
		labels[i] = show(x)
		if x == *v {
			sel = labels[i]
		}
	}
	if sel == "" {
		labels = append(labels, show(*v))
		values = append(values, *v)
		sel = labels[len(labels)-1]
	}
	a.selectBtn(c, a.colors(c), key, label, &sel, labels, 0).OnChange(func() {
		for i, l := range labels {
			if l == sel {
				*v = values[i]
			}
		}
		a.saveConfig()
	})
}

func (a *App) keepModelSelect(c *mu.Context) {
	values := []*uint32{nil, ptr(uint32(5)), ptr(uint32(15)), ptr(uint32(30)), ptr(uint32(60))}
	labels := []string{i18n.T("settings.advanced.keep_always")}
	for _, v := range values[1:] {
		if *v == 60 {
			labels = append(labels, i18n.T("settings.advanced.keep_hour"))
		} else {
			labels = append(labels, i18n.TP("settings.advanced.keep_minutes", int(*v), nil))
		}
	}
	sel := labels[0]
	for i, v := range values {
		if v != nil && a.cfg.Dictation.KeepModelMinutes != nil && *v == *a.cfg.Dictation.KeepModelMinutes {
			sel = labels[i]
		}
	}
	a.selectBtn(c, a.colors(c), "dictation.keep_model", i18n.T("settings.advanced.keep"), &sel, labels, 0).OnChange(func() {
		for i, l := range labels {
			if l == sel {
				a.cfg.Dictation.KeepModelMinutes = values[i]
			}
		}
		a.saveConfig()
	})
}

// ---- Audio ----

// audioDevices are the microphones, listed once.
func (a *App) audioDevices() []platform.AudioDevice {
	if !a.set.devicesRead && a.d.Platform != nil && a.d.Platform.Audio != nil {
		a.set.devicesRead = true
		if devs, err := a.d.Platform.Audio.ListDevices(); err == nil {
			a.set.devices = devs
		} else {
			log.Printf("audio devices: %v", err)
		}
	}
	return a.set.devices
}

func (a *App) audioSection(c *mu.Context, p theme.Colors) {
	a.audioDevices()
	// The inputs as one radio group (audio.rs): System default with the
	// device it follows now, then each microphone.
	a.group(c, p, i18n.T("settings.audio.input"), func() {
		follows, followsNow := "settings.audio.follows_other", "settings.audio.follows_now_other"
		if runtime.GOOS == "darwin" {
			follows, followsNow = "settings.audio.follows_mac", "settings.audio.follows_now_mac"
		}
		defDetail := i18n.T(follows)
		for _, d := range a.set.devices {
			if d.IsDefault {
				defDetail = i18n.T(followsNow, "name", d.Name)
			}
		}
		labels, ids, details := []string{i18n.T("settings.audio.system_default")}, []string{""}, []string{defDetail}
		for _, d := range a.set.devices {
			labels, ids, details = append(labels, d.Name), append(ids, d.ID), append(details, "")
		}
		chosen := 0
		for i, id := range ids {
			if a.cfg.Audio.InputDevice != nil && id == platform.NormalizeDeviceID(*a.cfg.Audio.InputDevice) {
				chosen = i
			}
		}
		mu.Column(a.k(c, "audio.input_device")).Role(mu.RoleRadioGroup).Label(i18n.T("settings.audio.input")).Value(labels[chosen]).Children(func() {
			for i := range labels {
				on := i == chosen
				key := "audio.device.default"
				if ids[i] != "" {
					key = "audio.device." + ids[i]
				}
				r := mu.ButtonBase(a.k(c, key)).Label(labels[i]).Role(mu.RoleRadio).Checked(on).PaddingY(12).Gap(14).
					AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Cursor(mu.CursorPointer)
				r.Children(func() {
					radioDot(c, p, on)
					mu.Column(c).Gap(2).Grow(1).Basis(0).MinWidth(0).Children(func() {
						w := 500
						if on {
							w = 600
						}
						uiText(c, labels[i], 15, w, p.Ink).FixedLineHeight(18)
						if details[i] != "" {
							uiText(c, details[i], 13, 400, p.Graphite).FixedLineHeight(17)
						}
					})
				})
				if clicked(c, r) && !on {
					if ids[i] == "" {
						a.cfg.Audio.InputDevice = nil
					} else {
						a.cfg.Audio.InputDevice = ptr(ids[i])
					}
					a.saveConfig()
				}
			}
		})
		if len(a.set.devices) == 0 {
			uiText(c, i18n.T("settings.audio.no_microphone", "settings", i18n.TOS("os.sound_settings", nil)), 13, 400, p.Graphite).PaddingY(10).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule)
		}
	})
	a.group(c, p, i18n.T("settings.audio.recording"), func() {
		a.toggleRow(c, p, "dictation.live_preview", i18n.T("settings.audio.live_preview"), i18n.T("settings.audio.live_preview_detail"), &a.cfg.Dictation.LivePreview, a.saveConfig)
		a.toggleRow(c, p, "audio.pause_media", i18n.T("settings.audio.pause_media"), i18n.T("settings.audio.pause_media_detail"), &a.cfg.Audio.PauseMedia, a.saveConfig)
		a.row(c, p, i18n.T("settings.audio.max_length"), i18n.T("settings.audio.max_length_detail"), func() {
			d := &a.cfg.Dictation
			at := len(recordingLengths) - 1
			for i, l := range recordingLengths {
				if l >= d.MaxDurationS {
					at = i
					break
				}
			}
			if n := a.stepper(c, p, "audio.max_length", i18n.T("settings.audio.max_length"), lengthLabel(d.MaxDurationS)); n != 0 {
				d.MaxDurationS = recordingLengths[max(0, min(len(recordingLengths)-1, at+n))]
				a.saveConfig()
			}
		})
	})
	snd := &a.cfg.Sounds
	a.group(c, p, i18n.T("settings.dictation.sounds"), func() {
		a.toggleRow(c, p, "audio.sounds", i18n.T("settings.dictation.sounds_row"), i18n.T("settings.dictation.sounds_detail"), &snd.Enabled, a.saveConfig)
		a.row(c, p, i18n.T("settings.common.volume"), "", func() {
			v := float64(snd.Volume)
			mu.Slider(a.k(c, "audio.volume"), &v, 0, 1).Label(i18n.T("settings.common.volume")).Width(200).Disabled(!snd.Enabled).OnChange(func() {
				snd.Volume = float32(v)
				a.saveConfig()
			})
		})
		a.row(c, p, i18n.T("settings.dictation.sound_events"), i18n.T("settings.dictation.sound_events_detail"), func() {
			mu.Row(c).Gap(12).Children(func() {
				for _, ev := range []struct {
					key, label string
					v          *bool
				}{
					{"audio.sound.start", i18n.T("settings.dictation.sound_start"), &snd.Start},
					{"audio.sound.stop", i18n.T("settings.dictation.sound_stop"), &snd.Stop},
					{"audio.sound.cancel", i18n.T("settings.dictation.sound_cancel"), &snd.Cancel},
					{"audio.sound.insert", i18n.T("settings.dictation.sound_insert"), &snd.Insert},
				} {
					if a.chip(c, p, ev.key, ev.label, *ev.v) && snd.Enabled {
						*ev.v = !*ev.v
						a.saveConfig()
					}
				}
			})
		})
	})
}

// ---- Overlay ----

func (a *App) overlaySection(c *mu.Context, p theme.Colors) {
	o := &a.cfg.Overlay
	notch := o.Style == config.OverlayNotch
	a.group(c, p, i18n.T("settings.overlay.style"), func() {
		uiText(c, i18n.T("settings.overlay.style_detail"), 15, 400, p.Graphite).LineHeight(22.0 / 15).MaxWidth(560).PaddingY(6)
		mu.Row(c).Gap(20).Padding(10, 0, 14, 0).Role(mu.RoleRadioGroup).Label(i18n.T("settings.overlay.style")).Children(func() {
			a.overlayOption(c, p, "overlay.pill", i18n.T("settings.overlay.pill"), i18n.T("settings.overlay.style_pill_detail"), !notch, false, config.OverlayPill)
			a.overlayOption(c, p, "overlay.notch", i18n.T("settings.overlay.style_notch"), i18n.TOS("settings.overlay.style_notch_detail", nil), notch, true, config.OverlayNotch)
		})
	})
	title := i18n.T("settings.overlay.pill")
	if notch {
		title = i18n.T("settings.overlay.style_notch")
	}
	a.group(c, p, title, func() {
		if notch {
			a.toggleRow(c, p, "overlay.notch_other_displays", i18n.T("settings.overlay.notch_other_displays"), i18n.T("settings.overlay.notch_other_displays_detail"), &o.NotchOnOtherDisplays, a.overlayChanged)
		}
		a.row(c, p, i18n.T("settings.overlay.size"), i18n.T("settings.overlay.size_detail"), func() {
			sizes := []config.OverlaySize{config.OverlaySmall, config.OverlayMedium, config.OverlayLarge}
			chosen := 1
			for i, s := range sizes {
				if s == o.Size {
					chosen = i
				}
			}
			if i := a.choice(c, p, []string{"overlay.size.small", "overlay.size.medium", "overlay.size.large"},
				[]string{i18n.T("settings.overlay.size_small"), i18n.T("settings.overlay.size_medium"), i18n.T("settings.overlay.size_large")}, chosen, false); i >= 0 {
				o.Size = sizes[i]
				a.saveConfig()
				if a.d.Shell != nil {
					a.d.Shell.SetOverlaySize(o.Size)
				}
			}
		})
		if !notch {
			a.toggleRow(c, p, "overlay.idle_pill", i18n.T("settings.overlay.idle_pill"), i18n.T("settings.overlay.idle_pill_detail"), &o.IdlePill, a.overlayChanged)
		}
		a.toggleRow(c, p, "overlay.hide_fullscreen", i18n.T("settings.overlay.hide_fullscreen"), i18n.T("settings.overlay.hide_fullscreen_detail"), &o.HideInFullscreen, a.overlayChanged)
		a.toggleRow(c, p, "overlay.follow_display", i18n.T("settings.overlay.follow"), i18n.T("settings.overlay.follow_detail"), &o.FollowDisplay, a.overlayChanged)
		detail := i18n.T("settings.overlay.live_preview_detail")
		if notch {
			detail = i18n.T("settings.overlay.live_preview_notch_detail")
		}
		a.toggleRow(c, p, "overlay.show_preview", i18n.T("settings.overlay.live_preview"), detail, &o.ShowPreview, a.overlayChanged)
	})
}

// overlayOption is a style card: a drawn preview of the pill or the notch,
// its name and what it does. Chosen, it has an ink ring.
func (a *App) overlayOption(c *mu.Context, p theme.Colors, key, title, detail string, on, notch bool, style config.OverlayStyle) {
	b := mu.ButtonBase(a.k(c, key)).Label(title).Role(mu.RoleRadio).Checked(on).Column().Width(264).Gap(10).AlignItems(mu.Stretch).Cursor(mu.CursorPointer)
	b.Children(func() {
		prev := mu.Box(c).Height(132).Radius(14).Clip().Background(p.Deboss)
		prev.Draw(func(pt *mu.Painter, r mu.Rect) {
			// The ink ring of the chosen card goes over the drawing.
			if on {
				defer pt.Stroke(mu.Rect{X: r.X + 1, Y: r.Y + 1, W: r.W - 2, H: r.H - 2}, p.Ink, 13, 2)
			}
			// The menu bar of a screen, then the overlay on it.
			pt.Fill(mu.Rect{X: r.X, Y: r.Y, W: r.W, H: 20}, theme.Over(p.Deboss, p.Sheet, 0.55), 0)
			if notch {
				pt.Fill(mu.Rect{X: r.X, Y: r.Y, W: r.W, H: 4}, black, 0)
				n := mu.Rect{X: r.X + r.W/2 - 76, Y: r.Y, W: 152, H: 18}
				pt.FillRadii(n, [4]float32{0, 0, 9, 9}, black)
				pt.Fill(mu.Rect{X: n.X + 10, Y: n.Y + 7, W: 5, H: 5}, p.Accent, 3)
				pt.Fill(mu.Rect{X: n.X + n.W - 34, Y: n.Y + 8, W: 22, H: 2}, white.Alpha(0.85), 1)
				s := mu.Rect{X: r.X + r.W/2 - 70, Y: r.Y + 20, W: 140, H: 26}
				pt.Shadow(s, 6, 0, 2, 6, 0, p.ShadowA(0.14))
				pt.Fill(s, p.SheetRaised, 6)
				pt.Fill(mu.Rect{X: s.X + 10, Y: s.Y + 8, W: 110, H: 2}, p.Ink.Alpha(0.7), 1)
				pt.Fill(mu.Rect{X: s.X + 10, Y: s.Y + 14, W: 58, H: 2}, p.Ink.Alpha(0.7), 1)
				pt.Fill(mu.Rect{X: s.X + 72, Y: s.Y + 14, W: 26, H: 2}, p.Pencil.Alpha(0.6), 1)
				return
			}
			pl := mu.Rect{X: r.X + r.W/2 - 52, Y: r.Y + r.H - 42, W: 104, H: 26}
			pt.Shadow(pl, 13, 0, 2, 8, 0, p.ShadowA(0.16))
			pt.Fill(pl, p.SheetRaised, 13)
			pt.Fill(mu.Rect{X: pl.X + 6, Y: pl.Y + 6, W: 14, H: 14}, p.Deboss, 7)
			pt.Fill(mu.Rect{X: pl.X + 10, Y: pl.Y + 10, W: 6, H: 6}, p.Accent, 3)
			for i := range 24 {
				h := float32(2 + 5*math.Abs(math.Sin(float64(i)*0.55)))
				pt.Fill(mu.Rect{X: pl.X + 30 + float32(i)*2.6, Y: pl.Y + 13 - h/2, W: 1.6, H: h}, p.Ink, 1)
			}
		})
		mu.Column(c).Gap(4).Children(func() {
			uiText(c, title, 15, 600, p.Ink)
			uiText(c, detail, 13, 400, p.Graphite).LineHeight(18.0 / 13)
		})
	})
	if clicked(c, b) && !on {
		a.cfg.Overlay.Style = style
		a.saveConfig()
		a.syncOverlay()
	}
}

func (a *App) overlayChanged() {
	a.saveConfig()
	a.syncOverlay()
}

// ---- Appearance ----

func (a *App) appearanceSection(c *mu.Context, p theme.Colors) {
	ap := &a.cfg.Appearance
	a.group(c, p, i18n.T("settings.appearance.ink"), func() {
		uiText(c, i18n.T("settings.appearance.ink_detail"), 15, 400, p.Graphite).LineHeight(22.0 / 15).MaxWidth(560).PaddingY(6)
		mu.Row(c).Gap(12).Padding(14, 0, 8, 0).MarginStart(-6).Wrap().Role(mu.RoleRadioGroup).Label(i18n.T("settings.appearance.ink")).Children(func() {
			for _, ink := range inkPresets {
				named := config.NamedInk(ink)
				on := !ap.Ink.IsCustom && ap.Ink.Named == named
				if a.inkSwatch(c, p, "appearance.ink."+string(ink), i18n.T("core.ink."+string(ink)), i18n.T("core.ink."+string(ink)), ink.Color().Hex(), on) {
					a.SetInk(named)
				}
			}
			custom := ""
			if ap.Ink.IsCustom {
				custom = ap.Ink.String()
			}
			if a.inkSwatch(c, p, "appearance.ink.custom_open", i18n.T("settings.appearance.custom_ink"), i18n.T("settings.appearance.custom"), custom, ap.Ink.IsCustom) {
				a.set.customOpen = true
				if a.set.customInk == "" && ap.Ink.IsCustom {
					a.set.customInk = ap.Ink.String()
				}
				a.set.customField.Focus(c)
			}
		})
		if a.set.customOpen || ap.Ink.IsCustom {
			a.row(c, p, i18n.T("settings.appearance.custom_ink"), i18n.T("settings.appearance.hex_hint"), func() {
				mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
					in := a.fieldW(c, p, "appearance.ink.custom", i18n.T("settings.appearance.custom_ink"), "#2D3C8C", &a.set.customInk, 120).Font(theme.Mono).Bind(&a.set.customField)
					in.OnSubmit(a.applyCustomInk)
					if a.btn(c, p, "appearance.ink.custom_apply", i18n.T("settings.appearance.custom"), btnSecondary) {
						a.applyCustomInk()
					}
				})
			})
		}
		if a.set.inkErr != "" {
			uiText(c, a.set.inkErr, 13, 500, p.Danger).PaddingY(8)
		}
		a.inkSpecimens(c)
	})
	a.group(c, p, i18n.T("settings.appearance.paper"), func() {
		a.row(c, p, i18n.T("settings.appearance.theme"), i18n.TOS("settings.appearance.theme_detail", nil), func() {
			modes := []config.ThemeMode{config.ThemeSystem, config.ThemeLight, config.ThemeDark}
			chosen := 0
			for i, m := range modes {
				if m == ap.Theme {
					chosen = i
				}
			}
			if i := a.choice(c, p, []string{"appearance.theme.auto", "appearance.theme.light", "appearance.theme.dark"},
				[]string{i18n.T("settings.appearance.theme_auto"), i18n.T("settings.appearance.theme_light"), i18n.T("settings.appearance.theme_dark")}, chosen, false); i >= 0 {
				a.SetThemeMode(modes[i])
			}
		})
		a.toggleRow(c, p, "appearance.texture", i18n.T("settings.appearance.texture"), i18n.T("settings.appearance.texture_detail"), &ap.PaperTexture, a.saveConfig)
		a.row(c, p, i18n.T("settings.appearance.reduce_motion"), i18n.TOS("settings.appearance.motion_detail", nil), func() {
			chosen := 0
			if ap.ReduceMotion != nil {
				chosen = map[bool]int{true: 1, false: 2}[*ap.ReduceMotion]
			}
			if i := a.choice(c, p, []string{"appearance.motion.auto", "appearance.motion.on", "appearance.motion.off"},
				[]string{i18n.T("settings.appearance.motion_auto"), i18n.T("settings.appearance.motion_on"), i18n.T("settings.appearance.motion_off")}, chosen, false); i >= 0 {
				ap.ReduceMotion = []*bool{nil, ptr(true), ptr(false)}[i]
				a.saveConfig()
			}
		})
		if runtime.GOOS == "linux" {
			a.toggleRow(c, p, "appearance.system_title_bar", i18n.T("settings.appearance.system_title_bar"), i18n.T("settings.appearance.system_title_bar_detail"), &ap.SystemTitleBar, a.saveConfig)
		}
	})
}

// inkSwatch is a 60 px well holding a domed ink disc, with its name below;
// an empty hex is the dashed Custom circle.
func (a *App) inkSwatch(c *mu.Context, p theme.Colors, key, label, shown, hex string, on bool) bool {
	// A column with the name under the swatch; every item has one width so
	// the swatches sit 96 px apart, as in Rust.
	b := mu.ButtonBase(a.k(c, key)).Label(label).Role(mu.RoleRadio).Checked(on).Column().Width(84).Shrink(0).Gap(10).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	b.Children(func() {
		w := mu.Box(c).Size(60, 60).Radius(99).Justify(mu.Center).AlignItems(mu.Center)
		switch {
		case hex == "":
			w.Border(1, p.Pencil).BorderStyle(mu.BorderDashed)
		default:
			shallowWell(w, p)
		}
		if on {
			w.Border(2, p.Ink).BorderStyle(mu.BorderSolid)
		}
		w.Children(func() {
			if hex == "" {
				mu.Icon(c, icon("plus")).FontSize(16).TextColor(p.Graphite)
				return
			}
			sealDot(c, p, hex, 40)
		})
		fg, weight := p.Graphite, 500
		if on {
			fg, weight = p.Ink, 600
		}
		uiText(c, shown, 13, weight, fg).SingleLine()
	})
	return clicked(c, b)
}

func (a *App) applyCustomInk() {
	if a.SetCustomInk(a.set.customInk) {
		a.set.inkErr = ""
	} else {
		a.set.inkErr = i18n.T("settings.appearance.hex_error")
	}
}

// inkSpecimens show the ink on light and on dark paper, with the contrast.
func (a *App) inkSpecimens(c *mu.Context) {
	ti, err := theme.ParseInk(a.cfg.Appearance.Ink.String())
	if err != nil {
		return
	}
	rep := theme.NewContrastReport(ti)
	mu.Row(c).Gap(16).Padding(18, 0, 6, 0).Children(func() {
		for _, dark := range []bool{false, true} {
			q := a.themes.get(a.cfg.Appearance.Ink, dark).Colors
			ratio, pass := rep.LightRatio, rep.LightRatio >= 4.5
			fill := q.SheetRaised
			if dark {
				ratio, pass, fill = rep.DarkRatio, rep.DarkRatio >= 4.5, q.Sheet
			}
			raised(mu.Column(c).Grow(1).Basis(0).Padding(22, 22, 20, 22).Gap(14).Radius(14), q).Background(fill).Children(func() {
				heading(c, q, i18n.T("settings.appearance.specimen_title"), 22)
				uiText(c, i18n.T("settings.appearance.specimen_body"), 13, 400, q.Graphite)
				mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
					inkButton(mu.Box(c).Height(28).PaddingX(12).Radius(8).Justify(mu.Center).AlignItems(mu.Center), q, false).Children(func() {
						uiText(c, i18n.T("settings.appearance.specimen_primary"), 13, 600, q.OnInk)
					})
					mu.Box(c).Size(10, 10).Radius(99).Background(q.Accent).Shadow(0, 0, 0, 2, q.Accent.Alpha(0.25))
					uiText(c, i18n.T("settings.appearance.specimen_recording"), 13, 600, q.Accent).Grow(1)
					key, col := "settings.appearance.specimen_pass", q.Success
					if !pass {
						key, col = "settings.appearance.specimen_fail", q.Danger
					}
					mono(c, i18n.T(key, "ratio", i18n.Decimal(ratio, 1)), 11, col)
				})
			})
		}
	})
}

// ---- History and privacy ----

func (a *App) historySection(c *mu.Context, p theme.Colors) {
	h := &a.cfg.History
	// Keep text and Keep audio are segmented, with Rust's choices
	// (history_privacy.rs TEXT_DAYS, AUDIO_DAYS).
	daySeg := func(key string, dst **uint32, choices []*uint32) {
		keys, labels := make([]string, len(choices)), make([]string, len(choices))
		chosen := 0
		for i, d := range choices {
			if d == nil {
				keys[i], labels[i] = key+".forever", i18n.T("settings.history.forever")
			} else {
				keys[i], labels[i] = fmt.Sprintf("%s.%d", key, *d), i18n.TP("settings.history.days", int(*d), nil)
			}
			if (d == nil && *dst == nil) || (d != nil && *dst != nil && *d == **dst) {
				chosen = i
			}
		}
		if i := a.choice(c, p, keys, labels, chosen, false); i >= 0 && i != chosen {
			*dst = choices[i]
			a.saveConfig()
			a.applyRetention()
			a.set.storage.stale()
		}
	}
	a.group(c, p, i18n.T("settings.history.history"), func() {
		detail := i18n.T("settings.history.save_detail")
		if !h.Enabled {
			detail = i18n.T("settings.history.save_detail_off")
		}
		a.toggleRow(c, p, "history_settings.enabled", i18n.T("settings.history.save"), detail, &h.Enabled, a.saveConfig)
		a.row(c, p, i18n.T("settings.history.keep_text"), i18n.T("settings.history.keep_text_detail"), func() {
			daySeg("history_settings.keep_text", &h.KeepTextDays, []*uint32{nil, ptr(uint32(90)), ptr(uint32(30)), ptr(uint32(7))})
		})
		a.toggleRow(c, p, "history_settings.save_audio", i18n.T("settings.history.save_audio"), i18n.T("settings.history.save_audio_detail"), &h.SaveAudio, a.saveConfig)
		a.row(c, p, i18n.T("settings.history.keep_audio"), i18n.T("settings.history.keep_audio_detail"), func() {
			daySeg("history_settings.keep_audio", &h.KeepAudioDays, []*uint32{ptr(uint32(30)), ptr(uint32(7)), ptr(uint32(1))})
		})
		a.toggleRow(c, p, "history_settings.site_icons", i18n.T("settings.history.site_icons"), i18n.T("settings.history.site_icons_detail"), &h.SiteIcons, a.saveConfig)
	})
	a.group(c, p, i18n.T("settings.history.data", "computer", computer()), func() { a.dataOnThisComputer(c, p) })
	a.group(c, p, i18n.T("settings.history.leaves", "computer", computer()), func() {
		uiText(c, i18n.T("settings.history.leaves_local", "computer", computer()), 13, 400, p.Graphite).PaddingY(14).LineHeight(1.4).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule)
	})
}

func (a *App) applyRetention() {
	if a.d.Store == nil {
		return
	}
	if _, err := a.d.Store.ApplyRetention(a.cfg.History.KeepTextDays, a.cfg.History.KeepAudioDays, a.now()); err != nil {
		log.Printf("retention: %v", err)
	}
	a.dataChanged()
}

func (a *App) confirmClearDialog(c *mu.Context, p theme.Colors) {
	n := 0
	if a.d.Store != nil {
		n, _ = a.d.Store.Count(store.HistoryQuery{})
	}
	mu.Box(c).Absolute().Top(0).Left(0).Right(0).Bottom(0).Background(p.Ground.Alpha(0.55)).ZIndex(50).
		Justify(mu.Center).AlignItems(mu.Center).Children(func() {
		floating(mu.Column(c).Width(420).Padding(24).Gap(14).Radius(14), p).Role(mu.RoleAlertDialog).
			Label(i18n.T("settings.history.clear_history")).Children(func() {
			heading(c, p, i18n.T("settings.history.clear_history"), 22)
			uiText(c, i18n.TP("settings.history.entries_confirm", n, map[string]any{"number": n}), 14, 400, p.Graphite).LineHeight(1.4)
			mu.Row(c).Gap(8).Justify(mu.End).Children(func() {
				if a.btn(c, p, "history_settings.clear_cancel", i18n.T("settings.history.cancel"), btnGhost) {
					a.set.confirmClear = false
				}
				if a.btn(c, p, "history_settings.clear_confirm", i18n.T("settings.history.delete_all"), btnDanger) {
					a.ClearHistory()
				}
			})
		})
	})
}

// ClearHistory deletes every entry and its audio.
func (a *App) ClearHistory() {
	if a.d.Store != nil {
		if _, err := a.d.Store.ClearAll(); err != nil {
			a.flash = err.Error()
		}
	}
	a.set.confirmClear = false
	a.hist.selected = 0
	a.stopPlayer()
	a.dataChanged()
	a.invalidate()
}

// ---- Advanced ----

func (a *App) advancedSection(c *mu.Context, p theme.Colors) {
	a.group(c, p, i18n.T("settings.advanced.engine"), func() {
		a.row(c, p, i18n.T("settings.advanced.engine_path"), i18n.T("settings.advanced.engine_path_detail"), func() {
			a.fieldW(c, p, "advanced.engine_path", i18n.T("settings.advanced.engine_path"), "SaysoEngine", &a.set.enginePath, 280).Font(theme.Mono).FontSize(12).OnChange(func() {
				if v := strings.TrimSpace(a.set.enginePath); v == "" {
					a.cfg.Advanced.EnginePath = nil
				} else {
					a.cfg.Advanced.EnginePath = ptr(v)
				}
				a.set.engineEdited = true
				a.saveConfig()
			})
		})
		if v := strings.TrimSpace(a.set.enginePath); v != "" {
			if _, err := os.Stat(v); err != nil {
				uiText(c, i18n.T("settings.advanced.engine_missing"), 13, 400, p.Danger).PaddingY(10)
			} else if a.set.engineEdited {
				uiText(c, i18n.T("settings.advanced.engine_saved"), 13, 400, p.Graphite).PaddingY(10)
			}
		}
	})
	a.group(c, p, i18n.T("settings.advanced.logs"), func() {
		a.row(c, p, i18n.T("settings.advanced.log_level"), i18n.T("settings.advanced.log_level_detail"), func() {
			levels := []string{"error", "warn", "info", "debug", "trace"}
			sel := derefOr(a.cfg.Advanced.LogLevel, "info")
			a.selectBtn(c, p, "advanced.log_level", i18n.T("settings.advanced.log_level"), &sel, levels, 0).OnChange(func() {
				a.cfg.Advanced.LogLevel = ptr(sel)
				a.saveConfig()
			})
		})
		a.row(c, p, i18n.T("settings.advanced.logs"), i18n.T("settings.advanced.logs_detail"), func() {
			if a.btn(c, p, "advanced.open_logs", i18n.T("settings.advanced.open_logs"), btnSecondary) {
				a.openPath(a.d.Paths.LogDir())
			}
		})
	})
	a.group(c, p, i18n.T("settings.advanced.folders"), func() {
		folder := func(key, label, path string) {
			a.row(c, p, label, path, func() {
				if a.btn(c, p, key, i18n.T("settings.common.reveal"), btnGhost) {
					a.openPath(path)
				}
			})
		}
		folder("advanced.open_config", i18n.T("settings.advanced.folder_config"), a.d.Paths.ConfigDir)
		folder("advanced.open_data", i18n.T("settings.advanced.folder_data"), a.d.Paths.DataDir)
		folder("advanced.open_cache", i18n.T("settings.advanced.folder_cache"), a.d.Paths.CacheDir)
	})
}

// ---- Permissions ----

// permissionsSection is each permission with its state and a way to the
// system settings, then a note (Rust hub/settings/permissions.rs).
func (a *App) permissionsSection(c *mu.Context, p theme.Colors) {
	var st platform.PermissionStatus
	pm := a.platformPermissions()
	if pm != nil {
		st = pm.Status()
	}
	type perm struct {
		key, name, detail, kind string
		ok                      bool
	}
	list := []perm{
		{"permissions.mic", i18n.T("os.permission.microphone"), i18n.T("settings.permissions.microphone_detail"), "microphone", st.Microphone},
		{"permissions.ax", i18n.TOS("os.permission.accessibility", nil), i18n.T("settings.permissions.accessibility_detail"), "accessibility", st.Accessibility},
	}
	if st.NeedsInputPerms || runtime.GOOS == "darwin" {
		list = append(list, perm{"permissions.im", i18n.TOS("os.permission.input_monitoring", nil), i18n.T("settings.permissions.input_monitoring_detail"), "input_monitoring", st.InputMonitoring})
	}
	a.group(c, p, i18n.T("settings.permissions.title"), func() {
		for _, it := range list {
			a.row(c, p, it.name, it.detail, func() {
				mu.Row(c).Gap(16).AlignItems(mu.Center).Children(func() {
					if it.ok {
						mu.Row(c).Gap(6).AlignItems(mu.Center).Children(func() {
							mu.Box(c).Size(16, 16).Radius(99).Background(p.InkFill).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
								mu.Icon(c, icon("check")).FontSize(10).TextColor(p.OnInk)
							})
							uiText(c, i18n.T("settings.common.allowed"), 13, 600, p.Ink)
						})
					} else if a.btn(c, p, it.key+".request", i18n.T("settings.permissions.request"), btnPrimary) && pm != nil {
						kind := it.kind
						go func() {
							var err error
							switch kind {
							case "microphone":
								err = pm.RequestMicrophone()
							case "accessibility":
								err = pm.RequestAccessibility()
							default:
								err = pm.RequestInputMonitoring()
							}
							if err != nil {
								log.Printf("permission %s: %v", kind, err)
							}
							a.post(a.invalidate)
						}()
					}
					if a.btn(c, p, it.key+".settings", i18n.TOS("os.open_permission_settings", nil), btnSecondary) && pm != nil {
						kind := it.kind
						go func() {
							if err := pm.OpenSettings(kind); err != nil {
								log.Printf("permission settings %s: %v", kind, err)
							}
						}()
					}
				})
			})
		}
	})
	mu.Row(c).Gap(12).Padding(14, 16).Radius(12).Border(1, p.Rule).AlignItems(mu.Start).Margin(-16, 0, 0, 0).Children(func() {
		mu.Icon(c, icon("info")).FontSize(15).TextColor(p.Graphite)
		uiText(c, i18n.TOS("settings.permissions.note", nil), 13, 400, p.Graphite).LineHeight(1.45).Grow(1).Basis(0)
	})
}

// recordingLengths are the steps of Maximum length (audio.rs LENGTHS).
var recordingLengths = []uint64{30, 60, 120, 300, 600, 900, 1800}

// lengthLabel is a recording length: "30 s", "10 min".
func lengthLabel(s uint64) string {
	if s < 60 {
		return i18n.T("settings.audio.length_seconds", "seconds", s)
	}
	return i18n.T("settings.audio.length_minutes", "minutes", s/60)
}

// storageUse is the data of Settings › History › Data on this computer.
type storageUse struct {
	store       store.StorageUse
	modelsBytes uint64
	models      int
}

// storageCache is the last storageUse, read off the UI thread; at is when
// (zero: read it again).
type storageCache struct {
	mu   sync.Mutex
	use  storageUse
	at   time.Time
	busy bool
}

func (s *storageCache) get() storageUse {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.use
}

// stale makes the next frame read the sizes again.
func (s *storageCache) stale() {
	s.mu.Lock()
	s.at = time.Time{}
	s.mu.Unlock()
}

// loadStorage reads the sizes off the UI thread, at most every 10 s.
func (a *App) loadStorage() {
	sc := &a.set.storage
	now := a.now()
	sc.mu.Lock()
	if sc.busy || (!sc.at.IsZero() && now.Sub(sc.at) < 10*time.Second) {
		sc.mu.Unlock()
		return
	}
	sc.busy = true
	sc.mu.Unlock()
	st := a.d.Store
	var modelsBytes uint64
	n := 0
	for _, m := range models.Catalog() {
		if !m.IsRemote() && a.modelStatus(m.ID).IsOnDisk() {
			modelsBytes += m.SizeBytes
			n++
		}
	}
	go func() {
		var u store.StorageUse
		if st != nil {
			u, _ = st.StorageUse()
		}
		sc.mu.Lock()
		sc.use = storageUse{store: u, modelsBytes: modelsBytes, models: n}
		sc.at, sc.busy = now, false
		sc.mu.Unlock()
		a.post(a.invalidate)
	}()
}

// dataOnThisComputer is the size of everything Sayso keeps, as one bar,
// then a row for History, Audio and Models (history_privacy.rs).
func (a *App) dataOnThisComputer(c *mu.Context, p theme.Colors) {
	a.loadStorage()
	u := a.set.storage.get()
	n := 0
	if a.d.Store != nil {
		n, _ = a.d.Store.Count(store.HistoryQuery{})
	}
	parts := []struct {
		bytes uint64
		col   mu.Color
	}{{u.modelsBytes, p.Ink}, {u.store.AudioBytes, p.Accent}, {u.store.DatabaseBytes, p.Pencil}}
	all := uint64(0)
	for _, pt := range parts {
		all += pt.bytes
	}
	mu.Row(c).Gap(28).Padding(12, 0, 16, 0).AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		mu.Row(c).Gap(8).Shrink(0).AlignItems(mu.Baseline).Children(func() {
			display(c, p, models.FormatSize(all), 32).SingleLine()
			uiText(c, i18n.T("settings.history.in_total"), 13, 400, p.Graphite)
		})
		mu.Row(c).Grow(1).Basis(0).Height(10).Padding(2).Gap(2).Radius(99).Background(p.Deboss).
			InsetShadow(0, 1, 2, 0, p.ShadowA(0.28)).Children(func() {
			for _, pt := range parts {
				if pt.bytes == 0 || all == 0 {
					continue
				}
				mu.Box(c).Height(6).Radius(99).Background(pt.col).Grow(max(float32(pt.bytes)/float32(all), 0.01)).Basis(0).Shrink(1)
			}
		})
		if a.link(c, p, "history_settings.show_data", i18n.TOS("settings.history.show", nil), 13, 600, "") {
			_ = os.MkdirAll(a.d.Paths.DataDir, 0o755)
			a.openPath(a.d.Paths.DataDir)
		}
	})
	row := func(dot mu.Color, title, detail string, bytes uint64, action func()) {
		mu.Row(c).Gap(16).PaddingY(14).AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
			mu.Box(c).Size(8, 8).Shrink(0).Radius(99).Background(dot)
			mu.Column(c).Gap(2).Grow(1).Basis(0).MinWidth(0).Children(func() {
				uiText(c, title, 15, 600, p.Ink).FixedLineHeight(18)
				uiText(c, detail, 13, 400, p.Graphite).FixedLineHeight(16)
			})
			mono(c, models.FormatSize(bytes), 13, p.Ink).Width(72).Shrink(0).TextAlign(mu.End)
			mu.Row(c).Width(172).Shrink(0).Justify(mu.End).Gap(8).AlignItems(mu.Center).Children(action)
		})
	}
	entries := i18n.T("settings.history.entries_none")
	if n > 0 {
		key := "settings.history.entries"
		if !a.cfg.History.Enabled {
			key = "settings.history.entries_kept"
		}
		entries = i18n.TP(key, n, map[string]any{"number": formatCount(n)})
	}
	row(p.Pencil, i18n.T("settings.history.data_history"), entries, u.store.DatabaseBytes, func() {
		if a.button(c, p, "history_settings.clear", i18n.T("settings.history.clear_history"), bopt{kind: btnDanger, size: sizeSmall, disabled: n == 0}) {
			a.set.confirmClear = true
		}
	})
	files := u.store.AudioFiles
	audio := i18n.T("settings.history.recordings_none")
	switch {
	case a.set.confirmAudio:
		audio = i18n.TP("settings.history.recordings_confirm", files, map[string]any{"number": formatCount(files)})
	case files > 0:
		audio = i18n.TP("settings.history.recordings", files, map[string]any{"number": formatCount(files)})
	}
	row(p.Accent, i18n.T("settings.history.data_audio"), audio, u.store.AudioBytes, func() {
		if !a.set.confirmAudio {
			if a.button(c, p, "history_settings.delete_audio", i18n.T("settings.history.delete_audio"), bopt{kind: btnDanger, size: sizeSmall, disabled: files == 0}) {
				a.set.confirmAudio = true
			}
			return
		}
		if a.button(c, p, "history_settings.delete_audio_cancel", i18n.T("settings.history.cancel"), bopt{kind: btnGhost, size: sizeSmall}) {
			a.set.confirmAudio = false
		}
		if a.button(c, p, "history_settings.delete_audio_confirm", i18n.T("settings.history.delete_audio"), bopt{kind: btnDanger, size: sizeSmall}) {
			a.set.confirmAudio = false
			if a.d.Store != nil {
				if _, err := a.d.Store.ClearAudio(); err != nil {
					a.flash = err.Error()
				}
			}
			a.stopPlayer()
			a.set.storage.stale()
			a.dataChanged()
		}
	})
	modelsLine := i18n.T("settings.history.models_none")
	if u.models > 0 {
		modelsLine = i18n.TP("settings.history.models", u.models, nil)
	}
	row(p.Ink, i18n.T("settings.history.data_models"), modelsLine, u.modelsBytes, func() {
		if a.link(c, p, "history_settings.open_models", i18n.T("settings.history.open_models"), 13, 600, "") {
			a.CloseSettings()
			a.SetPage(PageModels)
		}
	})
}
