package platform

// Linux desktops have no Dock policy for an app to set: the window shows in
// the task bar while it is open. The call succeeds and does nothing.
type linuxDock struct{}

func (linuxDock) SetShowInDock(bool) error { return nil }

// DeactivateApp does nothing here: the window manager gives the keyboard back.
func DeactivateApp() {}
