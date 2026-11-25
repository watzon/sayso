package main

import (
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/telemetry"
)

// telemetrySwitch follows the config: the switch, and the end of onboarding.
// app.installed and app.launched wait for that end, so nothing is sent
// before the user has seen the switch. It applies start now and returns the
// hook for each later config.
func telemetrySwitch(t *telemetry.Client, start *config.Config) func(*config.Config) {
	apply := func(c *config.Config) {
		t.SetEnabled(c.Telemetry.Enabled)
		if c.Onboarding.Completed {
			t.Launched()
		}
	}
	apply(start)
	return apply
}

// eachConfig runs every hook on each published config.
func eachConfig(hooks ...func(*config.Config)) func(*config.Config) {
	return func(c *config.Config) {
		for _, h := range hooks {
			h(c)
		}
	}
}
