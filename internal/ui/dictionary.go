package ui

import (
	"fmt"
	"strings"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
)

type dictState struct {
	loaded  bool
	words   []store.Word
	reps    []store.Replacement
	filter  string
	newWord string
	from    string
	to      string
	editing int64
	editFr  string
	editTo  string
	try     string
	err     string
}

func (a *App) loadDictionary() {
	if a.dict.loaded {
		return
	}
	a.dict.loaded = true
	a.dict.words, a.dict.reps = nil, nil
	if a.d.Store == nil {
		return
	}
	a.dict.words, _ = a.d.Store.ListWords()
	a.dict.reps, _ = a.d.Store.ListReplacements()
}

// Vocabulary is the words of the dictionary, for the engine.
func (a *App) Vocabulary() []string {
	a.loadDictionary()
	out := make([]string, len(a.dict.words))
	for i, w := range a.dict.words {
		out[i] = w.Text
	}
	return out
}

// AddWord adds a word to the dictionary.
func (a *App) AddWord(text string) {
	text = strings.TrimSpace(text)
	if text == "" || a.d.Store == nil {
		return
	}
	for _, w := range a.dict.words {
		if strings.EqualFold(w.Text, text) {
			a.dict.err = i18n.T("dictionary.error.word_exists", "word", text)
			return
		}
	}
	if _, err := a.d.Store.AddWord(text); err != nil {
		a.dict.err = err.Error()
		return
	}
	a.dict.newWord, a.dict.err = "", ""
	a.dataChanged()
}

// AddReplacement adds a rule.
func (a *App) AddReplacement(from, to string) {
	from = strings.TrimSpace(from)
	if from == "" {
		a.dict.err = i18n.T("dictionary.error.needs_phrase")
		return
	}
	for _, r := range a.dict.reps {
		if strings.EqualFold(r.From, from) {
			a.dict.err = i18n.T("dictionary.error.rule_exists", "from", from)
			return
		}
	}
	if a.d.Store == nil {
		return
	}
	if _, err := a.d.Store.AddReplacement(store.Replacement{From: from, To: to}); err != nil {
		a.dict.err = err.Error()
		return
	}
	a.dict.from, a.dict.to, a.dict.err = "", "", ""
	a.dataChanged()
}

func (a *App) dictionaryPage(c *mu.Context, p theme.Colors) {
	a.loadDictionary()
	d := &a.dict
	mu.Row(c).AlignItems(mu.End).Gap(16).Children(func() {
		mu.Column(c).Grow(1).Children(func() { pageHeader(c, p, i18n.T("dictionary.title"), i18n.T("dictionary.intro")) })
		mu.SearchField(a.k(c, "dict.filter"), &d.filter).Label(i18n.T("dictionary.placeholder.filter")).
			Placeholder(i18n.T("dictionary.placeholder.filter")).Width(220)
	})
	if d.err != "" {
		mu.Text(c, d.err).FontSize(13).TextColor(p.Danger).Role(mu.RoleAlert)
	}
	f := strings.ToLower(strings.TrimSpace(d.filter))
	// Words: slips that wrap, with an inline field to add one.
	mu.Column(c).Gap(10).Children(func() {
		mu.Row(c).Gap(8).AlignItems(mu.Baseline).Children(func() {
			heading(c, p, i18n.T("dictionary.words"), 20)
			mu.Text(c, i18n.T("dictionary.words_note", "total", i18n.TP("home.checklist.words", len(d.words), nil))).FontSize(12.5).TextColor(p.Graphite)
		})
		if len(d.words) == 0 {
			mu.Text(c, i18n.T("dictionary.words_empty")).FontSize(13).TextColor(p.Graphite)
		}
		mu.Row(c).Gap(6).Wrap().AlignItems(mu.Center).Children(func() {
			for _, w := range d.words {
				if f != "" && !strings.Contains(strings.ToLower(w.Text), f) {
					continue
				}
				a.wordSlip(c, p, w)
			}
			in := a.field(c, p, "dict.new_word", i18n.T("dictionary.placeholder.new_word"), i18n.T("dictionary.placeholder.new_word"), &d.newWord).Width(160)
			in.OnSubmit(func() { a.AddWord(d.newWord) })
			if a.btn(c, p, "dict.add_word", i18n.T("dictionary.add_word"), btnSecondary) {
				a.AddWord(d.newWord)
			}
		})
	})
	rule(c, p)
	a.replacementsLedger(c, p, f)
	rule(c, p)
	a.tryPhrase(c, p)
}

