package i18n

import "time"

var (
	monthKeys = [...]string{"january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"}
	dayKeys   = [...]string{"sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"}
)

// DayMonthShort is "Sep 28".
func DayMonthShort(d time.Time) string {
	return T("date.pattern.day_month_short", "month", MonthShort(d.Month()), "day", d.Day())
}

// DayMonth is "September 28".
func DayMonth(d time.Time) string {
	return T("date.pattern.day_month", "month", Month(d.Month()), "day", d.Day())
}

// WeekdayDayMonth is "Monday, September 28".
func WeekdayDayMonth(d time.Time) string {
	return T("date.pattern.weekday_day_month", "weekday", Weekday(d.Weekday()), "month", Month(d.Month()), "day", d.Day())
}

// DayMonthYear is "September 28, 2026".
func DayMonthYear(d time.Time) string {
	return T("date.pattern.day_month_year", "month", Month(d.Month()), "day", d.Day(), "year", d.Year())
}

// Today is "Today".
func Today() string { return T("date.today") }

// Yesterday is "Yesterday".
func Yesterday() string { return T("date.yesterday") }

// Month is the name of a month: "September".
func Month(m time.Month) string { return T("date.month." + monthKeys[monthIndex(m)]) }

// MonthShort is the short name of a month: "Sep".
func MonthShort(m time.Month) string { return T("date.month_short." + monthKeys[monthIndex(m)]) }

// Weekday is the name of a day: "Monday".
func Weekday(d time.Weekday) string { return T("date.weekday." + dayKeys[int(d)%7]) }

// monthIndex maps a month to 0..11; like the Rust port, out of range is December.
func monthIndex(m time.Month) int {
	if m < time.January || m > time.December {
		return 11
	}
	return int(m) - 1
}
