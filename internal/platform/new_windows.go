package platform

// New returns the Windows services.
func New() (*Services, error) {
	return &Services{
		Audio:       &windowsAudio{},
		Hotkeys:     newWindowsHotkeys(),
		Inserter:    windowsInserter{},
		Context:     windowsContext{},
		Permissions: windowsPermissions{},
		Sounds:      &windowsSounds{},
		Media:       &windowsMedia{},
		LoginItem:   windowsLoginItem{},
		Dock:        windowsDock{},
	}, nil
}