func (a *App) wordSlip(c *mu.Context, p theme.Colors, w store.Word) {
	mu.Row(c).Gap(4).Padding(3, 4, 3, 10).Radius(99).Background(p.SheetRaised).Border(1, p.Rule).AlignItems(mu.Center).Children(func() {
		mu.Text(c, w.Text).FontSize(13).TextColor(p.Ink)
		b := mu.ButtonBase(a.k(c, fmt.Sprintf("dict.remove_word.%d", w.ID))).Label(i18n.T("dictionary.remove_word", "word", w.Text)).
			Size(18, 18).Radius(99).Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
		if b.Hovered() {
			b.Background(p.Deboss)
		}
		b.Children(func() { mu.Icon(c, icon("close")).FontSize(10).TextColor(p.Graphite) })
		if clicked(c, b) && a.d.Store != nil {
			a.d.Store.RemoveWord(w.ID)
			a.dataChanged()
		}
	})
}

func (a *App) replacementsLedger(c *mu.Context, p theme.Colors, f string) {
	d := &a.dict
	mu.Column(c).Gap(10).Children(func() {
		mu.Row(c).Gap(8).AlignItems(mu.Baseline).Children(func() {
			heading(c, p, i18n.T("dictionary.replacements"), 20)
			mu.Text(c, i18n.TP("dictionary.replacements_note", len(d.reps), nil)).FontSize(12.5).TextColor(p.Graphite)
		})
		mu.Column(c).Radius(12).Background(p.SheetRaised).Border(1, p.Rule).Dividers(1, p.Rule).Clip().Role(mu.RoleTable).Children(func() {
			ledgerRow(c, p, true, i18n.T("dictionary.column.from"), i18n.T("dictionary.column.to"), i18n.T("dictionary.column.used"), nil)
			if len(d.reps) == 0 {
				mu.Text(c, i18n.T("dictionary.rules_empty")).FontSize(13).TextColor(p.Graphite).Padding(12, 16)
			}
			for _, r := range d.reps {
				if f != "" && !strings.Contains(strings.ToLower(r.From+" "+r.To), f) {
					continue
				}
				if d.editing == r.ID {
					a.editRuleRow(c, p, r)
					continue
				}
				ledgerRow(c, p, false, r.From, showTo(r.To), i18n.Integer(r.Uses), func() {
					if a.iconBtn(c, p, fmt.Sprintf("dict.edit_rule.%d", r.ID), "settings", i18n.T("dictionary.edit_rule", "from", r.From, "to", r.To)) {
						d.editing, d.editFr, d.editTo = r.ID, r.From, r.To
					}
					if a.iconBtn(c, p, fmt.Sprintf("dict.remove_rule.%d", r.ID), "trash", i18n.T("dictionary.remove_rule", "from", r.From)) && a.d.Store != nil {
						a.d.Store.RemoveReplacement(r.ID)
						a.dataChanged()
					}
				})
			}
			// The add row.
			mu.Row(c).Gap(10).Padding(8, 12).AlignItems(mu.Center).Children(func() {
				a.field(c, p, "dict.new_from", i18n.T("dictionary.placeholder.from"), i18n.T("dictionary.placeholder.from"), &d.from).Grow(1).Basis(0)
				mu.Icon(c, icon("arrow-right")).FontSize(14).TextColor(p.Pencil)
				to := a.field(c, p, "dict.new_to", i18n.T("dictionary.placeholder.to"), i18n.T("dictionary.placeholder.to"), &d.to).Grow(1).Basis(0)
				to.OnSubmit(func() { a.AddReplacement(d.from, d.to) })
				if a.btn(c, p, "dict.add_replacement", i18n.T("dictionary.add_rule"), btnSecondary) {
					a.AddReplacement(d.from, d.to)
				}
			})
		})
	})
}

