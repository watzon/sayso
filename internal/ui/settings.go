package ui

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
)

type settingsState struct {
	name         string
	enginePath   string
	recording    string // the key of the recorder that listens
	confirmClear bool
	customInk    string
	inkErr       string
	devices      []platform.AudioDevice
	devicesRead  bool
	importMsg    string
	provName     string
	provURL      string
	provKey      string
	provModel    string
	provErr      string
	keyEdit      map[string]string
}

func (a *App) settingsPage(c *mu.Context, p theme.Colors) {
	s := a.section
	sub := map[Section]string{
		SecGeneral: i18n.TOS("settings.general.subtitle", nil), SecDictation: i18n.T("settings.dictation.subtitle"),
		SecAudio: i18n.T("settings.audio.subtitle"), SecProviders: i18n.T("settings.providers.subtitle"),
		SecOverlay: i18n.T("settings.overlay.subtitle"), SecAppearance: i18n.T("settings.appearance.subtitle"),
		SecHistory: i18n.TOS("settings.history.subtitle", nil), SecApps: i18n.T("apps.settings.subtitle"),
		SecUpdates: i18n.T("settings.general.check_updates_detail"), SecAdvanced: i18n.T("settings.advanced.subtitle"),
	}[s]
	pageHeader(c, p, s.title(), sub)
	switch s {
	case SecGeneral:
		a.generalSection(c, p)
	case SecDictation:
		a.dictationSection(c, p)
	case SecAudio:
		a.audioSection(c, p)
	case SecProviders:
		a.providersSection(c, p)
	case SecOverlay:
		a.overlaySection(c, p)
	case SecAppearance:
		a.appearanceSection(c, p)
	case SecHistory:
		a.historySection(c, p)
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
		mu.Text(c, i18n.TP("settings.general.config_problems", n, nil)).FontSize(13).TextColor(p.Danger).
			Padding(10, 14).Radius(10).Background(p.DangerWash)
	}
	a.group(c, p, i18n.T("settings.general.you"), func() {
		a.row(c, p, i18n.T("settings.general.name"), i18n.TOS("settings.general.name_detail", nil), func() {
			a.field(c, p, "general.name", i18n.T("settings.general.name"), i18n.T("settings.general.name_placeholder"), &a.set.name).
				Width(220).OnChange(func() {
				if v := strings.TrimSpace(a.set.name); v == "" {
					a.cfg.General.Name = nil
				} else {
					a.cfg.General.Name = ptr(v)
				}
				a.saveConfig()
			})
		})
		a.row(c, p, i18n.T("settings.general.interface_language"), i18n.T("settings.general.interface_language_detail"), func() {
			a.languageSelect(c, "general.language")
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
	a.group(c, p, i18n.T("settings.general.about"), func() {
		a.row(c, p, i18n.T("settings.general.version", "version", a.d.Version), i18n.TOS("settings.general.tagline", map[string]any{"os": osName()}), func() {
			if a.btn(c, p, "general.whats_new", i18n.T("settings.general.whats_new"), btnSecondary) {
				a.OpenWhatsNew()
			}
		})
		a.row(c, p, i18n.T("settings.history.import_pindrop"), a.pindropLine(), func() {
			if a.btn(c, p, "general.import_pindrop", i18n.T("settings.history.import"), btnSecondary) {
				a.importPindrop()
			}
		})
		a.row(c, p, i18n.T("settings.general.config_file"), a.d.ConfigPath, func() {
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
	mu.Select(a.k(c, key), &sel, options).Label(i18n.T("settings.general.interface_language")).Width(220).OnChange(func() {
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
		a.row(c, p, i18n.T("settings.dictation.dictation_language"), "", func() {
			langs := []string{"auto", "en", "es", "fr", "de", "it", "pt", "ja", "zh", "ko", "ru", "nl"}
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
			if a.cfg.Dictation.Language == "" {
				sel = labels[0]
			}
			mu.Select(a.k(c, "dictation.language"), &sel, labels).Label(i18n.T("settings.dictation.dictation_language")).Width(200).OnChange(func() {
				for i, l := range labels {
					if l == sel {
						a.cfg.Dictation.Language = langs[i]
					}
				}
				a.saveConfig()
			})
		})
	})
	a.group(c, p, i18n.T("settings.dictation.hotkeys"), func() {
		a.row(c, p, i18n.T("settings.dictation.toggle"), i18n.T("settings.dictation.toggle_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.toggle", i18n.T("settings.dictation.toggle"), h.Toggle, setHK(&h.Toggle))
		})
		a.row(c, p, i18n.T("settings.dictation.push_to_talk"), i18n.T("settings.dictation.push_to_talk_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.push_to_talk", i18n.T("settings.dictation.push_to_talk"), h.PushToTalk, setHK(&h.PushToTalk))
		})
		a.row(c, p, i18n.T("settings.dictation.paste_last"), i18n.T("settings.dictation.paste_last_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.paste_last", i18n.T("settings.dictation.paste_last"), h.PasteLast, setHK(&h.PasteLast))
		})
		a.row(c, p, i18n.T("settings.dictation.next_style"), i18n.T("settings.dictation.next_style_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.cycle_style", i18n.T("settings.dictation.next_style"), h.CycleStyle, setHK(&h.CycleStyle))
		})
		a.row(c, p, i18n.T("settings.dictation.incognito"), i18n.T("settings.dictation.incognito_detail"), func() {
			a.recorder(c, p, "dictation.hotkey.incognito", i18n.T("settings.dictation.incognito"), h.Incognito, setHK(&h.Incognito))
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
		a.toggleRow(c, p, "dictation.enter_sends", i18n.T("settings.dictation.enter_sends"), i18n.T("settings.dictation.enter_sends_detail"), &h.EnterSends, a.hotkeysChanged)
		a.toggleRow(c, p, "dictation.take_escape", i18n.T("settings.dictation.take_escape"), i18n.T("settings.dictation.take_escape_detail"), &h.TakeEscape, a.hotkeysChanged)
	})
	a.group(c, p, i18n.T("settings.dictation.inserting"), func() {
		a.toggleRow(c, p, "dictation.live_preview", i18n.T("settings.audio.live_preview"), i18n.T("settings.audio.live_preview_detail"), &a.cfg.Dictation.LivePreview, a.saveConfig)
		a.toggleRow(c, p, "dictation.spoken_punctuation", i18n.T("settings.dictation.spoken_punctuation"), i18n.T("settings.dictation.spoken_punctuation_detail"), &a.cfg.Dictation.SpokenPunctuation, a.saveConfig)
		a.toggleRow(c, p, "dictation.restore_clipboard", i18n.T("settings.dictation.restore_clipboard"), i18n.T("settings.dictation.restore_clipboard_detail"), &a.cfg.Insertion.RestoreClipboard, a.saveConfig)
		a.toggleRow(c, p, "dictation.trailing_space", i18n.T("settings.dictation.trailing_space"), i18n.T("settings.dictation.trailing_space_detail"), &a.cfg.Insertion.TrailingSpace, a.saveConfig)
		a.row(c, p, i18n.T("settings.audio.max_length"), i18n.T("settings.audio.max_length_detail"), func() {
			a.uintSelect(c, "dictation.max_duration", i18n.T("settings.audio.max_length"), &a.cfg.Dictation.MaxDurationS,
				[]uint64{60, 120, 300, 600, 1200}, func(v uint64) string { return i18n.T("settings.audio.length_minutes", "minutes", v/60) })
		})
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
	mu.Select(a.k(c, key), &sel, labels).Label(label).Width(160).OnChange(func() {
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
	mu.Select(a.k(c, "dictation.keep_model"), &sel, labels).Label(i18n.T("settings.advanced.keep")).Width(160).OnChange(func() {
		for i, l := range labels {
			if l == sel {
				a.cfg.Dictation.KeepModelMinutes = values[i]
			}
		}
		a.saveConfig()
	})
}

// ---- Audio ----

func (a *App) audioSection(c *mu.Context, p theme.Colors) {
	if !a.set.devicesRead && a.d.Platform != nil && a.d.Platform.Audio != nil {
		a.set.devicesRead = true
		if devs, err := a.d.Platform.Audio.ListDevices(); err == nil {
			a.set.devices = devs
		} else {
			log.Printf("audio devices: %v", err)
		}
	}
	a.group(c, p, i18n.T("settings.audio.input"), func() {
		a.row(c, p, i18n.T("settings.audio.input"), i18n.T("settings.audio.follows_other"), func() {
			labels := []string{i18n.T("settings.audio.system_default")}
			ids := []string{""}
			for _, d := range a.set.devices {
				labels, ids = append(labels, d.Name), append(ids, d.ID)
			}
			sel := labels[0]
			for i, id := range ids {
				if a.cfg.Audio.InputDevice != nil && id == platform.NormalizeDeviceID(*a.cfg.Audio.InputDevice) {
					sel = labels[i]
				}
			}
			mu.Select(a.k(c, "audio.input_device"), &sel, labels).Label(i18n.T("settings.audio.input")).Width(240).OnChange(func() {
				for i, l := range labels {
					if l == sel {
						if ids[i] == "" {
							a.cfg.Audio.InputDevice = nil
						} else {
							a.cfg.Audio.InputDevice = ptr(ids[i])
						}
					}
				}
				a.saveConfig()
			})
		})
		if len(a.set.devices) == 0 {
			mu.Text(c, i18n.T("settings.audio.no_microphone", "settings", i18n.TOS("os.sound_settings", nil))).FontSize(12.5).TextColor(p.Graphite).Padding(0, 0, 10, 0)
		}
		a.toggleRow(c, p, "audio.pause_media", i18n.T("settings.audio.pause_media"), i18n.T("settings.audio.pause_media_detail"), &a.cfg.Audio.PauseMedia, a.saveConfig)
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
					mu.Checkbox(a.k(c, ev.key), ev.v, ev.label).Disabled(!snd.Enabled).OnChange(a.saveConfig)
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
		a.row(c, p, i18n.T("settings.overlay.style"), i18n.T("settings.overlay.style_detail"), func() {
			chosen := 0
			if notch {
				chosen = 1
			}
			if i := a.choice(c, p, []string{"overlay.pill", "overlay.notch"},
				[]string{i18n.T("settings.overlay.pill"), i18n.T("settings.overlay.style_notch")}, chosen, false); i >= 0 {
				o.Style = []config.OverlayStyle{config.OverlayPill, config.OverlayNotch}[i]
				a.saveConfig()
				a.syncOverlay()
			}
		})
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
	})
	a.group(c, p, i18n.T("settings.overlay.while_dictating"), func() {
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

func (a *App) overlayChanged() {
	a.saveConfig()
	a.syncOverlay()
}

// ---- Appearance ----

func (a *App) appearanceSection(c *mu.Context, p theme.Colors) {
	ap := &a.cfg.Appearance
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
	a.group(c, p, i18n.T("settings.appearance.ink"), func() {
		mu.Text(c, i18n.T("settings.appearance.ink_detail")).FontSize(12.5).TextColor(p.Graphite).Padding(10, 0, 0, 0)
		mu.Row(c).Gap(10).Padding(10, 0).Wrap().Children(func() {
			for _, ink := range inkPresets {
				named := config.NamedInk(ink)
				on := !ap.Ink.IsCustom && ap.Ink.Named == named
				b := mu.ButtonBase(a.k(c, "appearance.ink."+string(ink))).Role(mu.RoleRadio).Checked(on).Label(ink.Name()).
					Padding(6, 10).Gap(6).Radius(99).AlignItems(mu.Center).Border(1, p.Rule).Cursor(mu.CursorPointer)
				if on {
					b.Border(2, p.Accent)
				}
				b.Children(func() {
					mu.Box(c).Size(14, 14).Radius(99).Background(theme.UIColor(ink.Color()))
					mu.Text(c, i18n.T("core.ink."+string(ink))).FontSize(13).TextColor(p.Ink)
				})
				if clicked(c, b) {
					a.SetInk(named)
				}
			}
		})
		a.row(c, p, i18n.T("settings.appearance.custom_ink"), i18n.T("settings.appearance.hex_hint"), func() {
			in := a.field(c, p, "appearance.ink.custom", i18n.T("settings.appearance.custom_ink"), "#2D3C8C", &a.set.customInk).Width(120)
			in.OnSubmit(a.applyCustomInk)
			if a.btn(c, p, "appearance.ink.custom_apply", i18n.T("settings.appearance.custom"), btnSecondary) {
				a.applyCustomInk()
			}
		})
		if a.set.inkErr != "" {
			mu.Text(c, a.set.inkErr).FontSize(12.5).TextColor(p.Danger).Padding(0, 0, 10, 0)
		}
		a.inkSpecimen(c, p)
	})
}

func (a *App) applyCustomInk() {
	if a.SetCustomInk(a.set.customInk) {
		a.set.inkErr = ""
	} else {
		a.set.inkErr = i18n.T("settings.appearance.hex_error")
	}
}

// inkSpecimen shows the ink on paper with its contrast.
func (a *App) inkSpecimen(c *mu.Context, p theme.Colors) {
	ti, err := theme.ParseInk(a.cfg.Appearance.Ink.String())
	if err != nil {
		return
	}
	rep := theme.NewContrastReport(ti)
	mu.Column(c).Gap(6).Padding(14, 16).Margin(0, 0, 12, 0).Radius(10).Background(p.Sheet).Children(func() {
		mu.Text(c, i18n.T("settings.appearance.specimen_title")).Font(theme.Display).FontSize(22).TextColor(p.Ink)
		mu.Text(c, i18n.T("settings.appearance.specimen_body")).FontSize(13).TextColor(p.Graphite)
		ratio := i18n.Decimal(min(rep.LightRatio, rep.DarkRatio), 1)
		if rep.Passes() {
			mu.Text(c, i18n.T("settings.appearance.specimen_pass", "ratio", ratio)).FontSize(12).TextColor(p.Success)
		} else {
			mu.Text(c, i18n.T("settings.appearance.specimen_fail", "ratio", ratio)).FontSize(12).TextColor(p.Danger)
		}
	})
}

// ---- History and privacy ----

func (a *App) historySection(c *mu.Context, p theme.Colors) {
	h := &a.cfg.History
	days := []*uint32{nil, ptr(uint32(1)), ptr(uint32(7)), ptr(uint32(30)), ptr(uint32(90)), ptr(uint32(365))}
	dayLabel := func(v *uint32) string {
		if v == nil {
			return i18n.T("settings.history.forever")
		}
		return i18n.TP("settings.history.days", int(*v), nil)
	}
	daySelect := func(key, label string, dst **uint32) {
		labels := make([]string, len(days))
		sel := labels[0]
		for i, d := range days {
			labels[i] = dayLabel(d)
			if (d == nil && *dst == nil) || (d != nil && *dst != nil && *d == **dst) {
				sel = labels[i]
			}
		}
		if sel == "" {
			sel = labels[0]
		}
		mu.Select(a.k(c, key), &sel, labels).Label(label).Width(160).OnChange(func() {
			for i, l := range labels {
				if l == sel {
					*dst = days[i]
				}
			}
			a.saveConfig()
			a.applyRetention()
		})
	}
	a.group(c, p, i18n.T("settings.history.history"), func() {
		detail := i18n.T("settings.history.save_detail")
		if !h.Enabled {
			detail = i18n.T("settings.history.save_detail_off")
		}
		a.toggleRow(c, p, "history_settings.enabled", i18n.T("settings.history.save"), detail, &h.Enabled, a.saveConfig)
		a.row(c, p, i18n.T("settings.history.keep_text"), i18n.T("settings.history.keep_text_detail"), func() {
			daySelect("history_settings.keep_text", i18n.T("settings.history.keep_text"), &h.KeepTextDays)
		})
		a.toggleRow(c, p, "history_settings.save_audio", i18n.T("settings.history.save_audio"), i18n.T("settings.history.save_audio_detail"), &h.SaveAudio, a.saveConfig)
		a.row(c, p, i18n.T("settings.history.keep_audio"), i18n.T("settings.history.keep_audio_detail"), func() {
			daySelect("history_settings.keep_audio", i18n.T("settings.history.keep_audio"), &h.KeepAudioDays)
		})
		a.toggleRow(c, p, "history_settings.site_icons", i18n.T("settings.history.site_icons"), i18n.T("settings.history.site_icons_detail"), &h.SiteIcons, a.saveConfig)
	})
	a.group(c, p, i18n.T("settings.history.data", "computer", computer()), func() {
		n := 0
		if a.d.Store != nil {
			n, _ = a.d.Store.Count(store.HistoryQuery{})
		}
		line := i18n.T("settings.history.entries_none")
		if n > 0 {
			line = i18n.TP("settings.history.entries", n, map[string]any{"number": n})
		}
		a.row(c, p, i18n.T("settings.history.data_history"), line, func() {
			if a.btn(c, p, "history_settings.clear", i18n.T("settings.history.clear_history"), btnDanger) {
				a.set.confirmClear = true
			}
		})
		a.row(c, p, i18n.T("settings.history.data_audio"), a.d.Paths.AudioDir(), func() {
			if a.btn(c, p, "history_settings.show_audio", i18n.TOS("settings.history.show", nil), btnGhost) {
				a.openPath(a.d.Paths.AudioDir())
			}
		})
	})
	a.group(c, p, i18n.T("settings.history.leaves", "computer", computer()), func() {
		mu.Text(c, i18n.T("settings.history.leaves_local", "computer", computer())).FontSize(13).TextColor(p.Graphite).Padding(12, 0)
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
	mu.Box(c).Absolute().Top(0).Left(0).Right(0).Bottom(0).Background(p.Shadow.Alpha(0.25)).ZIndex(50).
		Justify(mu.Center).AlignItems(mu.Center).Children(func() {
		mu.Column(c).Width(420).Padding(24).Gap(14).Radius(14).Background(p.SheetRaised).Border(1, p.Rule).Role(mu.RoleAlertDialog).
			Label(i18n.T("settings.history.clear_history")).Shadow(0, 16, 40, 0, p.Shadow.Alpha(0.25)).Children(func() {
			heading(c, p, i18n.T("settings.history.clear_history"), 22)
			mu.Text(c, i18n.TP("settings.history.entries_confirm", n, map[string]any{"number": n})).FontSize(13.5).TextColor(p.Graphite)
			mu.Row(c).Gap(8).Justify(mu.End).Children(func() {
				if a.btn(c, p, "history_settings.clear_cancel", i18n.T("settings.history.cancel"), btnSecondary) {
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
			a.field(c, p, "advanced.engine_path", i18n.T("settings.advanced.engine_path"), "SaysoEngine", &a.set.enginePath).Width(280).OnChange(func() {
				if v := strings.TrimSpace(a.set.enginePath); v == "" {
					a.cfg.Advanced.EnginePath = nil
				} else {
					a.cfg.Advanced.EnginePath = ptr(v)
				}
				a.saveConfig()
			})
		})
		if v := strings.TrimSpace(a.set.enginePath); v != "" {
			if _, err := os.Stat(v); err != nil {
				mu.Text(c, i18n.T("settings.advanced.engine_missing")).FontSize(12.5).TextColor(p.Danger).Padding(0, 0, 10, 0)
			} else {
				mu.Text(c, i18n.T("settings.advanced.engine_saved")).FontSize(12.5).TextColor(p.Graphite).Padding(0, 0, 10, 0)
			}
		}
	})
	a.group(c, p, i18n.T("settings.advanced.logs"), func() {
		a.row(c, p, i18n.T("settings.advanced.log_level"), i18n.T("settings.advanced.log_level_detail"), func() {
			levels := []string{"error", "warn", "info", "debug", "trace"}
			sel := derefOr(a.cfg.Advanced.LogLevel, "info")
			mu.Select(a.k(c, "advanced.log_level"), &sel, levels).Label(i18n.T("settings.advanced.log_level")).Width(140).OnChange(func() {
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
