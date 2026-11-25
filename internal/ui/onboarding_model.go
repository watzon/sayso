package ui

import (
	"math"
	"runtime"
	"slices"
	"strings"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/models/languages"
	"github.com/watzon/sayso/internal/theme"
)

// Step 2: choose a speech model. Choosing one starts its download, and the
// step does not continue until a model is on disk. It offers three models,
// and a list of all local models.

// modelRunsHere is false for a model this computer cannot run. The Go port
// does not look for a GPU yet, so a model that needs one never shows.
func modelRunsHere(m models.ModelInfo) bool { return !m.Engine.NeedsGPU() }

// suggestedModel is the model to suggest for a dictation language.
func suggestedModel(language string) models.ModelID {
	return models.SuggestedFor(language, modelRunsHere)
}

// modelChoices are the three models onboarding offers: the suggested model
// for the language, a GPU model that understands it when one runs here,
// then the best models for English, for European languages and for every
// other language, first those that understand the language.
func modelChoices(language string) []models.ModelID {
	return modelChoicesWith(language, modelRunsHere)
}

func modelChoicesWith(language string, runsHere func(models.ModelInfo) bool) []models.ModelID {
	english, european := models.ModelID("parakeet-tdt-v2"), models.ModelID("parakeet-tdt-v3")
	if runtime.GOOS == "darwin" {
		english, european = "parakeet-unified-en", "parakeet-ultra"
	}
	rest := []models.ModelID{english, european, "whisper-large-v3-turbo"}
	var gpu models.ModelID
	for _, m := range models.Catalog() {
		if m.Engine.NeedsGPU() && m.Understands(language) && runsHere(m) {
			gpu = m.ID
			break
		}
	}
	understands := func(id models.ModelID) bool {
		m, ok := models.Find(id)
		return ok && m.Understands(language)
	}
	return orderChoices(models.SuggestedFor(language, runsHere), gpu, rest, understands)
}

// orderChoices is suggested, then gpu, then the models of rest that
// understand the language, then the others, each group in the order of
// rest, without repeats: three models at most.
func orderChoices(suggested, gpu models.ModelID, rest []models.ModelID, understands func(models.ModelID) bool) []models.ModelID {
	rest = slices.Clone(rest)
	slices.SortStableFunc(rest, func(x, y models.ModelID) int {
		ux, uy := understands(x), understands(y)
		switch {
		case ux && !uy:
			return -1
		case uy && !ux:
			return 1
		}
		return 0
	})
	list := []models.ModelID{suggested}
	cands := rest
	if gpu != "" {
		cands = append([]models.ModelID{gpu}, rest...)
	}
	for _, id := range cands {
		if !slices.Contains(list, id) {
			list = append(list, id)
		}
	}
	return list[:min(3, len(list))]
}

// localModels are the models the user can make active here: local, and
// able to do the final pass.
func localModels() []models.ModelInfo {
	var out []models.ModelInfo
	for _, m := range models.Catalog() {
		if !m.IsRemote() && m.FinalPass && m.Task() == models.TaskSpeech && modelRunsHere(m) {
			out = append(out, m)
		}
	}
	return out
}

// modelMatches is true when the name, the vendor, the description or a
// language of the model contains query.
func modelMatches(m models.ModelInfo, query string) bool {
	q := strings.ToLower(strings.TrimSpace(query))
	if q == "" {
		return true
	}
	text := m.Name + " " + m.Vendor + " " + modelDescription(m)
	for _, l := range m.Languages {
		text += " " + i18n.LanguageName(l)
	}
	return strings.Contains(strings.ToLower(text), q)
}

func modelDescription(m models.ModelInfo) string {
	if m.DescriptionKey == "" {
		return ""
	}
	return i18n.T(m.DescriptionKey)
}

// chooseModel makes a model the final model and starts its download.
func (a *App) chooseModel(id models.ModelID) {
	a.UseModel(id)
	a.ensureDownload()
}

// ensureDownload starts the final model's download when it is not on disk
// and not downloading (or failed).
func (a *App) ensureDownload() {
	id := a.FinalModel()
	switch a.modelStatus(id).State {
	case engine.StateReady, engine.StateDownloaded, engine.StateOptimizing, engine.StateDownloading:
		return
	}
	a.downloadModel(id)
}

