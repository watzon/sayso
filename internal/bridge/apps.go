package bridge

import (
	"bytes"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/watzon/sayso/internal/config"
)

// apps.toml: the answers of the user about connected apps (apps.rs).

// AppRecord is one app in apps.toml. It names its program with exactly one
// of team_id + identifier, path, or flatpak.
type AppRecord struct {
	Name       string
	TeamID     *string
	Identifier *string
	Signer     *string
	Path       *string
	Flatpak    *string
	Allowed    bool
	Granted    []string
	Denied     []string
	// FirstSeen is a TOML datetime as it was read (time.Time or a go-toml
	// local date/time), or nil.
	FirstSeen any
}

// Apps is the whole file.
type Apps struct {
	Apps []AppRecord
}

// Identity is the identity of the record, or false when it names no
// program or more than one.
func (r *AppRecord) Identity() (Identity, bool) {
	switch {
	case r.TeamID != nil && r.Identifier != nil && r.Path == nil && r.Flatpak == nil:
		return Identity{Kind: IdentitySigned, TeamID: *r.TeamID, Identifier: *r.Identifier}, true
	case r.TeamID == nil && r.Identifier == nil && r.Path != nil && r.Flatpak == nil:
		return Identity{Kind: IdentityPath, Path: *r.Path}, true
	case r.TeamID == nil && r.Identifier == nil && r.Path == nil && r.Flatpak != nil:
		return Identity{Kind: IdentityFlatpak, AppID: *r.Flatpak}, true
	}
	return Identity{}, false
}

// ID is the key of the identity of the record, or "".
func (r *AppRecord) ID() string {
	if id, ok := r.Identity(); ok {
		return id.ID()
	}
	return ""
}

// ShownName is the name without the characters that can mislead.
func (r *AppRecord) ShownName() string {
	if n, ok := CleanName(r.Name); ok {
		return n
	}
	return "?"
}

// Capabilities are the capabilities that the app has: models.read always,
// and each known grant that is not also denied.
func (r *AppRecord) Capabilities() []Capability {
	caps := []Capability{CapModelsRead}
	denied := r.DeniedCapabilities()
	for _, c := range knownCaps(r.Granted) {
		if !slices.Contains(caps, c) && !slices.Contains(denied, c) {
			caps = append(caps, c)
		}
	}
	return caps
}

// DeniedCapabilities are the known capabilities that the user refused.
func (r *AppRecord) DeniedCapabilities() []Capability { return knownCaps(r.Denied) }

func (r *AppRecord) hasAnswer(c Capability) bool {
	return slices.Contains(r.Granted, string(c)) || slices.Contains(r.Denied, string(c))
}

func knownCaps(names []string) []Capability {
	out := []Capability{}
	for _, n := range names {
		c := Capability(n)
		if c.Known() && c != CapModelsRead {
			out = append(out, c)
		}
	}
	return out
}

// askable are the asked capabilities that need an answer of the user, each
// once: not models.read and not names that this version does not know.
func askable(asked []Capability) []Capability {
	out := []Capability{}
	for _, c := range asked {
		if c.Known() && c != CapModelsRead && !slices.Contains(out, c) {
			out = append(out, c)
		}
	}
	return out
}

func unanswered(r *AppRecord, asked []Capability) []Capability {
	out := []Capability{}
	for _, c := range askable(asked) {
		if !r.hasAnswer(c) {
			out = append(out, c)
		}
	}
	return out
}

// Find is the record of an identity.
func (a *Apps) Find(id Identity) *AppRecord { return a.Record(id.ID()) }

// Record is the record with this key.
func (a *Apps) Record(id string) *AppRecord {
	for i := range a.Apps {
		if a.Apps[i].ID() == id {
			return &a.Apps[i]
		}
	}
	return nil
}

// AnyApproved is true when the user allowed an app.
func (a *Apps) AnyApproved() bool {
	for i := range a.Apps {
		if _, ok := a.Apps[i].Identity(); ok && a.Apps[i].Allowed {
			return true
		}
	}
	return false
}

func (a *Apps) clone() *Apps {
	out := &Apps{}
	if a.Apps != nil {
		out.Apps = make([]AppRecord, len(a.Apps))
	}
	for i, r := range a.Apps {
		r.Granted = slices.Clone(r.Granted)
		r.Denied = slices.Clone(r.Denied)
		out.Apps[i] = r
	}
	return out
}

// ParseApps reads apps.toml with the rules of serde: unknown keys are left
// out, and a key of the wrong type is an error.
func ParseApps(text string) (*Apps, error) {
	doc, err := config.ParseTOML(text)
	if err != nil {
		return nil, err
	}
	apps := &Apps{}
	raw, ok := doc["apps"]
	if !ok {
		return apps, nil
	}
	list, ok := raw.([]any)
	if !ok {
		return nil, errors.New("apps: invalid type, expected a sequence")
	}
	for i, e := range list {
		m, ok := e.(map[string]any)
		if !ok {
			return nil, fmt.Errorf("apps[%d]: invalid type, expected struct AppRecord", i)
		}
		r, err := parseRecord(m)
		if err != nil {
			return nil, fmt.Errorf("apps[%d]: %v", i, err)
		}
		apps.Apps = append(apps.Apps, r)
	}
	return apps, nil
}

