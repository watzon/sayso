package ui

import (
	"math"
	"time"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/store"
)

// typingWPM is the typing speed behind "time saved", a common average for
// adults.
const typingWPM = 40.0

// Stats are the numbers of the Home strip.
type Stats struct {
	WordsToday        int
	MinutesSavedToday int
	// PaceWPM is the speaking pace over the last 7 days; 0 without enough data.
	PaceWPM       int
	WordsThisWeek int
	// WeekByDay is the words of each day of this week, Monday first.
	WeekByDay  [7]int
	TodayIndex int
}

// ComputeStats is the port of sayso_core::stats::compute.
func ComputeStats(entries []store.StatsEntry, now time.Time) Stats {
	y, m, d := now.Date()
	today := time.Date(y, m, d, 0, 0, 0, 0, now.Location())
	wd := (int(today.Weekday()) + 6) % 7 // Monday is 0
	monday := today.AddDate(0, 0, -wd)
	weekAgo := now.AddDate(0, 0, -7)
	s := Stats{TodayIndex: wd}
	var paceWords int
	var paceMs uint64
	saved := 0.0
	for _, e := range entries {
		local := e.CreatedAt.In(now.Location())
		ly, lm, ld := local.Date()
		day := time.Date(ly, lm, ld, 0, 0, 0, 0, now.Location())
		if day.Equal(today) {
			s.WordsToday += e.Words
			typing := float64(e.Words) / typingWPM
			speaking := float64(e.DurationMs) / 60000
			saved += math.Max(typing-speaking, 0)
		}
		if !day.Before(monday) && !day.After(today) {
			s.WordsThisWeek += e.Words
			idx := int(day.Sub(monday).Hours()/24 + 0.5)
			if idx >= 0 && idx < 7 {
				s.WeekByDay[idx] += e.Words
			}
		}
		if !local.Before(weekAgo) && e.DurationMs > 0 {
			paceWords += e.Words
			paceMs += e.DurationMs
		}
	}
	s.MinutesSavedToday = int(math.Round(saved))
	if paceMs >= 10000 {
		s.PaceWPM = int(math.Round(float64(paceWords) / (float64(paceMs) / 60000)))
	}
	return s
}

// formatCount is "2,184" or "11.9k", in the interface language.
func formatCount(n int) string {
	if n >= 10000 {
		return i18n.Decimal(float64(n)/1000, 1) + "k"
	}
	return i18n.Integer(uint64(n))
}
