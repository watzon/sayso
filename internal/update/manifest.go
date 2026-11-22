package update

import (
	"bytes"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"log/slog"
	"runtime"
	"sort"
	"strings"
)

// Signed update information (sayso-update manifest.rs; docs/updates.md).

const (
	// ManifestMaxBytes is the largest latest.json that is read.
	ManifestMaxBytes = 64 * 1024
	// ManifestSchema is the schema this version reads.
	ManifestSchema = 1
	// URLPrefix is where every release URL must point.
	URLPrefix = "https://github.com/watzon/sayso/releases/"
	// ManifestURL is the latest.json of the newest release.
	ManifestURL = "https://github.com/watzon/sayso/releases/latest/download/latest.json"
	// DownloadPage is the page for a manual update.
	DownloadPage = "https://justsayso.app/download"
)

// ReleaseKeys are the public keys that sign releases: (id, base64 key).
var ReleaseKeys = [][2]string{{"2026-10", "LxKkwF10F2Oski5tz8EgwPfhNPd90oOmm7/MX66Atgk="}}

// OfficialBuild is "1" in release builds (-ldflags -X ...update.OfficialBuild=1).
var OfficialBuild = ""

// IsOfficialBuild is true for a build that may update itself.
func IsOfficialBuild() bool { return OfficialBuild == "1" }

// Platform is the asset key of this system, as Rust names it ("macos-aarch64").
func Platform() string {
	os := map[string]string{"darwin": "macos"}[runtime.GOOS]
	if os == "" {
		os = runtime.GOOS
	}
	arch := map[string]string{"amd64": "x86_64", "arm64": "aarch64", "386": "x86"}[runtime.GOARCH]
	if arch == "" {
		arch = runtime.GOARCH
	}
	return os + "-" + arch
}

// Envelope is latest.json: a payload and its signatures.
type Envelope struct {
	Payload    string              `json:"payload"`
	Signatures []EnvelopeSignature `json:"signatures"`
}

// EnvelopeSignature is one signature of the payload.
type EnvelopeSignature struct {
	Key       string `json:"key"`
	Signature string `json:"signature"`
}

// Manifest is the signed payload.
type Manifest struct {
	Schema    uint32           `json:"schema"`
	Version   Semver           `json:"version"`
	Published string           `json:"published"`
	NotesURL  string           `json:"notes_url"`
	Assets    map[string]Asset `json:"assets"`
}

// Asset is the download of one platform.
type Asset struct {
	URL    string `json:"url"`
	SHA256 string `json:"sha256"`
	Size   uint64 `json:"size"`
}

// PublicKey is a release key.
type PublicKey struct {
	ID  string
	key ed25519.PublicKey
}

// ManifestError is update information that is not accepted.
type ManifestError struct {
	Kind    string // too_large, malformed, bad_key, bad_signature, not_permitted
	Message string
}

func (e *ManifestError) Error() string {
	switch e.Kind {
	case "too_large":
		return "the update information is too large"
	case "malformed":
		return "the update information is not readable: " + e.Message
	case "bad_key":
		return "a public key is not valid"
	case "bad_signature":
		return "the signature of the update information is not correct"
	}
	return "the update information has a value that is not permitted: " + e.Message
}

// ParsePublicKey reads a base64 Ed25519 public key.
func ParsePublicKey(id, key string) (PublicKey, error) {
	b, err := base64.StdEncoding.DecodeString(key)
	if err != nil || len(b) != ed25519.PublicKeySize {
		return PublicKey{}, &ManifestError{Kind: "bad_key"}
	}
	return PublicKey{ID: id, key: ed25519.PublicKey(b)}, nil
}

// ReleasePublicKeys are the valid ReleaseKeys.
func ReleasePublicKeys() []PublicKey {
	var out []PublicKey
	for _, k := range ReleaseKeys {
		pk, err := ParsePublicKey(k[0], k[1])
		if err != nil {
			slog.Error("update: a public key is not valid", "id", k[0])
			continue
		}
		out = append(out, pk)
	}
	return out
}

// VerifiedKind is the result of a verification.
type VerifiedKind int

const (
	VerifiedManifest VerifiedKind = iota
	VerifiedUnknownKey
	VerifiedUnknownSchema
)

// strictJSON decodes with unknown fields refused (serde deny_unknown_fields).
func strictJSON(data []byte, v any) error {
	d := json.NewDecoder(bytes.NewReader(data))
	d.DisallowUnknownFields()
	if err := d.Decode(v); err != nil {
		return err
	}
	if d.More() {
		return fmt.Errorf("trailing characters")
	}
	return nil
}

// Verify checks latest.json against the release keys.
func Verify(data []byte, keys []PublicKey) (VerifiedKind, *Manifest, error) {
	return VerifyFor(data, keys, URLPrefix)
}