func parseRecord(m map[string]any) (AppRecord, error) {
	var r AppRecord
	str := func(key string) (*string, error) {
		v, ok := m[key]
		if !ok {
			return nil, nil
		}
		s, ok := v.(string)
		if !ok {
			return nil, fmt.Errorf("%s: invalid type, expected a string", key)
		}
		return &s, nil
	}
	list := func(key string) ([]string, error) {
		v, ok := m[key]
		if !ok {
			return []string{}, nil
		}
		arr, ok := v.([]any)
		if !ok {
			return nil, fmt.Errorf("%s: invalid type, expected a sequence", key)
		}
		out := make([]string, 0, len(arr))
		for _, e := range arr {
			s, ok := e.(string)
			if !ok {
				return nil, fmt.Errorf("%s: invalid type, expected a string", key)
			}
			out = append(out, s)
		}
		return out, nil
	}
	name, err := str("name")
	if err != nil {
		return r, err
	}
	if name == nil {
		return r, errors.New("missing field `name`")
	}
	r.Name = *name
	if r.TeamID, err = str("team_id"); err != nil {
		return r, err
	}
	if r.Identifier, err = str("identifier"); err != nil {
		return r, err
	}
	if r.Signer, err = str("signer"); err != nil {
		return r, err
	}
	if r.Path, err = str("path"); err != nil {
		return r, err
	}
	if r.Flatpak, err = str("flatpak"); err != nil {
		return r, err
	}
	if v, ok := m["allowed"]; ok {
		b, ok := v.(bool)
		if !ok {
			return r, errors.New("allowed: invalid type, expected a boolean")
		}
		r.Allowed = b
	}
	if r.Granted, err = list("granted"); err != nil {
		return r, err
	}
	if r.Denied, err = list("denied"); err != nil {
		return r, err
	}
	if v, ok := m["first_seen"]; ok {
		switch v.(type) {
		case time.Time:
		default:
			if _, isString := v.(string); isString || !isDatetime(v) {
				return r, errors.New("first_seen: invalid type, expected a TOML datetime")
			}
		}
		r.FirstSeen = v
	}
	return r, nil
}

func isDatetime(v any) bool {
	switch reflect.TypeOf(v).String() {
	case "toml.LocalDate", "toml.LocalDateTime", "toml.LocalTime":
		return true
	}
	return false
}

// Encode writes the file as toml::to_string_pretty does.
func (a *Apps) Encode() string {
	root := config.NewTable()
	list := make([]any, 0, len(a.Apps))
	for _, r := range a.Apps {
		t := config.NewTable()
		t.Set("name", r.Name)
		for _, kv := range []struct {
			k string
			v *string
		}{{"team_id", r.TeamID}, {"identifier", r.Identifier}, {"signer", r.Signer}, {"path", r.Path}, {"flatpak", r.Flatpak}} {
			if kv.v != nil {
				t.Set(kv.k, *kv.v)
			}
		}
		t.Set("allowed", r.Allowed)
		t.Set("granted", stringsAny(r.Granted))
		t.Set("denied", stringsAny(r.Denied))
		if r.FirstSeen != nil {
			t.Set("first_seen", r.FirstSeen)
		}
		list = append(list, t)
	}
	root.Set("apps", list)
	return root.Encode()
}

func stringsAny(s []string) []any {
	out := make([]any, len(s))
	for i, v := range s {
		out[i] = v
	}
	return out
}

// AppsFile reads apps.toml again when it changes, and writes it whole.
type AppsFile struct {
	path     string
	mu       sync.Mutex
	bytes    []byte
	present  bool
	apps     *Apps
	broken   bool
	unread   bool
	write    sync.Mutex
	observer func()
	obsMu    sync.Mutex
}

// NewAppsFile is the file at path. It is read on first use.
func NewAppsFile(path string) *AppsFile {
	return &AppsFile{path: path, apps: &Apps{}, unread: true}
}

// Path is the path of the file.
func (f *AppsFile) Path() string { return f.path }

// Current is the file as it is now. A missing or broken file has no apps.
// Do not change what it gives.
func (f *AppsFile) Current() *Apps { a, _ := f.load(); return a }

// IsBroken is true when the file cannot be read or does not parse: then no
// app can connect, and the host never writes over it.
func (f *AppsFile) IsBroken() bool { _, b := f.load(); return b }

