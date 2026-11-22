package ui

import (
	"fmt"
	"slices"
	"strings"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/theme"
)

// Model sort orders.
const (
	sortRecommended = iota
	sortAccuracy
	sortSpeed
	sortSize
)

type modelsState struct {
	query    string
	sort     int
	sortOpen bool
	err      string
	// searchField is the search well, which Change focuses.
	searchField mu.Handle
}

// modelName is the shown name of a model id.
func (a *App) modelName(id models.ModelID) string {
	if id == "" {
		return i18n.T("app.engine.no_model")
	}
	if m, ok := models.Find(id); ok {
		return m.Name
	}
	return string(id)
}

// modelList is the speech models that match the search, sorted.
func (a *App) modelList(task models.ModelTask) []models.ModelInfo {
	q := strings.ToLower(strings.TrimSpace(a.mdl.query))
	var out []models.ModelInfo
	for _, m := range models.Catalog() {
		if m.Task() != task || m.IsRemote() {
			continue
		}
		if q != "" && !strings.Contains(strings.ToLower(m.Name+" "+m.Vendor+" "+string(m.ID)), q) {
			continue
		}
		out = append(out, m)
	}
	wer := func(m models.ModelInfo) float32 {
		if m.WerPercent == nil {
			return 100
		}
		return *m.WerPercent
	}
	slices.SortStableFunc(out, func(x, y models.ModelInfo) int {
		switch a.mdl.sort {
		case sortAccuracy:
			return cmpF(wer(x), wer(y))
		case sortSpeed:
			return int(y.Speed) - int(x.Speed)
		case sortSize:
			return cmpF(float32(x.SizeBytes), float32(y.SizeBytes))
		}
		if x.Recommended != y.Recommended {
			if x.Recommended {
				return -1
			}
			return 1
		}
		return 0
	})
	return out
}

func cmpF(a, b float32) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	}
	return 0
}

// Column widths of the model rows (models.rs).
const (
	accuracyW = 176
	speedW    = 84
	sizeW     = 64
	actionW   = 132
)

func (a *App) modelsPage(c *mu.Context, p theme.Colors) {
	used := uint64(0)
	for _, m := range models.Catalog() {
		if !m.IsRemote() && a.modelStatus(m.ID).IsOnDisk() {
			used += m.SizeBytes
		}
	}
	pageHead(c, p, i18n.T("models.page.title"), i18n.TOS("models.page.intro", nil), 600, func() {
		line := i18n.T("models.page.nothing_downloaded")
		if used > 0 {
			line = i18n.T("models.page.used", "size", models.FormatSize(used))
		}
		b := mu.ButtonBase(a.k(c, "models.folder")).Label(i18n.TOS("settings.history.show", nil)).Cursor(mu.CursorPointer)
		b.Children(func() {
			uiText(c, line+" · "+tildePath(a.d.Paths.ModelsDir()), 13, 400, p.Graphite).SingleLine()
		})
		if clicked(c, b) {
			a.openPath(a.d.Paths.ModelsDir())
		}
	})
	if !a.engineRunning() {
		uiText(c, i18n.T("models.page.engine_down"), 13, 500, p.Danger).Padding(10, 14).Radius(10).
			Background(p.DangerWash).Role(mu.RoleAlert)
	}
	if a.mdl.err != "" {
		uiText(c, a.mdl.err, 13, 500, p.Danger)
	}
	a.modelSlots(c, p)
	mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
		a.searchWell(c, p, "models.search", i18n.T("models.search.placeholder"), &a.mdl.query, 34, 300, "").Bind(&a.mdl.searchField)
		mu.Spacer(c)
		a.sortMenu(c, p)
	})
	all := a.modelList(models.TaskSpeech)
	var batch, stream []models.ModelInfo
	for _, m := range all {
		if m.LivePreview {
			stream = append(stream, m)
		} else {
			batch = append(batch, m)
		}
	}
	if len(all) == 0 {
		uiText(c, i18n.T("models.search.empty", "query", a.mdl.query), 13, 400, p.Graphite)
	}
	a.modelGroup(c, p, i18n.T("models.group.non_streaming"), i18n.T("models.group.non_streaming_note"), batch)
	a.modelGroup(c, p, i18n.T("models.group.streaming"), i18n.T("models.group.streaming_note"), stream)
	if a.d.Apps != nil && a.d.Apps.Current() != nil && a.d.Apps.Current().AnyApproved() {
		a.modelGroup(c, p, i18n.T("models.roles.vad_title"), i18n.T("models.roles.vad_note"), a.modelList(models.TaskVad))
		a.modelGroup(c, p, i18n.T("models.roles.speakers_title"), i18n.T("models.roles.speakers_note"), a.modelList(models.TaskDiarization))
	}
}