func (a *App) openAllModels() {
	a.onb.allModels = true
	a.onb.allQuery = ""
	a.invalidate()
}

func (a *App) closeAllModels() {
	a.onb.allModels = false
	a.onb.allQuery = ""
	a.invalidate()
}

// downloadClock keeps the bytes seen in the last seconds of a download,
// for the time left.
type downloadClock struct {
	model   models.ModelID
	samples []dlSample
}

type dlSample struct {
	at   time.Time
	done uint64
}

// left is "about 40 s left", from the bytes seen in the last 8 seconds.
func (d *downloadClock) left(id models.ModelID, now time.Time, done, total uint64) string {
	if d.model != id {
		*d = downloadClock{model: id}
	}
	if n := len(d.samples); n == 0 || d.samples[n-1].done != done {
		d.samples = append(d.samples, dlSample{now, done})
	}
	for len(d.samples) > 2 && now.Sub(d.samples[0].at) > 8*time.Second {
		d.samples = d.samples[1:]
	}
	first := d.samples[0]
	secs := now.Sub(first.at).Seconds()
	if secs < 1 || done <= first.done || total <= done {
		return ""
	}
	rate := float64(done-first.done) / secs
	left := float64(total-done) / rate
	if left < 90 {
		return i18n.T("onboarding.model.seconds_left", "seconds", int(max(1, math.Ceil(left))))
	}
	return i18n.T("onboarding.model.minutes_left", "minutes", int(math.Ceil(left/60)))
}

func (a *App) onbModel(c *mu.Context, p theme.Colors) {
	active := a.FinalModel()
	choices := modelChoices(a.cfg.Dictation.Language)
	stepColumn(c, p, i18n.T("onboarding.model.title"), i18n.T("onboarding.model.lead"), func() {
		mu.Column(c).Gap(14).Children(func() {
			// One choice of a set: the cards are radio buttons.
			mu.Row(a.k(c, "onboarding.model.choices")).Gap(14).AlignItems(mu.Stretch).Role(mu.RoleRadioGroup).
				Label(i18n.T("onboarding.model.title")).Children(func() {
				for i, id := range choices {
					if m, ok := models.Find(id); ok {
						a.modelCard(c, p, m, i == 0, choices, active, i)
					}
				}
			})
			if slices.Contains(choices, active) {
				mu.Row(c).Gap(6).AlignItems(mu.Center).Children(func() {
					uiText(c, i18n.T("onboarding.model.different_model"), 13, 400, p.Graphite)
					if a.link(c, p, "onboarding.model.show_all", i18n.TP("onboarding.model.show_all", len(localModels()), nil), 13, 500, "") {
						a.openAllModels()
					}
				})
			} else {
				m, _ := models.Find(active)
				ring(raised(mu.Row(c).Gap(12).Padding(12, 18).Radius(14).AlignItems(mu.Center), p), p).Children(func() {
					radioDot(c, p, true)
					cardTitle(c, p, m.Name, 16, 20).SingleLine()
					meta := sizeLabel(m) + " · " + languageLabel(m)
					mu.Text(c, meta).Font(monoFont(meta)).FontSize(12).TextColor(p.Ink).SingleLine().Grow(1).Basis(0)
					b := mu.ButtonBase(a.k(c, "onboarding.model.change")).Label(i18n.T("onboarding.model.change_label")).Role(mu.RoleLink).Cursor(mu.CursorPointer)
					b.Children(func() { uiText(c, i18n.T("onboarding.model.change"), 13, 500, p.Accent) })
					if clicked(c, b) {
						a.openAllModels()
					}
				})
			}
		})
		a.downloadStrip(c, p, active)
	})
}

