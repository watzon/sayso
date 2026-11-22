//go:build !darwin && !windows

package i18n

func platformLocales() []string { return nil }
