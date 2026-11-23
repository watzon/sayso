package ui

import (
	"fmt"
	"math"
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
	// wordsScroll and repsScroll are how far the words and the rules
	// are scrolled, on a wide page where each scrolls on its own.
	wordsScroll, repsScroll mu.ScrollState
	// wordField is the add-word input, which the header's Add focuses.
	wordField mu.Handle
	// addOpen opens the dashed "+ Add word" slip into a well; removeShown
	// shows each word's remove button, by word.
	addOpen     tween
	removeShown map[int64]*tween
}

func (a *App) loadDictionary() {
	if a.dict.loaded {
		return
	}
	a.dict.loaded = true
	a.dict.words, a.dict.reps, a.dict.removeShown = nil, nil, nil
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
	pageHead(c, p, i18n.T("dictionary.title"), i18n.T("dictionary.intro"), 520, func() {
		a.searchWell(c, p, "dict.filter", i18n.T("dictionary.placeholder.filter"), &d.filter, 36, 220, "")
		if a.button(c, p, "dict.add", i18n.T("dictionary.add"), bopt{kind: btnPrimary, icon: "plus", size: sizeLarge}) {
			if strings.TrimSpace(d.newWord) != "" {
				a.AddWord(d.newWord)
			} else {
				d.wordField.Focus(c)
			}
		}
	})
	if d.err != "" {
		uiText(c, d.err, 13, 500, p.Danger).Role(mu.RoleAlert)
	}
	f := strings.ToLower(strings.TrimSpace(d.filter))
	w, _ := c.Size()
	// Wide, the columns fill the page and scroll on their own (the page
	// does not scroll), with Try a phrase held at the foot of the words.
	// Narrow, they stack and the page scrolls.
	wide := w-sidebarWidth >= 820
	cols := mu.Row(c).Gap(44).AlignItems(mu.Start)
	if wide {
		cols.AlignItems(mu.Stretch).Grow(1).MinHeight(0)
	} else {
		cols.Column().Gap(32).AlignItems(mu.Stretch)
	}
	cols.Children(func() {
		left := mu.Column(c).Gap(32)
		if wide {
			left.Width(380).Shrink(0).MinHeight(0).Gap(24)
		}
		left.Children(func() {
			a.wordsSection(c, p, f, wide)
			a.tryPhrase(c, p)
		})
		right := mu.Column(c).Grow(1).Basis(0).MinWidth(0)
		if wide {
			right.MinHeight(0)
		}
		right.Children(func() { a.replacementsLedger(c, p, f, wide) })
	})
}

// slipRoom is the room a scrolling list of slips keeps around them for
// their shadows and the add well's focus ring, which its edge would clip.
const slipRoom = 10

// barGutter is the room past the end of a scrolling list for its scroll
// bar, which MyGo draws over the list's end edge (the left, right to
// left): the bar's 12 px track and 4 px to spare, so the thumb never sits
// on the list's trailing text.
const barGutter = 16

// wordsSection is the words as slips that wrap, and a dashed slip to add one.
// In scroll, the slips scroll under the heading.
func (a *App) wordsSection(c *mu.Context, p theme.Colors, f string, scroll bool) {
	d := &a.dict
	sec := mu.Column(c).Gap(16)
	if scroll {
		sec.Grow(1).MinHeight(0)
	}
	sec.Children(func() {
		sectionHead(c, p, i18n.T("dictionary.words"), i18n.T("dictionary.words_note", "total", i18n.Integer(uint64(len(d.words)))))
		slips := func() {
			if len(d.words) == 0 {
				uiText(c, i18n.T("dictionary.words_empty"), 13, 400, p.Graphite).LineHeight(1.4)
			}
			mu.Row(c).Gap(8).Wrap().AlignItems(mu.Center).Children(func() {
				for _, w := range d.words {
					if f != "" && !strings.Contains(strings.ToLower(w.Text), f) {
						continue
					}
					a.wordSlip(c, p, w)
				}
				a.addWordSlip(c, p)
			})
		}
		if !scroll {
			mu.Column(c).Gap(16).Children(slips)
			return
		}
		// The room for the shadows, and at the end for the scroll bar,
		// sits outside the column's edges, so the slips line up with the
		// heading.
		mu.Scroll(c.Key("dict.words.scroll")).Grow(1).MinHeight(0).Margin(-slipRoom).Padding(slipRoom).
			MarginEnd(-barGutter).PaddingEnd(barGutter).Gap(16).
			Role(mu.RoleGroup).Label(i18n.T("dictionary.words")).TrackScroll(&d.wordsScroll).Children(slips)
	})
}