// modelCard is one of the three models: a radio button.
func (a *App) modelCard(c *mu.Context, p theme.Colors, m models.ModelInfo, recommended bool, choices []models.ModelID, active models.ModelID, i int) {
	selected, n := m.ID == active, len(choices)
	b := mu.ButtonBase(a.k(c, "onboarding.model."+string(m.ID))).Label(m.Name).Role(mu.RoleRadio).Checked(selected).
		PositionInSet(i+1, n).Description(modelDescription(m) + " " + sizeText(m.SizeBytes) + " · " + languageLabel(m)).
		Column().Grow(1).Basis(0).MinWidth(0).Gap(10).Padding(18).Radius(14).Justify(mu.Start).AlignItems(mu.Stretch).Cursor(mu.CursorPointer)
	a.radioArrows(b, i, slices.Index(choices, active), n,
		func(j int) string { return "model:" + string(choices[j]) },
		func(j int) { a.chooseModel(choices[j]) })
	raised(b, p)
	if selected {
		ring(b, p)
	} else if b.Hovered() {
		b.Background(p.Sheet)
	}
	b.Children(func() {
		mu.Row(c).Height(22).AlignItems(mu.Center).Children(func() {
			radioDot(c, p, selected)
			mu.Spacer(c)
			if recommended {
				badge(c, p, i18n.T("onboarding.model.recommended"), toneAccent, "")
			}
		})
		cardTitle(c, p, m.Name, 20, 24)
		uiText(c, modelDescription(m), 13, 400, p.Graphite).FixedLineHeight(19)
		meta := sizeText(m.SizeBytes) + " · " + languageLabel(m)
		mu.Text(c, meta).Font(monoFont(meta)).FontSize(12).TextColor(p.Ink).Padding(4, 0, 0, 0)
	})
	if clicked(c, b) {
		a.chooseModel(m.ID)
	}
}

// track is a progress track: the darker deboss shade with an ink fill.
func track(c *mu.Context, p theme.Colors, key string, fraction float32, height float32, label string) mu.Element {
	return trackIn(c, p, key, fraction, height, label, p.InkFill)
}

// trackIn is a progress track filled with fill.
func trackIn(c *mu.Context, p theme.Colors, key string, fraction float32, height float32, label string, fill mu.Color) mu.Element {
	f := max(0, min(fraction, 1))
	return mu.Box(a11yKey(c, key)).Height(height).Radius(99).Background(p.DebossShade).InsetShadow(0, 1, 2, 0, p.ShadowA(0.3)).
		Role(mu.RoleProgress).Label(label).Range(0, 100, math.Round(float64(f)*100)).Draw(mirrored(rtl(c), func(pt *mu.Painter, r mu.Rect) {
		if f > 0 {
			pt.Fill(mu.Rect{X: r.X, Y: r.Y, W: max(r.H, r.W*f), H: r.H}, fill, r.H/2)

		}
	}))
}

func a11yKey(c *mu.Context, key string) *mu.Context {
	if key == "" {
		return c
	}
	return c.Key(key)
}

// downloadStrip is the well under the cards: the state of the download.
func (a *App) downloadStrip(c *mu.Context, p theme.Colors, id models.ModelID) {
	m, _ := models.Find(id)
	if a.d.Engine == nil {
		banner(c, p, bannerWarning, i18n.T("onboarding.model.no_engine"))
		return
	}
	st := a.modelStatus(id)
	well := func(fn func()) {
		debossed(mu.Column(a.k(c, "onboarding.model.status")).Gap(10).Padding(16, 18).Radius(12), p).Role(mu.RoleStatus).Children(fn)
	}
	switch st.State {
	case engine.StateDownloading:
		total := st.BytesTotal
		if total == 0 {
			total = m.SizeBytes
		}
		right := ""
		if total > 0 {
			right = i18n.T("onboarding.model.progress", "done", sizeText(st.BytesDone), "total", sizeText(total))
		}
		if left := a.onb.dl.left(id, time.Now(), st.BytesDone, total); left != "" {
			right = i18n.T("onboarding.model.progress_left", "progress", right, "left", left)
		}
		title := i18n.T("onboarding.model.downloading", "name", m.Name)
		well(func() {
			mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
				uiText(c, title, 14, 600, p.Ink).Grow(1).Basis(0).SingleLine().Ellipsis("…")
				mu.Text(c, right).Font(monoFont(right)).FontSize(12).TextColor(p.Graphite).SingleLine()
			})
			track(c, p, "onboarding.model.progress", st.Fraction, 8, title)
			if m.Engine.Kind == models.KindParakeetUnified {
				uiText(c, i18n.T("onboarding.model.parts_note"), 12, 400, p.Graphite)
			}
		})
	case engine.StateFailed:
		msg := i18n.T("onboarding.model.download_failed", "name", m.Name, "error", st.Message)
		if st.Reason != "" {
			msg = i18n.T("onboarding.model.download_failed_reason", "name", m.Name, "reason", engineReason(st))
		}
		mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
			mu.Column(c).Grow(1).Basis(0).MinWidth(0).Children(func() { banner(c, p, bannerWarning, msg) })
			if a.btn(c, p, "onboarding.model.retry", i18n.T("onboarding.model.try_again"), btnSecondary) {
				a.downloadModel(id)
			}
		})
	case engine.StateReady, engine.StateDownloaded, engine.StateOptimizing:
		detail := i18n.T("onboarding.model.downloaded")
		switch st.State {
		case engine.StateReady:
			detail = i18n.TOS("onboarding.model.ready", nil)
		case engine.StateOptimizing:
			detail = i18n.TOS("onboarding.model.optimizing", nil)
		}
		well(func() {
			checkLine(c, p, m.Name+" · "+sizeLabel(m), 600)
			uiText(c, detail, 12, 400, p.Graphite)
		})
	default:
		title := i18n.T("onboarding.model.not_downloaded", "name", m.Name)
		well(func() {
			mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
				uiText(c, title, 14, 600, p.Ink).Grow(1).Basis(0)
				label := i18n.T("onboarding.model.download")
				if m.SizeBytes > 0 {
					label = i18n.T("onboarding.model.download_size", "size", sizeText(m.SizeBytes))
				}
				if a.button(c, p, "onboarding.model.download", label, bopt{size: sizeSmall, icon: "download"}) {
					a.downloadModel(id)
				}
			})
			uiText(c, i18n.T("onboarding.model.choose_then_download"), 12, 400, p.Graphite)
		})
	}
}

