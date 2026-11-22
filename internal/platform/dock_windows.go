package platform

// windowsDock: the taskbar button follows the app's windows on Windows, so
// there is nothing to set.
type windowsDock struct{}

func (windowsDock) SetShowInDock(bool) error { return nil }

// DeactivateApp does nothing here: the window manager gives the keyboard back.
func DeactivateApp() {}
