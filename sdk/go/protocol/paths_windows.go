//go:build windows

package protocol

import "golang.org/x/sys/windows"

func userID() (uint32, bool) { return 0, false }

func userSID() string {
	user, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		return ""
	}
	return user.User.Sid.String()
}

func darwinUserTempDir() string { return "" }
