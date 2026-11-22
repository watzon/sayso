package platform

// New builds every macOS service. It loads the frameworks once; an error
// means that the system frameworks could not be loaded.
func New() (*Services, error) {
	if err := loadDarwin(); err != nil {
		return nil, err
	}
	return &Services{
		Audio:       &macAudio{},
		Hotkeys:     newMacHotkeys(),
		Inserter:    &macInserter{},
		Context:     macContext{},
		Permissions: macPermissions{},
		Sounds:      macSounds{},
		Media:       newMacMedia(),
		LoginItem:   macLoginItem{},
		Dock:        macDock{},
		AppIcons:    macAppIcons{},
	}, nil
}