// sortMenu is "Sort: Recommended", which opens the four orders.
func (a *App) sortMenu(c *mu.Context, p theme.Colors) {
	labels := []string{i18n.T("models.sort.recommended"), i18n.T("models.group.accuracy"), i18n.T("models.group.speed"), i18n.T("models.group.size")}
	keys := []string{"models.sort.recommended", "models.sort.accuracy", "models.sort.speed", "models.sort.size"}
	label := i18n.T("models.sort.button", "key", labels[a.mdl.sort])
	b := mu.ButtonBase(a.k(c, "models.sort")).Label(label).Height(28).PaddingX(12).Radius(8).AlignItems(mu.Center).
		HasPopup(mu.PopupMenu).Cursor(mu.CursorPointer)
	raisedSmall(b, p)
	if b.Hovered() {
		b.Background(p.Sheet)
	}
	b.Children(func() { uiText(c, label, 13, 500, p.Ink).SingleLine() })
	if clicked(c, b) {
		a.mdl.sortOpen = !a.mdl.sortOpen
	}
	mu.PopoverBase(c, b, &a.mdl.sortOpen, func(panel mu.Element) {
		floating(panel.Margin(4, 0, 0, 0).MinWidth(200).Padding(4).Radius(10).Role(mu.RoleMenu), p)
		mu.Column(c).Children(func() {
			for i, l := range labels {
				it := mu.ButtonBase(a.k(c, keys[i])).Label(l).Role(mu.RoleMenuItemRadio).Checked(i == a.mdl.sort).
					Height(30).PaddingX(10).Gap(8).Radius(7).AlignItems(mu.Center).Cursor(mu.CursorPointer)
				if it.Hovered() {
					it.Background(p.Deboss)
				}
				it.Children(func() {
					mu.Box(c).Width(14).Shrink(0).Children(func() {
						if i == a.mdl.sort {
							mu.Icon(c, icon("check")).FontSize(12).TextColor(p.Ink)
						}
					})
					uiText(c, l, 13, 500, p.Ink)
				})
				if clicked(c, it) {
					a.mdl.sort, a.mdl.sortOpen = i, false
				}
			}
		})
	})
}

// languageLabel is "English" or "25 languages".
func languageLabel(m models.ModelInfo) string {
	switch {
	case m.IsRemote():
		return i18n.T("core.models.many_languages")
	case len(m.Languages) == 1:
		return i18n.LanguageName(m.Languages[0])
	}
	return i18n.TP("core.models.languages", len(m.Languages), nil)
}

// meter is five bars, level of them in ink.
func meter(c *mu.Context, p theme.Colors, level int) {
	mu.Row(c).Gap(3).Shrink(0).AlignItems(mu.Center).Children(func() {
		for i := range 5 {
			col := p.DebossShade
			if i < level {
				col = p.Ink
			}
			mu.Box(c).Size(14, 6).Radius(2).Background(col)
		}
	})
}

// accuracyLevel is the meter level of a word error rate (models.rs).
func accuracyLevel(wer float32) int {
	switch {
	case wer < 6.5:
		return 5
	case wer < 7:
		return 4
	case wer < 8.5:
		return 3
	case wer < 10:
		return 2
	}
	return 1
}

