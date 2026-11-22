package providers

import (
	"sync"
)

// KeychainService is the service name for all Sayso secrets, the same as the
// Rust app, so keys saved by either build are found by the other.
const KeychainService = "dev.sayso.Sayso"

// SecretError is a secret store failure.
type SecretError struct{ Msg string }

func (e *SecretError) Error() string { return "secret store error: " + e.Msg }

// SecretStore keeps secrets by account name. Keys never go in config.toml.
type SecretStore interface {
	// Get returns the secret for account, or "" when there is none or the
	// store cannot be read. An empty secret counts as none.
	Get(account string) string
	// Set saves the secret, replacing an earlier one.
	Set(account, secret string) error
	// Delete removes the secret; a missing secret is not an error.
	Delete(account string) error
}

// MemoryStore keeps secrets in memory: for tests and runs without a keychain.
type MemoryStore struct {
	mu      sync.Mutex
	secrets map[string]string
}

// NewMemoryStore makes an empty MemoryStore.
func NewMemoryStore() *MemoryStore { return &MemoryStore{secrets: map[string]string{}} }

func (m *MemoryStore) Get(account string) string {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.secrets[account]
}

func (m *MemoryStore) Set(account, secret string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.secrets[account] = secret
	return nil
}

func (m *MemoryStore) Delete(account string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	delete(m.secrets, account)
	return nil
}

// KeychainStore is the OS secret store: the macOS Keychain, the Windows
// Credential Manager, or the freedesktop Secret Service elsewhere. The item
// layout matches the keyring crate the Rust app uses.
type KeychainStore struct{}

func (KeychainStore) Get(account string) string {
	s, err := osGet(account)
	if err != nil {
		logSecretRead(account, err)
		return ""
	}
	return s
}

func (KeychainStore) Set(account, secret string) error { return osSet(account, secret) }

func (KeychainStore) Delete(account string) error { return osDelete(account) }