// showTo shows line breaks in a rule's text.
func showTo(to string) string {
	if to == "" {
		return i18n.T("dictionary.nothing")
	}
	if n := strings.Count(to, "\n"); n > 0 && strings.TrimSpace(to) == "" {
		return i18n.TP("dictionary.line_breaks", n, nil)
	}
	return strings.ReplaceAll(to, "\n", "↵")
}

func ledgerRow(c *mu.Context, p theme.Colors, head bool, from, to, used string, actions func()) {
	r := mu.Row(c).Gap(10).Padding(9, 16).AlignItems(mu.Center).Role(mu.RoleRow)
	if head {
		r.Background(p.Sheet)
	}
	r.Children(func() {
		size, fg, font := float32(13.5), p.Ink, theme.Body
		if head {
			size, fg, font = 11.5, p.Graphite, theme.UI
		}
		mu.Text(c, from).Font(font).FontSize(size).TextColor(fg).Grow(1).Basis(0).SingleLine().Ellipsis("…")
		mu.Text(c, to).Font(font).FontSize(size).TextColor(fg).Grow(1).Basis(0).SingleLine().Ellipsis("…")
		mu.Text(c, used).FontSize(size).TextColor(p.Graphite).Width(48).TextAlign(mu.End)
		mu.Row(c).Width(68).Gap(2).Justify(mu.End).Children(func() {
			if actions != nil {
				actions()
			}
		})
	})
}

func (a *App) editRuleRow(c *mu.Context, p theme.Colors, r store.Replacement) {
	d := &a.dict
	mu.Row(c).Gap(10).Padding(8, 12).AlignItems(mu.Center).Background(p.AccentWash.Alpha(0.4)).Children(func() {
		a.field(c, p, "dict.edit_from", i18n.T("dictionary.placeholder.from"), "", &d.editFr).Grow(1).Basis(0)
		a.field(c, p, "dict.edit_to", i18n.T("dictionary.placeholder.to"), "", &d.editTo).Grow(1).Basis(0)
		if a.btn(c, p, "dict.save_rule", i18n.T("dictionary.save_rule"), btnPrimary) {
			if strings.TrimSpace(d.editFr) == "" {
				d.err = i18n.T("dictionary.error.edit_needs_phrase")
				return
			}
			r.From, r.To = strings.TrimSpace(d.editFr), d.editTo
			if a.d.Store != nil {
				if err := a.d.Store.UpdateReplacement(r); err != nil {
					d.err = err.Error()
				}
			}
			d.editing = 0
			a.dataChanged()
		}
		if a.btn(c, p, "dict.cancel_rule", i18n.T("dictionary.cancel_rule"), btnGhost) {
			d.editing = 0
		}
	})
}

func (a *App) tryPhrase(c *mu.Context, p theme.Colors) {
	d := &a.dict
	mu.Column(c).Gap(10).Children(func() {
		heading(c, p, i18n.T("dictionary.try.title"), 20)
		mu.TextArea(a.k(c, "dict.try_input"), &d.try).Label(i18n.T("dictionary.try.title")).
			Placeholder(i18n.T("dictionary.placeholder.phrase")).Height(64)
		out, applied := enhance.NewReplacer(d.reps).Apply(d.try)
		mu.Column(c).Gap(4).Padding(12, 14).Radius(10).Background(p.Deboss).Role(mu.RoleStatus).Children(func() {
			if strings.TrimSpace(d.try) == "" {
				mu.Text(c, i18n.T("dictionary.try.result_empty")).FontSize(13).TextColor(p.Pencil)
				return
			}
			mu.Text(c, out).Font(theme.Body).FontSize(15).TextColor(p.Ink)
			n := 0
			for _, r := range applied {
				n += r.Count
			}
			mu.Text(c, i18n.TP("dictionary.try.rules_apply", n, nil)).FontSize(12).TextColor(p.Graphite)
		})
	})
}
