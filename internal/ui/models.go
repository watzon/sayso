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
	query string
	sort  int
	err   string
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

func (a *App) modelsPage(c *mu.Context, p theme.Colors) {
	pageHeader(c, p, i18n.T("models.page.title"), i18n.TOS("models.page.intro", nil))
	if !a.engineRunning() {
		mu.Text(c, i18n.T("models.page.engine_down")).FontSize(13).TextColor(p.Danger).Padding(10, 14).Radius(10).
			Background(p.DangerWash).Role(mu.RoleAlert)
	}
	if a.mdl.err != "" {
		mu.Text(c, a.mdl.err).FontSize(13).TextColor(p.Danger)
	}
	a.modelSlots(c, p)
	mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
		mu.SearchField(a.k(c, "models.search"), &a.mdl.query).Label(i18n.T("models.search.placeholder")).
			Placeholder(i18n.T("models.search.placeholder")).Width(260)
		mu.Spacer(c)
		keys := []string{"models.sort.recommended", "models.sort.accuracy", "models.sort.speed", "models.sort.size"}
		labels := []string{i18n.T("models.sort.recommended"), i18n.T("models.group.accuracy"), i18n.T("models.group.speed"), i18n.T("models.group.size")}
		if i := a.choice(c, p, keys, labels, a.mdl.sort, false); i >= 0 {
			a.mdl.sort = i
		}
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
		mu.Text(c, i18n.T("models.search.empty", "query", a.mdl.query)).FontSize(13).TextColor(p.Graphite)
	}
	a.modelGroup(c, p, i18n.T("models.group.non_streaming"), i18n.T("models.group.non_streaming_note"), batch)
	a.modelGroup(c, p, i18n.T("models.group.streaming"), i18n.T("models.group.streaming_note"), stream)
	if a.d.Apps != nil && a.d.Apps.Current() != nil && a.d.Apps.Current().AnyApproved() {
		a.modelGroup(c, p, i18n.T("models.roles.vad_title"), i18n.T("models.roles.vad_note"), a.modelList(models.TaskVad))
		a.modelGroup(c, p, i18n.T("models.roles.speakers_title"), i18n.T("models.roles.speakers_note"), a.modelList(models.TaskDiarization))
	}
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		mu.Text(c, a.d.Paths.ModelsDir()).Font(theme.Mono).FontSize(12).TextColor(p.Pencil).SingleLine().TruncateMiddle().Grow(1)
		if a.btn(c, p, "models.folder", i18n.TOS("settings.history.show", nil), btnGhost) {
			a.openPath(a.d.Paths.ModelsDir())
		}
	})
}

// modelSlots are the Final text and Live preview cards.
func (a *App) modelSlots(c *mu.Context, p theme.Colors) {
	mu.Row(c).Gap(14).Children(func() {
		final := a.FinalModel()
		slot := func(key, title string, id models.ModelID, detail string) {
			card(c, p).Grow(1).Basis(0).Children(func() {
				eyebrow(c, p, title)
				mu.Text(c, a.modelName(id)).Font(theme.Display).FontSize(22).FontWeight(600).TextColor(p.Ink)
				mu.Text(c, detail).FontSize(12.5).TextColor(p.Graphite)
			})
		}
		slot("models.slot.final", i18n.T("models.setup.final"), final, a.statusText(final))
		preview, detail := models.ModelID(""), i18n.T("models.setup.no_preview")
		if a.cfg.Dictation.LivePreview {
			preview = a.cfg.Dictation.PreviewModel.Model
			detail = i18n.T("models.setup.automatic")
			if preview == "" {
				if m, ok := models.Find(final); ok && m.LivePreview {
					preview, detail = final, i18n.T("models.setup.same_model")
				} else {
					preview = models.PreviewFallback()
				}
			}
		} else {
			detail = i18n.T("models.setup.off")
		}
		slot("models.slot.preview", i18n.T("models.setup.preview"), preview, detail)
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
	mu.Column(c).Gap(8).Children(func() {
		mu.Row(c).Gap(8).AlignItems(mu.Baseline).Children(func() {
			heading(c, p, title, 19)
			mu.Text(c, note).FontSize(12.5).TextColor(p.Graphite)
		})
		mu.Column(c).Radius(12).Background(p.SheetRaised).Border(1, p.Rule).Dividers(1, p.Rule).Clip().Role(mu.RoleList).Children(func() {
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
	mu.Row(c).Gap(14).Padding(12, 16).AlignItems(mu.Center).Role(mu.RoleListItem).Label(m.Name).Children(func() {
		mu.Column(c).Gap(3).Grow(1).Basis(0).Children(func() {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
				mu.Text(c, m.Name).FontSize(14.5).FontWeight(600).TextColor(p.Ink)
				if inUse {
					slip(c, p, i18n.T("models.row.in_use"), p.Accent)
				}
				if m.Recommended {
					slip(c, p, i18n.T("styles.recommended"), p.Success)
				}
				if !m.Punctuation {
					slip(c, p, i18n.T("models.row.no_punctuation"), p.Graphite)
				}
			})
			if m.DescriptionKey != "" {
				mu.Text(c, i18n.T(m.DescriptionKey)).FontSize(12.5).TextColor(p.Graphite)
			}
			acc := i18n.T("models.row.not_measured")
			if m.WerPercent != nil {
				acc = i18n.T("models.accuracy_percent", "value", i18n.Decimal(float64(100-*m.WerPercent), 1))
			}
			mu.Text(c, fmt.Sprintf("%s · %s · %s %s · %s", m.Vendor, models.FormatSize(m.SizeBytes),
				i18n.T("models.setup.accuracy"), acc, i18n.T("models.details.speed_of_five", "n", m.Speed))).FontSize(11.5).TextColor(p.Pencil)
			if st.State == engine.StateDownloading {
				mu.Progress(c, float64(st.Fraction)).Label(i18n.T("models.a11y.downloading", "name", m.Name)).Height(4)
			}
			if st.State == engine.StateFailed && st.Message != "" {
				mu.Text(c, st.Message).FontSize(12).TextColor(p.Danger)
			}
		})
		switch {
		case st.State == engine.StateDownloading:
			mu.Text(c, fmt.Sprintf("%d%%", int(st.Fraction*100))).Font(theme.Mono).FontSize(12).TextColor(p.Graphite)
		case st.State == engine.StateOptimizing:
			mu.Text(c, i18n.T("models.row.optimizing")).FontSize(12).TextColor(p.Graphite)
		case st.IsOnDisk():
			if !inUse && m.Task() == models.TaskSpeech && a.btn(c, p, "models.use."+id, i18n.T("models.row.use"), btnSecondary) {
				a.UseModel(m.ID)
			}
			if !inUse && a.btn(c, p, "models.delete."+id, i18n.T("models.row.delete"), btnGhost) {
				a.deleteModel(m.ID)
			}
		default:
			if a.btn(c, p, "models.download."+id, i18n.T("models.download_size", "size", models.FormatSize(m.SizeBytes)), btnSecondary) {
				a.downloadModel(m.ID)
			}
		}
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