func accuracyLabel(wer float32) string {
	return i18n.T("models.accuracy_percent", "value", i18n.Decimal(float64(100-wer), 1))
}

// previewModel is the model of the live preview: the one chosen, else the
// final model when it can preview, else the fallback. same is true when it
// is the final model.
func (a *App) previewModel() (id models.ModelID, same bool) {
	if id = a.cfg.Dictation.PreviewModel.Model; id != "" {
		return id, false
	}
	final := a.FinalModel()
	if m, ok := models.Find(final); ok && m.LivePreview {
		return final, true
	}
	return models.PreviewFallback(), false
}

// modelSlots is one card: Final text and Live preview, split by a rule.
func (a *App) modelSlots(c *mu.Context, p theme.Colors) {
	final := a.FinalModel()
	preview, previewSame, previewOn := models.ModelID(""), false, a.cfg.Dictation.LivePreview
	if previewOn {
		preview, previewSame = a.previewModel()
	}
	stat := func(figure, note string) {
		mu.Column(c).Gap(0).Children(func() {
			heading(c, p, figure, 22).SingleLine()
			uiText(c, note, 12, 400, p.Graphite).LineHeight(16.0 / 12).SingleLine()
		})
	}
	slot := func(key, title string, id models.ModelID, body func(m models.ModelInfo, st engine.ModelStatus)) {
		m, _ := models.Find(id)
		st := a.modelStatus(id)
		dot, state := p.Success, ""
		switch st.State {
		case engine.StateReady, engine.StateDownloaded:
		case engine.StateDownloading:
			dot, state = p.Accent, fmt.Sprintf("%d%%", int(st.Fraction*100))
		case engine.StateOptimizing:
			dot, state = p.Accent, i18n.T("models.row.optimizing")
		case engine.StateFailed:
			dot, state = p.Danger, i18n.T("models.hero.model_error")
		default:
			dot, state = p.Danger, i18n.T("models.hero.not_downloaded")
		}
		if title == i18n.T("models.setup.preview") && !previewOn {
			dot, state = p.Pencil, ""
		}
		if state != "" {
			title += " · " + state
		}
		grow := float32(1)
		if key == "models.slot.final" {
			grow = 1.25 // models.rs: the final slot takes more room
		}
		mu.Column(c).Grow(grow).Basis(0).MinWidth(0).Padding(22, 28).Gap(6).Children(func() {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
				statusDot(c, dot)
				caps(c, p, title)
			})
			body(m, st)
		})
	}
	raised(mu.Row(c).Radius(16).AlignItems(mu.Stretch), p).Children(func() {
		slot("models.slot.final", i18n.T("models.setup.final"), final, func(m models.ModelInfo, st engine.ModelStatus) {
			heading(c, p, a.modelName(final), 30).LineHeight(34.0 / 30).SingleLine().Ellipsis("…")
			uiText(c, m.Vendor+" · "+languageLabel(m), 14, 400, p.Graphite)
			mu.Row(c).Gap(24).Padding(14, 0, 0, 0).AlignItems(mu.End).Children(func() {
				if m.WerPercent != nil {
					stat(accuracyLabel(*m.WerPercent), i18n.T("models.setup.accuracy"))
				}
				if m.Rtfx != nil {
					stat(fmt.Sprintf("%d×", *m.Rtfx), i18n.T("models.setup.faster"))
				} else {
					stat(fmt.Sprintf("%d/5", m.Speed), i18n.T("models.hero.speed"))
				}
				onDisk := i18n.T("models.setup.on_disk")
				if !st.IsOnDisk() {
					onDisk = i18n.T("models.hero.to_download")
				}
				stat(models.FormatSize(m.SizeBytes), onDisk)
				mu.Spacer(c)
				if a.button(c, p, "models.change_final", i18n.T("models.setup.change"), bopt{size: sizeSmall}) {
					a.mdl.searchField.Focus(c)
				}
			})
		})
		mu.Box(c).Width(1).Margin(22, 0).Background(p.Rule)
		slot("models.slot.preview", i18n.T("models.setup.preview"), preview, func(m models.ModelInfo, st engine.ModelStatus) {
			switch {
			case !previewOn:
				heading(c, p, i18n.T("models.setup.no_preview"), 30).Italic().FontWeight(500).TextColor(p.Graphite).LineHeight(34.0 / 30)
			case previewSame:
				heading(c, p, i18n.T("models.setup.same_model"), 30).Italic().FontWeight(500).TextColor(p.Graphite).LineHeight(34.0 / 30)
			default:
				heading(c, p, a.modelName(preview), 30).LineHeight(34.0 / 30).SingleLine().Ellipsis("…")
			}
			if previewOn && !previewSame {
				uiText(c, m.Vendor+" · "+languageLabel(m), 14, 400, p.Graphite)
			} else {
				mu.Box(c).Height(20)
			}
			mu.Row(c).Gap(24).Padding(14, 0, 0, 0).AlignItems(mu.End).Children(func() {
				if previewOn {
					if w := m.PreviewWerPercent; w != nil {
						stat(accuracyLabel(*w), i18n.T("models.setup.accuracy"))
					} else if w := m.WerPercent; w != nil {
						stat(accuracyLabel(*w), i18n.T("models.setup.accuracy"))
					}
					if ms, ok := m.PreviewIntervalMs(); ok {
						stat(i18n.T("models.hero.ms", "ms", ms), i18n.T("models.setup.between_updates"))
					}
				}
				mu.Spacer(c)
				if a.button(c, p, "models.change_preview", i18n.T("models.setup.change"), bopt{size: sizeSmall}) {
					a.OpenSettings(SecDictation)
				}
			})
		})
	})
}

