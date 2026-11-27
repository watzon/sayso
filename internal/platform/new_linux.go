package platform

import "os/exec"

// New builds every Linux service for the session in the environment. The
// backends that need a display or a tool are chosen when they are used.
func New() (*Services, error) {
	s := currentSession()
	ins := newLinuxInserter(s)
	svc := &Services{
		Audio:       newLinuxAudio(),
		Meter:       newLinuxAudio(),
		LiveMic:     newLinuxAudio(),
		Hotkeys:     newLinuxHotkeys(s),
		Inserter:    ins,
		Context:     &linuxContext{session: s},
		Permissions: linuxPermissions{session: s, inserter: ins},
		Sounds:      &linuxSounds{lookPath: exec.LookPath},
		Media:       &linuxMedia{},
		LoginItem:   newLinuxLoginItem(),
		Dock:        linuxDock{},
		AppIcons:    noopAppIcons{},
	}
	// Wayland tells no client where another app's window is.
	if s.kind == sessionX11 {
		svc.Focus = newX11Focus()
	}
	return svc, nil
}