// wordSlip is a word on a raised slip, with its remove button. The button
// keeps its place when hidden and fades in on hover or focus, so a slip
// never changes its size: a slip at the end of a line would jump to the
// next one as the pointer crossed it.
func (a *App) wordSlip(c *mu.Context, p theme.Colors, w store.Word) {
	d := &a.dict
	s := mu.Row(c).Height(32).PaddingStart(12).PaddingEnd(6).Gap(4).Radius(6).AlignItems(mu.Center).Background(p.SheetRaised).
		InsetShadow(0, 1, 0, 0, p.Highlight(1)).Shadow(0, 1, 2, 0, p.ShadowA(0.16)).Shadow(0, 3, 8, 0, p.ShadowA(0.06))
	s.Children(func() {
		serif(c, w.Text, 15, p.Ink).SingleLine()
		b := mu.ButtonBase(a.k(c, fmt.Sprintf("dict.remove_word.%d", w.ID))).Label(i18n.T("dictionary.remove_word", "word", w.Text)).
			Size(18, 18).Shrink(0).Radius(99).Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
		if d.removeShown == nil {
			d.removeShown = map[int64]*tween{}
		}
		shown := d.removeShown[w.ID]
		if shown == nil {
			shown = &tween{}
			d.removeShown[w.ID] = shown
		}
		b.Opacity(unit(shown.update(c, s.Hovered() || b.Focused())))
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

// addWordSlip is the dashed "+ Add word" slip; it opens into a well while
// you type, easing its width, its fill and its border. Its + adds the word.
// The slip fits its label, measured in the language of the app, and the
// well is wider, for the word.
func (a *App) addWordSlip(c *mu.Context, p theme.Colors) {
	d := &a.dict
	typing := d.newWord != "" || d.wordField.Focused(c)
	label := i18n.T("dictionary.add_word")
	lw, _ := c.MeasureText(0, mu.Span{Text: label, Size: 14, Weight: 400})
	// Border, padding, the +, the gap, the label and the caret's room.
	rest := 2 + 2*addSlipPad + 16 + 6 + float32(math.Ceil(float64(lw))) + 8
	well := max(170, rest+48)
	t := d.addOpen.update(c, typing)
	o := unit(t)
	s := mu.Row(c).Height(32).Width(rest + (well-rest)*t).Shrink(0).PaddingX(addSlipPad).Gap(6).Radius(6).AlignItems(mu.Center)
	// The dashed slip's border fades as the well's fill and shade come in.
	s.Border(1, p.DebossShade.Alpha(1-o)).BorderStyle(mu.BorderDashed)
	fill := p.Deboss.Alpha(o)
	if !typing && s.Hovered() {
		fill = p.Deboss.Alpha(max(o, 0.4))
	}
	s.Background(fill).InsetShadow(0, 1, 3, 0, p.ShadowA(0.22*o)).InsetShadow(0, -1, 0, 0, p.Highlight(0.75*o))
	if d.wordField.Focused(c) {
		// The focus ring of the well.
		s.Shadow(0, 0, 0, 2, p.Accent.Alpha(0.35*o))
	}
	s.Children(func() {
		add := mu.ButtonBase(a.k(c, "dict.add_word")).Label(i18n.T("dictionary.add_word")).Size(16, 16).Radius(4).
			Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
		add.Children(func() { mu.Icon(c, icon("plus")).FontSize(13).TextColor(p.Graphite) })
		if clicked(c, add) {
			if strings.TrimSpace(d.newWord) != "" {
				a.AddWord(d.newWord)
			} else {
				d.wordField.Focus(c)
			}
		}
		in := mu.TextInputBase(a.k(c, "dict.new_word"), &d.newWord).Bind(&d.wordField).Label(i18n.T("dictionary.placeholder.new_word")).
			Placeholder(label).PlaceholderColor(p.Graphite).FontSize(14).TextColor(p.Ink).Grow(1).Basis(0).MinWidth(0)
		in.OnSubmit(func() { a.AddWord(d.newWord) })
	})
}

// addSlipPad is the padding at each end of the add-word slip.
const addSlipPad = 12

// replacementsLedger is the table of rules. In scroll, its rows scroll
// between the heading, which stays, and the row that adds a rule.
func (a *App) replacementsLedger(c *mu.Context, p theme.Colors, f string, scroll bool) {
	d := &a.dict
	col := mu.Column(c).Gap(0)
	if scroll {
		col.Grow(1).MinHeight(0)
	}
	col.Children(func() {
		sectionHead(c, p, i18n.T("dictionary.replacements"), i18n.TP("dictionary.replacements_note", len(d.reps), nil))
		table := mu.Column(c).Role(mu.RoleTable)
		if scroll {
			table.MinHeight(0)
		}
		table.Children(func() {
			mu.Row(c).Height(36).Shrink(0).Gap(12).AlignItems(mu.Center).Role(mu.RoleRow).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
				caps(c, p, i18n.T("dictionary.column.from")).Width(150).Shrink(0)
				mu.Box(c).Width(18).Shrink(0)
				caps(c, p, i18n.T("dictionary.column.to")).Grow(1).Basis(0)
				caps(c, p, i18n.T("dictionary.column.used")).Width(usedColumnW(c)).Shrink(0).TextAlign(mu.End)
			})
			rows := func() {
				if len(d.reps) == 0 {
					uiText(c, i18n.T("dictionary.rules_empty"), 13, 400, p.Graphite).PaddingY(14).LineHeight(1.4)
				}
				for _, r := range d.reps {
					if f != "" && !strings.Contains(strings.ToLower(r.From+" "+r.To), f) {
						continue
					}
					if d.editing == r.ID {
						a.editRuleRow(c, p, r)
						continue
					}
					a.ledgerRow(c, p, r)
				}
			}
			if scroll {
				// As tall as its rows, until the column runs out of room.
				// The bar sits in a gutter past the table's end, in the
				// page's margin, so the rows line up with the heading.
				mu.Scroll(c.Key("dict.replacements.scroll")).Shrink(1).MinHeight(0).MarginEnd(-barGutter).PaddingEnd(barGutter).
					TrackScroll(&d.repsScroll).Children(rows)
			} else {
				rows()
			}
			a.addRuleRow(c, p)
		})
	})
}

// usedColumnW is the width of the ledger's Used column: 56 DIPs, the room
// of the edit and remove buttons, or its heading's when a translation is
// wider ("مرات الاستخدام", "ПРИМЕНЕНО").
func usedColumnW(c *mu.Context) float32 {
	head := strings.ToUpper(i18n.T("dictionary.column.used"))
	w, _ := c.MeasureText(0, mu.Span{Text: head, Font: theme.UIFor(i18n.CurrentLocale()), Size: 12, Weight: 500})
	return max(56, float32(math.Ceil(float64(w))))
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

func (a *App) ledgerRow(c *mu.Context, p theme.Colors, r store.Replacement) {
	d := &a.dict
	row := mu.Row(c).Height(48).Shrink(0).Gap(12).AlignItems(mu.Center).Role(mu.RoleRow).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule)
	row.Children(func() {
		serif(c, r.From, 15, p.Graphite).Italic().Width(150).Shrink(0).SingleLine().Ellipsis("…")
		mu.Icon(c, dirIcon(c, "arrow-right")).FontSize(16).TextColor(p.Ink).Width(18)
		serif(c, showTo(r.To), 15, p.Ink).Grow(1).Basis(0).SingleLine().Ellipsis("…")
		mu.Row(c).Width(usedColumnW(c)).Shrink(0).Gap(2).Justify(mu.End).AlignItems(mu.Center).Children(func() {
			if row.Hovered() || row.FocusWithin() {
				if a.iconBtnSized(c, p, fmt.Sprintf("dict.edit_rule.%d", r.ID), "settings", i18n.T("dictionary.edit_rule", "from", r.From, "to", r.To), 26, 14) {
					d.editing, d.editFr, d.editTo = r.ID, r.From, r.To
				}
				if a.iconBtnSized(c, p, fmt.Sprintf("dict.remove_rule.%d", r.ID), "trash", i18n.T("dictionary.remove_rule", "from", r.From), 26, 14) && a.d.Store != nil {
					a.d.Store.RemoveReplacement(r.ID)
					a.dataChanged()
				}
				return
			}
			mono(c, i18n.Integer(r.Uses), 12, p.Graphite)
		})
	})
}

// ruleInput is a small debossed well of a ledger row.
func (a *App) ruleInput(c *mu.Context, p theme.Colors, key, label, placeholder string, v *string, from bool) mu.Element {
	in := mu.TextInputBase(a.k(c, key), v).Label(label).Placeholder(placeholder).PlaceholderColor(p.Pencil).
		Height(32).PaddingX(10).Radius(7).FontSize(15).TextColor(p.Ink)
	if from {
		in.Font(serifFont(theme.Display)).Italic().Width(150).Shrink(0)
	} else {
		in.Font(serifFont(theme.Display)).Grow(1).Basis(0)
	}
	return debossed(in, p)
}

func (a *App) addRuleRow(c *mu.Context, p theme.Colors) {
	d := &a.dict
	mu.Row(c).Height(52).Shrink(0).Gap(12).AlignItems(mu.Center).Children(func() {
		a.ruleInput(c, p, "dict.new_from", i18n.T("dictionary.placeholder.from"), i18n.T("dictionary.placeholder.from"), &d.from, true)
		mu.Icon(c, dirIcon(c, "arrow-right")).FontSize(16).TextColor(p.Pencil).Width(18)
		to := a.ruleInput(c, p, "dict.new_to", i18n.T("dictionary.placeholder.to"), i18n.T("dictionary.placeholder.to"), &d.to, false)
		to.OnSubmit(func() { a.AddReplacement(d.from, d.to) })
		if a.button(c, p, "dict.add_replacement", i18n.T("dictionary.add_rule"), bopt{size: sizeSmall}) {
			a.AddReplacement(d.from, d.to)
		}
	})
}

func (a *App) editRuleRow(c *mu.Context, p theme.Colors, r store.Replacement) {
	d := &a.dict
	mu.Row(c).Height(52).Shrink(0).Gap(12).AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		a.ruleInput(c, p, "dict.edit_from", i18n.T("dictionary.placeholder.from"), "", &d.editFr, true)
		mu.Icon(c, dirIcon(c, "arrow-right")).FontSize(16).TextColor(p.Pencil).Width(18)
		a.ruleInput(c, p, "dict.edit_to", i18n.T("dictionary.placeholder.to"), "", &d.editTo, false)
		if a.button(c, p, "dict.save_rule", i18n.T("dictionary.save_rule"), bopt{kind: btnPrimary, size: sizeSmall}) {
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
		if a.button(c, p, "dict.cancel_rule", i18n.T("dictionary.cancel_rule"), bopt{kind: btnGhost, size: sizeSmall}) {
			d.editing = 0
		}
	})
}

