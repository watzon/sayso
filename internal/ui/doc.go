// Package ui is Sayso's desktop interface on MyGo's native UI: the hub
// window (Home, History, Dictionary, Styles, Models, Settings), the
// recording overlay (pill or notch), the tray popover, and the secondary
// windows (onboarding, permissions guide, What's New, update, app
// approval). It is the Go port of crates/sayso-app.
//
// # Shape
//
// App is the model. It owns the config, the history store, the style
// library and a snapshot of the dictation state, and every view is a
// method of App that builds from that model. Views never do I/O while
// building; clicks call App actions, which change the model, save, and
// redraw.
//
// Threading: App's fields are read and written on the UI thread only.
// Work that blocks (engine calls, downloads, the update check) runs on a
// goroutine and comes back through Deps.Post, which is mygo.RunOnMain in
// the app and an inline call in tests.
//
// # Testing
//
// Every interactive element is built with App.k, which gives it a stable
// MyGo key (for mygo-automation selectors such as key=nav.home) and, when
// App.TrackKeys is on, records it, so headless tests can prove that the
// keys exist without a window. Views run under ui.NewTester in the tests
// and in the SAYSO_SMOKE mode of cmd/sayso.
package ui