// engineReason is the engine's reason code in words, or its message.
func engineReason(st engine.ModelStatus) string {
	if _, ok := i18n.Translate("engine." + st.Reason); ok {
		return i18n.T("engine."+st.Reason, "detail", st.Message)
	}
	return st.Message
}

// localGroups are the groups of the list of all models, in order.
var localGroups = []models.Family{models.FamilyParakeet, models.FamilyWhisper, models.FamilyOther}

func groupTitle(f models.Family) string {
	switch f {
	case models.FamilyParakeet:
		return "Parakeet"
	case models.FamilyWhisper:
		return "Whisper"
	}
	return i18n.T("models.group_more")
}

// allModelsDialog is the list of all local models over the model step.
// Esc, the backdrop or the close button dismiss it.
func (a *App) allModelsDialog(c *mu.Context, p theme.Colors) {
	active := a.FinalModel()
	suggested := suggestedModel(a.cfg.Dictation.Language)
	var list []models.ModelInfo
	for _, m := range localModels() {
		if modelMatches(m, a.onb.allQuery) {
			list = append(list, m)
		}
	}
	panel := mu.DialogBase(c, &a.onb.allModels, func(back, panel mu.Element) {
		back.Background(p.ShadowA(0.28))
		floating(panel.Column().Size(720, 540).Radius(16).Clip(), p).Label(i18n.T("onboarding.all_models.title"))
		mu.Column(c).Shrink(0).Gap(16).Padding(24, 28, 8, 28).Children(func() {
			mu.Row(c).Gap(16).AlignItems(mu.Start).Children(func() {
				mu.Column(c).Gap(4).Grow(1).Basis(0).Children(func() {
					cardTitle(c, p, i18n.T("onboarding.all_models.title"), 24, 30).Role(mu.RoleHeading)
					uiText(c, i18n.TOS("onboarding.all_models.lead", nil), 13, 400, p.Graphite)
				})
				if a.iconBtnSized(c, p, "onboarding.all_models.close", "close", i18n.T("onboarding.all_models.close"), 22, 12) {
					a.closeAllModels()
				}
			})
			a.searchWell(c, p, "onboarding.all_models.filter", i18n.T("onboarding.all_models.filter_placeholder"), &a.onb.allQuery, 38, 0, "").
				Label(i18n.T("onboarding.all_models.filter_label")).Bind(&a.onb.allField).AutoFocus()
		})
		mu.Scroll(a.k(c, "onboarding.all_models.list")).Grow(1).Basis(0).Padding(0, 16, 16, 16).Role(mu.RoleRadioGroup).
			Label(i18n.T("onboarding.all_models.title")).Children(func() {
			if len(list) == 0 {
				uiText(c, i18n.T("onboarding.all_models.no_match"), 13, 400, p.Graphite).Padding(20, 12)
			}
			// The rows in the order they show, for the arrow keys.
			var order []models.ModelID
			for _, fam := range localGroups {
				for _, m := range list {
					if m.Engine.Family() == fam {
						order = append(order, m.ID)
					}
				}
			}
			index := 0
			for _, fam := range localGroups {
				var group []models.ModelInfo
				for _, m := range list {
					if m.Engine.Family() == fam {
						group = append(group, m)
					}
				}
				if len(group) == 0 {
					continue
				}
				mu.Row(c).Height(34).Shrink(0).Gap(14).PaddingX(12).AlignItems(mu.Center).Children(func() {
					uiText(c, strings.ToUpper(groupTitle(fam)), 11, 500, p.Graphite).Grow(1).Role(mu.RoleHeading).Level(3)
					uiText(c, strings.ToUpper(i18n.T("onboarding.all_models.speed")), 11, 500, p.Graphite).Width(82).Shrink(0).AccessHidden(true)
					uiText(c, strings.ToUpper(i18n.T("onboarding.all_models.size")), 11, 500, p.Graphite).Width(64).Shrink(0).TextAlign(mu.End).AccessHidden(true)
				})
				for _, m := range group {
					row := a.allModelsRow(c, p, m, m.ID == active, m.ID == suggested, index, len(list))
					a.radioArrows(row, index, slices.Index(order, active), len(order),
						func(j int) string { return "all:" + string(order[j]) },
						func(j int) { a.chooseModel(order[j]) })
					index++
				}
			}
		})
	})
	if panel.Dismissed() {
		a.closeAllModels()
	}
}