// tryPhrase runs the rules on a phrase as you type it.
func (a *App) tryPhrase(c *mu.Context, p theme.Colors) {
	d := &a.dict
	out, applied := enhance.NewReplacer(d.reps).Apply(d.try)
	n := len(d.reps)
	if strings.TrimSpace(d.try) != "" {
		n = 0
		for _, r := range applied {
			n += r.Count
		}
	}
	mu.Column(c).Padding(18).Gap(12).Radius(14).Background(p.Deboss).
		InsetShadow(0, 2, 4, 0, p.ShadowA(0.16)).InsetShadow(0, -1, 0, 0, p.Highlight(0.75)).Children(func() {
		mu.Row(c).AlignItems(mu.Center).Children(func() {
			caps(c, p, i18n.T("dictionary.try.title")).Grow(1)
			uiText(c, i18n.TP("dictionary.try.rules_apply", n, nil), 13, 500, p.Graphite)
		})
		mu.TextInputBase(a.k(c, "dict.try_input"), &d.try).Label(i18n.T("dictionary.try.title")).
			Placeholder(i18n.T("dictionary.placeholder.phrase")).PlaceholderColor(p.Graphite).
			Font(serifFont(theme.Display)).Italic().FontSize(15).TextColor(p.Ink)
		mu.Box(c).Height(1).Background(p.DebossShade)
		mu.Column(c).Role(mu.RoleStatus).Children(func() {
			if strings.TrimSpace(d.try) == "" {
				serif(c, i18n.T("dictionary.try.result_empty"), 15, p.Pencil).LineHeight(21.0 / 15)
				return
			}
			serif(c, out, 15, p.Ink).LineHeight(21.0 / 15)
		})
	})
}
