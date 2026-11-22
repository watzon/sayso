//go:build windows

package providers

import (
	"errors"
	"log/slog"
	"syscall"
	"unicode/utf16"
	"unsafe"
)

// Windows Credential Manager, laid out as keyring's windows-native store
// does: a generic credential with target "<account>.<service>", user name
// account, the secret as a UTF-16LE blob, and Enterprise persistence.
const (
	credTypeGeneric       = 1
	credPersistEnterprise = 3
	errorNotFound         = syscall.Errno(1168)
)

var (
	advapi32       = syscall.NewLazyDLL("advapi32.dll")
	procCredReadW  = advapi32.NewProc("CredReadW")
	procCredWriteW = advapi32.NewProc("CredWriteW")
	procCredDelete = advapi32.NewProc("CredDeleteW")
	procCredFree   = advapi32.NewProc("CredFree")
)

type credential struct {
	Flags              uint32
	Type               uint32
	TargetName         *uint16
	Comment            *uint16
	LastWritten        [2]uint32
	CredentialBlobSize uint32
	CredentialBlob     *byte
	Persist            uint32
	AttributeCount     uint32
	Attributes         uintptr
	TargetAlias        *uint16
	UserName           *uint16
}

// CredentialTarget is the Credential Manager target name of an account.
func CredentialTarget(account string) string { return account + "." + KeychainService }

func osGet(account string) (string, error) {
	target, err := syscall.UTF16PtrFromString(CredentialTarget(account))
	if err != nil {
		return "", &SecretError{Msg: err.Error()}
	}
	var cred *credential
	r, _, e := procCredReadW.Call(uintptr(unsafe.Pointer(target)), credTypeGeneric, 0, uintptr(unsafe.Pointer(&cred)))
	if r == 0 {
		if errors.Is(e, errorNotFound) {
			return "", nil
		}
		return "", &SecretError{Msg: e.Error()}
	}
	defer procCredFree.Call(uintptr(unsafe.Pointer(cred)))
	blob := unsafe.Slice(cred.CredentialBlob, cred.CredentialBlobSize)
	return decodeUTF16LE(blob), nil
}

func osSet(account, secret string) error {
	target, err := syscall.UTF16PtrFromString(CredentialTarget(account))
	if err != nil {
		return &SecretError{Msg: err.Error()}
	}
	user, err := syscall.UTF16PtrFromString(account)
	if err != nil {
		return &SecretError{Msg: err.Error()}
	}
	blob := encodeUTF16LE(secret)
	cred := credential{Type: credTypeGeneric, TargetName: target, UserName: user, Persist: credPersistEnterprise, CredentialBlobSize: uint32(len(blob))}
	if len(blob) > 0 {
		cred.CredentialBlob = &blob[0]
	}
	if r, _, e := procCredWriteW.Call(uintptr(unsafe.Pointer(&cred)), 0); r == 0 {
		return &SecretError{Msg: e.Error()}
	}
	return nil
}

func osDelete(account string) error {
	target, err := syscall.UTF16PtrFromString(CredentialTarget(account))
	if err != nil {
		return &SecretError{Msg: err.Error()}
	}
	if r, _, e := procCredDelete.Call(uintptr(unsafe.Pointer(target)), credTypeGeneric, 0); r == 0 && !errors.Is(e, errorNotFound) {
		return &SecretError{Msg: e.Error()}
	}
	return nil
}

func encodeUTF16LE(s string) []byte {
	u := utf16.Encode([]rune(s))
	b := make([]byte, 2*len(u))
	for i, c := range u {
		b[2*i] = byte(c)
		b[2*i+1] = byte(c >> 8)
	}
	return b
}

func decodeUTF16LE(b []byte) string {
	u := make([]uint16, len(b)/2)
	for i := range u {
		u[i] = uint16(b[2*i]) | uint16(b[2*i+1])<<8
	}
	return string(utf16.Decode(u))
}

func logSecretRead(account string, err error) {
	slog.Warn("could not read the credential", "account", account, "error", err)
}
