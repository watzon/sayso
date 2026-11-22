//go:build !darwin && !windows

package providers

import (
	"bytes"
	"errors"
	"log/slog"
	"os/exec"
	"strings"
)

// The freedesktop Secret Service, through libsecret's secret-tool. Items
// carry the attributes keyring's Secret Service store uses (service and
// username) and its label "keyring:<user>@<service>".

var secretTool = "secret-tool"

func secretToolCmd(args ...string) *exec.Cmd { return exec.Command(secretTool, args...) }

func osGet(account string) (string, error) {
	var out, errOut bytes.Buffer
	cmd := secretToolCmd("lookup", "service", KeychainService, "username", account)
	cmd.Stdout, cmd.Stderr = &out, &errOut
	if err := cmd.Run(); err != nil {
		var exit *exec.ExitError
		// secret-tool exits 1 with no output when no item matches.
		if errors.As(err, &exit) && strings.TrimSpace(errOut.String()) == "" {
			return "", nil
		}
		return "", &SecretError{Msg: strings.TrimSpace(errOut.String() + " " + err.Error())}
	}
	return out.String(), nil
}

func osSet(account, secret string) error {
	var errOut bytes.Buffer
	cmd := secretToolCmd("store", "--label=keyring:"+account+"@"+KeychainService, "service", KeychainService, "username", account)
	cmd.Stdin = strings.NewReader(secret)
	cmd.Stderr = &errOut
	if err := cmd.Run(); err != nil {
		return &SecretError{Msg: strings.TrimSpace(errOut.String() + " " + err.Error())}
	}
	return nil
}

func osDelete(account string) error {
	var errOut bytes.Buffer
	cmd := secretToolCmd("clear", "service", KeychainService, "username", account)
	cmd.Stderr = &errOut
	if err := cmd.Run(); err != nil {
		var exit *exec.ExitError
		if errors.As(err, &exit) && strings.TrimSpace(errOut.String()) == "" {
			return nil
		}
		return &SecretError{Msg: strings.TrimSpace(errOut.String() + " " + err.Error())}
	}
	return nil
}

func logSecretRead(account string, err error) {
	slog.Warn("could not read the secret", "account", account, "error", err)
}