// VerifyFor checks latest.json with a URL prefix. Every signature of a known
// key must be correct; at least one key must be known.
func VerifyFor(data []byte, keys []PublicKey, urlPrefix string) (VerifiedKind, *Manifest, error) {
	if len(data) > ManifestMaxBytes {
		return 0, nil, &ManifestError{Kind: "too_large"}
	}
	var env Envelope
	if err := strictJSON(data, &env); err != nil {
		return 0, nil, &ManifestError{Kind: "malformed", Message: err.Error()}
	}
	if env.Signatures == nil {
		return 0, nil, &ManifestError{Kind: "malformed", Message: "missing field `signatures`"}
	}
	known := 0
	for _, s := range env.Signatures {
		var key *PublicKey
		for i := range keys {
			if keys[i].ID == s.Key {
				key = &keys[i]
				break
			}
		}
		if key == nil {
			continue
		}
		sig, err := base64.StdEncoding.DecodeString(s.Signature)
		if err != nil || len(sig) != ed25519.SignatureSize || !ed25519.Verify(key.key, []byte(env.Payload), sig) {
			return 0, nil, &ManifestError{Kind: "bad_signature"}
		}
		known++
	}
	if known == 0 {
		return VerifiedUnknownKey, nil, nil
	}
	var schema struct {
		Schema *uint32 `json:"schema"`
	}
	if err := json.Unmarshal([]byte(env.Payload), &schema); err != nil || schema.Schema == nil {
		msg := "missing field `schema`"
		if err != nil {
			msg = err.Error()
		}
		return 0, nil, &ManifestError{Kind: "malformed", Message: msg}
	}
	if *schema.Schema != ManifestSchema {
		return VerifiedUnknownSchema, nil, nil
	}
	var m Manifest
	if err := json.Unmarshal([]byte(env.Payload), &m); err != nil {
		return 0, nil, &ManifestError{Kind: "malformed", Message: err.Error()}
	}
	if m.Assets == nil {
		return 0, nil, &ManifestError{Kind: "malformed", Message: "missing field `assets`"}
	}
	if err := m.validate(urlPrefix); err != nil {
		return 0, nil, err
	}
	return VerifiedManifest, &m, nil
}

func belowPrefix(u, prefix string) bool {
	rest, ok := strings.CutPrefix(u, prefix)
	if !ok || rest == "" {
		return false
	}
	for i := 0; i < len(rest); i++ {
		c := rest[i]
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '-' || c == '_' || c == '/') {
			return false
		}
	}
	for _, p := range strings.Split(rest, "/") {
		if p == ".." || p == "." {
			return false
		}
	}
	return true
}

func (m *Manifest) validate(prefix string) error {
	np := func(what string) error { return &ManifestError{Kind: "not_permitted", Message: what} }
	if len(m.Version.Pre) > 0 {
		return np("version " + m.Version.String())
	}
	if !belowPrefix(m.NotesURL, prefix) {
		return np("notes_url " + m.NotesURL)
	}
	names := make([]string, 0, len(m.Assets))
	for k := range m.Assets {
		names = append(names, k)
	}
	sort.Strings(names) // BTreeMap order
	for _, platform := range names {
		a := m.Assets[platform]
		if !belowPrefix(a.URL, prefix) {
			return np(platform + " url " + a.URL)
		}
		hex := len(a.SHA256) == 64 && strings.Trim(a.SHA256, "0123456789abcdef") == ""
		if !hex {
			return np(platform + " sha256")
		}
		if a.Size == 0 {
			return np(platform + " size")
		}
	}
	return nil
}

// OfferKind is what a manifest offers this install.
type OfferKind int

const (
	OfferUpToDate OfferKind = iota
	OfferNewer
	OfferReplayed // older than a version seen before
)

// Release is a newer version for this platform.
type Release struct {
	Version  Semver
	NotesURL string
	Asset    Asset
}

// OfferFor is what manifest offers to a running version.
func OfferFor(m *Manifest, running Semver, highestSeen *Semver, platform string) (OfferKind, *Release) {
	if m.Version.CmpPrecedence(running) <= 0 {
		return OfferUpToDate, nil
	}
	if highestSeen != nil && m.Version.CmpPrecedence(*highestSeen) < 0 {
		return OfferReplayed, nil
	}
	a, ok := m.Assets[platform]
	if !ok {
		return OfferUpToDate, nil
	}
	return OfferNewer, &Release{Version: m.Version, NotesURL: m.NotesURL, Asset: a}
}

// Sign makes an envelope (for release tooling and tests).
func Sign(payload, keyID string, key ed25519.PrivateKey) Envelope {
	sig := ed25519.Sign(key, []byte(payload))
	return Envelope{Payload: payload, Signatures: []EnvelopeSignature{{Key: keyID, Signature: base64.StdEncoding.EncodeToString(sig)}}}
}