func (f *AppsFile) load() (*Apps, bool) {
	b, err := os.ReadFile(f.path)
	present := true
	if err != nil {
		if errors.Is(err, fs.ErrNotExist) {
			present, b = false, nil
		} else {
			slog.Warn("host: cannot read apps file", "path", f.path, "err", err)
			f.mu.Lock()
			defer f.mu.Unlock()
			f.bytes, f.present, f.apps, f.broken, f.unread = nil, false, &Apps{}, true, true
			return f.apps, true
		}
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if !f.unread && f.present == present && bytes.Equal(f.bytes, b) {
		return f.apps, f.broken
	}
	apps, broken := &Apps{}, false
	if present {
		if !utf8.Valid(b) {
			slog.Warn("host: apps file is broken, so no app can connect", "path", f.path, "err", "not UTF-8")
			broken = true
		} else if parsed, err := ParseApps(string(b)); err != nil {
			slog.Warn("host: apps file is broken, so no app can connect", "path", f.path, "err", err)
			broken = true
		} else {
			apps = parsed
		}
	}
	f.bytes, f.present, f.apps, f.broken, f.unread = b, present, apps, broken, false
	return apps, broken
}

// SetObserver runs observer after each change that the host writes.
func (f *AppsFile) SetObserver(observer func()) {
	f.obsMu.Lock()
	f.observer = observer
	f.obsMu.Unlock()
}

func (f *AppsFile) update(change func(*Apps)) error {
	if err := func() error {
		f.write.Lock()
		defer f.write.Unlock()
		current, broken := f.load()
		if broken {
			return fmt.Errorf("%s does not parse", f.path)
		}
		next := current.clone()
		change(next)
		if reflect.DeepEqual(next, current) {
			return errUnchanged
		}
		text := next.Encode()
		dir := filepath.Dir(f.path)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return err
		}
		tmp := filepath.Join(dir, trimTomlExt(filepath.Base(f.path))+fmt.Sprintf(".toml.%d.tmp", os.Getpid()))
		if err := writeSynced(tmp, []byte(text)); err != nil {
			_ = os.Remove(tmp)
			return err
		}
		if err := os.Rename(tmp, f.path); err != nil {
			_ = os.Remove(tmp)
			return err
		}
		return syncFolder(dir)
	}(); err != nil {
		if err == errUnchanged {
			return nil
		}
		return err
	}
	f.obsMu.Lock()
	observer := f.observer
	f.obsMu.Unlock()
	if observer != nil {
		observer()
	}
	return nil
}

var errUnchanged = errors.New("unchanged")

// trimTomlExt is the file stem as with_extension sees it.
func trimTomlExt(name string) string {
	if ext := filepath.Ext(name); ext != "" && ext != name {
		return name[:len(name)-len(ext)]
	}
	return name
}

func writeSynced(path string, b []byte) error {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	if _, err := f.Write(b); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}

func strp(s string) *string { return &s }

// answerFirst keeps the first answer of the user about an app.
func (f *AppsFile) answerFirst(caller Caller, caps []Capability, allow bool) error {
	names := make([]string, len(caps))
	for i, c := range caps {
		names[i] = string(c)
	}
	r := AppRecord{Name: caller.Name, Allowed: allow, Granted: []string{}, Denied: []string{}, FirstSeen: time.Now().UTC().Truncate(time.Second)}
	if caller.Signer != "" {
		r.Signer = strp(caller.Signer)
	}
	switch caller.Identity.Kind {
	case IdentitySigned:
		r.TeamID, r.Identifier = strp(caller.Identity.TeamID), strp(caller.Identity.Identifier)
	case IdentityPath:
		r.Path = strp(caller.Identity.Path)
	case IdentityFlatpak:
		r.Flatpak = strp(caller.Identity.AppID)
	}
	if allow {
		r.Granted = names
	} else {
		r.Denied = names
	}
	return f.update(func(a *Apps) {
		if a.Find(caller.Identity) == nil {
			a.Apps = append(a.Apps, r)
		}
	})
}

// answerNew keeps the answer about capabilities that the app asked for
// after the first time.
func (f *AppsFile) answerNew(id string, caps []Capability, allow bool) error {
	return f.update(func(a *Apps) {
		r := a.Record(id)
		if r == nil {
			return
		}
		for _, c := range caps {
			if !r.hasAnswer(c) {
				if allow {
					r.Granted = append(r.Granted, string(c))
				} else {
					r.Denied = append(r.Denied, string(c))
				}
			}
		}
	})
}

// SetGrant turns one capability of an app on or off (Settings).
func (f *AppsFile) SetGrant(id string, c Capability, on bool) error {
	return f.update(func(a *Apps) {
		r := a.Record(id)
		if r == nil {
			return
		}
		r.Granted = slices.DeleteFunc(r.Granted, func(n string) bool { return n == string(c) })
		r.Denied = slices.DeleteFunc(r.Denied, func(n string) bool { return n == string(c) })
		if on {
			r.Granted = append(r.Granted, string(c))
		} else {
			r.Denied = append(r.Denied, string(c))
		}
	})
}

// Allow allows an app that the user refused, with each capability that it
// asked for (Settings).
func (f *AppsFile) Allow(id string) error {
	return f.update(func(a *Apps) {
		r := a.Record(id)
		if r == nil {
			return
		}
		r.Allowed = true
		r.Granted = append(r.Granted, r.Denied...)
		r.Denied = []string{}
	})
}

// Remove forgets an app (Settings).
func (f *AppsFile) Remove(id string) error {
	return f.update(func(a *Apps) {
		a.Apps = slices.DeleteFunc(a.Apps, func(r AppRecord) bool { return r.ID() == id })
	})
}
