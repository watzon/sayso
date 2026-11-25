package main

import (
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/telemetry"
)

// telemetryOn is whether signals may leave: the switch is on and onboarding
// is over, so nothing is sent before the user has seen the switch.
func telemetryOn(c *config.Config) bool { return c.Onboarding.Completed && c.Telemetry.Enabled }

// telemetrySwitch follows the config. It applies start now and returns the
// hook for each later config.
func telemetrySwitch(t *telemetry.Client, start *config.Config) func(*config.Config) {
	apply := func(c *config.Config) {
		t.SetEnabled(telemetryOn(c))
		t.Launched()
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