// statusText is a model's state in words.
func (a *App) statusText(id models.ModelID) string {
	st := a.modelStatus(id)
	switch st.State {
	case engine.StateDownloading:
		return i18n.T("app.engine.detail_downloading", "model", a.modelName(id), "percent", int(st.Fraction*100))
	case engine.StateOptimizing:
		return i18n.T("app.engine.optimizing")
	case engine.StateReady:
		return i18n.T("app.engine.ready")
	case engine.StateDownloaded:
		return i18n.T("app.engine.detail_not_in_memory", "model", a.modelName(id))
	case engine.StateFailed:
		return i18n.T("app.engine.model_error")
	}
	return i18n.T("models.hero.not_downloaded")
}

func (a *App) modelGroup(c *mu.Context, p theme.Colors, title, note string, list []models.ModelInfo) {
	if len(list) == 0 {
		return
	}
	mu.Column(c).Children(func() {
		mu.Row(c).Gap(16).Padding(0, 0, 10, 0).AlignItems(mu.End).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
			mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
				heading(c, p, title, 20).LineHeight(26.0 / 20).Role(mu.RoleHeading)
				uiText(c, note, 13, 400, p.Graphite).LineHeight(18.0 / 13)
			})
			caps(c, p, i18n.T("models.group.accuracy")).Width(accuracyW).Shrink(0)
			caps(c, p, i18n.T("models.group.speed")).Width(speedW).Shrink(0)
			caps(c, p, i18n.T("models.group.size")).Width(sizeW).Shrink(0)
			mu.Box(c).Width(actionW).Shrink(0)
		})
		mu.Column(c).Role(mu.RoleList).Children(func() {
			for _, m := range list {
				a.modelRow(c, p, m)
			}
		})
	})
}