func (a *App) allModelsRow(c *mu.Context, p theme.Colors, m models.ModelInfo, selected, recommended bool, i, n int) mu.Element {
	st := a.modelStatus(m.ID)
	state := ""
	switch {
	case st.State == engine.StateDownloading:
		state = i18n.T("onboarding.all_models.downloading")
	case st.IsOnDisk():
		state = i18n.T("onboarding.all_models.downloaded")
	}
	details := m.Vendor + " · " + languageLabel(m) + " · " + strings.TrimSuffix(modelDescription(m), ".")
	spoken := []string{}
	if recommended {
		spoken = append(spoken, i18n.T("onboarding.model.recommended"))
	}
	if state != "" {
		spoken = append(spoken, state)
	}
	spoken = append(spoken, details, sizeLabel(m))
	b := mu.ButtonBase(a.k(c, "onboarding.all_models."+string(m.ID))).Label(m.Name).Role(mu.RoleRadio).Checked(selected).
		PositionInSet(i+1, n).Description(strings.Join(spoken, ". ")).
		Height(58).Shrink(0).Gap(14).PaddingX(12).Radius(10).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	if selected || b.Hovered() {
		b.Background(p.Deboss)
	}
	b.Children(func() {
		radioDot(c, p, selected)
		mu.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(2).Children(func() {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
				cardTitle(c, p, m.Name, 16, 20).SingleLine().Ellipsis("…").Shrink(1).MinWidth(0)
				if recommended {
					badge(c, p, i18n.T("onboarding.model.recommended"), toneAccent, "")
				}
				if state != "" {
					badge(c, p, state, toneMuted, "")
				}
			})
			uiText(c, details, 12, 400, p.Graphite).FixedLineHeight(16).SingleLine().Ellipsis("…")
		})
		mu.Row(c).Width(82).Shrink(0).Children(func() { meter(c, p, int(m.Speed)) })
		size := sizeLabel(m)
		mu.Text(c, size).Font(monoFont(size)).FontSize(12).TextColor(p.Graphite).Width(64).Shrink(0).TextAlign(mu.End).SingleLine()
	})
	if clicked(c, b) {
		a.chooseModel(m.ID)
		a.closeAllModels()
	}
	return b
}

// sortedLanguages are the dictation languages, by their name.
func sortedLanguages() []string {
	codes := slices.Clone(languages.All())
	if !slices.Contains(codes, languages.Auto) {
		codes = append(codes, languages.Auto)
	}
	slices.SortStableFunc(codes, func(x, y string) int { return strings.Compare(dictationLanguageName(x), dictationLanguageName(y)) })
	return codes
}

func dictationLanguageName(code string) string {
	if code == languages.Auto {
		return i18n.T("models.setup.automatic")
	}
	return i18n.LanguageName(code)
}
