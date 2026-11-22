//go:build windows

package paths

import (
	"os"

	"golang.org/x/sys/windows"
)

// SystemSocketEnv reads the real environment.
type SystemSocketEnv struct{}

func (SystemSocketEnv) Var(key string) string     { return os.Getenv(key) }
func (SystemSocketEnv) DarwinUserTempDir() string { return "" }
func (SystemSocketEnv) UID() (uint32, bool)       { return 0, false }
func (SystemSocketEnv) UserSID() string           { return CurrentUserSID() }

// CurrentUserSID is the SID of the user of this process, as text.
func CurrentUserSID() string {
	token := windows.GetCurrentProcessToken()
	user, err := token.GetTokenUser()
	if err != nil {
		return ""
	}
	return user.User.Sid.String()
}
