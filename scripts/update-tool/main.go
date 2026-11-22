// Command update-tool is the release tool for update manifests.
//
//	go run ./scripts/update-tool keygen <key id>
//	go run ./scripts/update-tool sign <key id> <payload file>
//	go run ./scripts/update-tool verify <manifest file>
//
// sign reads the private seed (base64) from SAYSO_UPDATE_SIGNING_KEY and prints
// latest.json. SAYSO_UPDATE_URL_PREFIX replaces the URL prefix, for a test
// against a local server.
package main

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/watzon/sayso/internal/update"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	enc := base64.StdEncoding
	switch {
	case len(args) == 2 && args[0] == "keygen":
		seed := make([]byte, ed25519.SeedSize)
		if _, err := rand.Read(seed); err != nil {
			return err
		}
		pub := ed25519.NewKeyFromSeed(seed).Public().(ed25519.PublicKey)
		fmt.Printf("{%q, %q},\n", args[1], enc.EncodeToString(pub))
		fmt.Fprintln(os.Stderr, enc.EncodeToString(seed))
		return nil
	case len(args) == 3 && args[0] == "sign":
		secret := os.Getenv("SAYSO_UPDATE_SIGNING_KEY")
		if secret == "" {
			return fmt.Errorf("SAYSO_UPDATE_SIGNING_KEY is not set")
		}
		seed, err := enc.DecodeString(strings.TrimSpace(secret))
		if err != nil || len(seed) != ed25519.SeedSize {
			return fmt.Errorf("SAYSO_UPDATE_SIGNING_KEY is not 32 bytes in base64")
		}
		key := ed25519.NewKeyFromSeed(seed)
		raw, err := os.ReadFile(args[2])
		if err != nil {
			return err
		}
		payload := strings.TrimSpace(string(raw))
		env := update.Envelope{
			Payload: payload,
			Signatures: []update.EnvelopeSignature{{
				Key:       args[1],
				Signature: enc.EncodeToString(ed25519.Sign(key, []byte(payload))),
			}},
		}
		out, err := json.Marshal(env)
		if err != nil {
			return err
		}
		// Refuse to print a manifest that the app with this key would not accept.
		pk, err := update.ParsePublicKey(args[1], enc.EncodeToString(key.Public().(ed25519.PublicKey)))
		if err != nil {
			return err
		}
		prefix := os.Getenv("SAYSO_UPDATE_URL_PREFIX")
		if prefix == "" {
			prefix = update.URLPrefix
		}
		kind, _, err := update.VerifyFor(out, []update.PublicKey{pk}, prefix)
		if err != nil {
			return err
		}
		if kind != update.VerifiedManifest {
			return fmt.Errorf("the payload does not verify")
		}
		fmt.Println(string(out))
		return nil
	case len(args) == 2 && args[0] == "verify":
		data, err := os.ReadFile(args[1])
		if err != nil {
			return err
		}
		kind, m, err := update.Verify(data, update.ReleasePublicKeys())
		if err != nil {
			return err
		}
		if kind != update.VerifiedManifest {
			return fmt.Errorf("not accepted (kind %d)", kind)
		}
		names := make([]string, 0, len(m.Assets))
		for k := range m.Assets {
			names = append(names, k)
		}
		fmt.Printf("ok: version %v for %s\n", m.Version, strings.Join(names, ", "))
		return nil
	}
	return fmt.Errorf("usage: update-tool keygen <key id> | sign <key id> <payload file> | verify <manifest file>")
}