func (a *App) modelRow(c *mu.Context, p theme.Colors, m models.ModelInfo) {
	st := a.modelStatus(m.ID)
	inUse := a.FinalModel() == m.ID
	id := string(m.ID)
	mu.Row(c).MinHeight(66).Gap(16).PaddingY(10).AlignItems(mu.Center).Role(mu.RoleListItem).Label(m.Name).
		BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		mu.Column(c).Gap(2).Grow(1).Basis(0).MinWidth(0).Children(func() {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
				heading(c, p, m.Name, 17).LineHeight(22.0 / 17).SingleLine().Ellipsis("…")
				if inUse {
					badge(c, p, i18n.T("models.row.in_use"), toneInk, "")
				}
			})
			uiText(c, m.Vendor+" · "+languageLabel(m), 13, 400, p.Graphite).SingleLine()
			if st.State == engine.StateDownloading {
				mu.Progress(c, float64(st.Fraction)).Label(i18n.T("models.a11y.downloading", "name", m.Name)).Height(4).MaxWidth(240)
			}
			if st.State == engine.StateFailed && st.Message != "" {
				uiText(c, st.Message, 12, 400, p.Danger)
			}
		})
		mu.Row(c).Width(accuracyW).Shrink(0).Gap(12).AlignItems(mu.Center).Children(func() {
			if m.WerPercent == nil {
				uiText(c, i18n.T("models.row.not_measured"), 13, 400, p.Graphite)
				return
			}
			meter(c, p, accuracyLevel(*m.WerPercent))
			uiText(c, accuracyLabel(*m.WerPercent), 13, 400, p.Graphite)
		})
		mu.Row(c).Width(speedW).Shrink(0).Label(i18n.T("models.a11y.speed", "n", m.Speed)).Children(func() { meter(c, p, int(m.Speed)) })
		mono(c, models.FormatSize(m.SizeBytes), 12, p.Graphite).Width(sizeW).Shrink(0)
		mu.Row(c).Width(actionW).Shrink(0).Gap(6).Justify(mu.End).AlignItems(mu.Center).Children(func() {
			switch {
			case st.State == engine.StateDownloading:
				mono(c, fmt.Sprintf("%d%%", int(st.Fraction*100)), 12, p.Graphite)
			case st.State == engine.StateOptimizing:
				uiText(c, i18n.T("models.row.optimizing"), 12, 500, p.Graphite)
			case st.IsOnDisk():
				if !inUse && a.button(c, p, "models.delete."+id, i18n.T("models.row.delete"), bopt{kind: btnGhost, size: sizeSmall}) {
					a.deleteModel(m.ID)
				}
				if !inUse && m.Task() == models.TaskSpeech && a.button(c, p, "models.use."+id, i18n.T("models.row.use"), bopt{size: sizeSmall}) {
					a.UseModel(m.ID)
				}
			default:
				// The size is in the SIZE column; screen readers hear it here.
				b := mu.ButtonBase(a.k(c, "models.download."+id)).Label(i18n.T("models.download_size", "size", models.FormatSize(m.SizeBytes))).
					Height(28).PaddingX(12).Radius(8).AlignItems(mu.Center).Cursor(mu.CursorPointer)
				raisedSmall(b, p)
				if b.Hovered() {
					b.Background(p.Sheet)
				}
				b.Children(func() { uiText(c, i18n.T("models.download"), 13, 500, p.Ink) })
				if clicked(c, b) {
					a.downloadModel(m.ID)
				}
			}
		})
	})
}

// UseModel makes a model the final model.
func (a *App) UseModel(id models.ModelID) {
	a.cfg.Dictation.Model = id
	a.saveConfig()
	if a.d.Engine != nil {
		eng := a.d.Engine
		go eng.Load(string(id))
	}
	a.invalidate()
}

func (a *App) downloadModel(id models.ModelID) {
	if a.d.Engine == nil {
		a.mdl.err = i18n.T("onboarding.model.no_engine")
		return
	}
	eng := a.d.Engine
	go func() {
		if err := eng.Download(string(id)); err != nil {
			a.post(func() { a.mdl.err = err.Error(); a.invalidate() })
		}
	}()
}

func (a *App) deleteModel(id models.ModelID) {
	if a.d.Engine == nil {
		return
	}
	eng := a.d.Engine
	go func() {
		if err := eng.Delete(string(id)); err != nil {
			a.post(func() { a.mdl.err = err.Error(); a.invalidate() })
		}
		a.post(a.invalidate)
	}()
}
