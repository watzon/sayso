package bridge

import (
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"unicode"
	"unicode/utf8"
)

// Peer is the process at the other end of a connection: the audit token on
// macOS (pid in Token[5], pid version in Token[7]), the pid elsewhere.
type Peer struct {
	PID   uint32
	Token [8]uint32
}

// SameProcess is true when other is the same process, also after a reuse
// of its pid on macOS.
func (p Peer) SameProcess(other Peer) bool {
	if runtime.GOOS == "darwin" {
		return p.Token[5] == other.Token[5] && p.Token[7] == other.Token[7]
	}
	return p.PID == other.PID
}

// IdentityKind is the kind of an Identity.
type IdentityKind int

const (
	// IdentitySigned is a signed macOS app: team id and identifier.
	IdentitySigned IdentityKind = iota
	// IdentityPath is the canonical path of the executable.
	IdentityPath
	// IdentityFlatpak is the id of a Flatpak app.
	IdentityFlatpak
)

// Identity is what the host knows of an app for sure.
type Identity struct {
	Kind       IdentityKind
	TeamID     string
	Identifier string
	Path       string
	AppID      string
}

// ID is the key of the identity in apps.toml and the host.
func (i Identity) ID() string {
	switch i.Kind {
	case IdentitySigned:
		return "signed:" + i.TeamID + "/" + i.Identifier
	case IdentityFlatpak:
		return "flatpak:" + i.AppID
	}
	return pathID(i.Path, runtime.GOOS == "windows")
}

func pathID(path string, windows bool) string {
	if !utf8.ValidString(path) {
		return "bytes:" + escapeASCII(path)
	}
	if windows {
		return "path:" + strings.ToLower(path)
	}
	return "path:" + path
}

// escapeASCII is the escape_ascii of Rust for the bytes of a path.
func escapeASCII(s string) string {
	var b strings.Builder
	const hex = "0123456789abcdef"
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c == '\t':
			b.WriteString(`\t`)
		case c == '\r':
			b.WriteString(`\r`)
		case c == '\n':
			b.WriteString(`\n`)
		case c == '\\':
			b.WriteString(`\\`)
		case c == '\'':
			b.WriteString(`\'`)
		case c == '"':
			b.WriteString(`\"`)
		case c >= 0x20 && c < 0x7f:
			b.WriteByte(c)
		default:
			b.WriteString(`\x`)
			b.WriteByte(hex[c>>4])
			b.WriteByte(hex[c&15])
		}
	}
	return b.String()
}

// Caller is a program that connected, as the system shows it.
type Caller struct {
	Identity    Identity
	Name        string
	Signer      string
	Interpreter bool
}

// CallerFromPath is the caller of an executable without a signature.
func CallerFromPath(executable string) Caller {
	return Caller{Name: systemName(executable), Interpreter: isInterpreter(executable), Identity: Identity{Kind: IdentityPath, Path: executable}}
}

// CallerFromFlatpak is the caller of a Flatpak app.
func CallerFromFlatpak(appID string) Caller {
	name := "?"
	if i := strings.LastIndexByte(appID, '.'); i >= 0 {
		if n, ok := CleanName(appID[i+1:]); ok {
			name = n
		}
	} else if n, ok := CleanName(appID); ok {
		name = n
	}
	return Caller{Name: name, Identity: Identity{Kind: IdentityFlatpak, AppID: appID}}
}

// Identify finds the caller of a peer.
type Identify interface {
	Identify(peer Peer) (Caller, error)
}

// SystemIdentify asks the system.
type SystemIdentify struct{}

func (SystemIdentify) Identify(peer Peer) (Caller, error) { return systemIdentify(peer) }

const maxName = 40

func hidden(c rune) bool {
	if unicode.IsControl(c) {
		return true
	}
	in := func(lo, hi rune) bool { return c >= lo && c <= hi }
	switch {
	case c == 0x00AD, in(0x0600, 0x0605), c == 0x061C, c == 0x06DD, c == 0x070F, in(0x0890, 0x0891), c == 0x08E2, c == 0x180E,
		in(0x200B, 0x200F), in(0x202A, 0x202E), in(0x2060, 0x206F), c == 0xFEFF, in(0xFFF9, 0xFFFB),
		c == 0x110BD, c == 0x110CD, in(0x13430, 0x1343F), in(0x1BCA0, 0x1BCA3), in(0x1D173, 0x1D17A), c == 0xE0001, in(0xE0020, 0xE007F),
		c == 0x2028, c == 0x2029,
		c == 0x034F, c == 0x115F, c == 0x1160, c == 0x17B4, c == 0x17B5, in(0x180B, 0x180D), c == 0x180F, c == 0x2800, c == 0x3164, in(0xFE00, 0xFE0F), c == 0xFFA0, in(0xE0100, 0xE01EF):
		return true
	}
	return false
}

func withoutHidden(text string) string {
	var b strings.Builder
	for _, c := range text {
		if !hidden(c) {
			b.WriteRune(c)
		}
	}
	return b.String()
}

// CleanName is a name without the characters that can mislead, at most 40
// characters, or false when nothing is left.
func CleanName(name string) (string, bool) {
	cleaned := strings.TrimSpace(withoutHidden(name))
	r := []rune(cleaned)
	if len(r) > maxName {
		r = r[:maxName]
	}
	cleaned = strings.TrimRightFunc(string(r), unicode.IsSpace)
	return cleaned, cleaned != ""
}

// VisibleText shows each character that can mislead as U+FFFD.
func VisibleText(text string) string {
	var b strings.Builder
	for _, c := range text {
		if hidden(c) {
			c = unicode.ReplacementChar
		}
		b.WriteRune(c)
	}
	return b.String()
}

// VisiblePath is VisibleText of a path.
func VisiblePath(path string) string { return VisibleText(strings.ToValidUTF8(path, "�")) }

func fileName(executable string, windows bool) string {
	name := baseName(executable, windows)
	if windows {
		if ext := filepath.Ext(name); ext != "" && ext != name {
			name = strings.TrimSuffix(name, ext)
		}
	}
	if n, ok := CleanName(strings.ToValidUTF8(name, "�")); ok {
		return n
	}
	return "?"
}

func baseName(path string, windows bool) string {
	cut := func(r rune) bool { return r == '/' || (windows && r == '\\') }
	path = strings.TrimRightFunc(path, cut)
	if i := strings.LastIndexFunc(path, cut); i >= 0 {
		path = path[i+1:]
	}
	if path == ".." || path == "." {
		return ""
	}
	return path
}

var interpreters = []string{"awk", "bash", "bun", "busybox", "cmd", "cscript", "dash", "deno", "dotnet", "fish", "gawk", "java", "javaw", "julia", "ksh", "lua", "mawk", "node", "nodejs", "osascript", "perl", "php", "powershell", "pwsh", "pypy", "python", "pythonw", "r", "rscript", "ruby", "sh", "tclsh", "wish", "wscript", "zsh"}

func isInterpreter(executable string) bool {
	stem := strings.ToLower(baseName(executable, runtime.GOOS == "windows"))
	if stem == "" {
		return false
	}
	if strings.HasPrefix(stem, "ld-linux") || strings.HasPrefix(stem, "ld-musl") || stem == "ld.so" {
		return true
	}
	stem = strings.TrimSuffix(stem, ".exe")
	stem = strings.TrimSuffix(stem, "-intel64")
	stem = strings.TrimRightFunc(stem, func(c rune) bool { return (c >= '0' && c <= '9') || c == '.' || c == '-' })
	return slices.Contains(interpreters, stem)
}

func signerName(summary, teamID string) (string, bool) {
	name := summary
	if _, after, ok := strings.Cut(summary, ": "); ok {
		name = after
	}
	name = strings.TrimSuffix(name, " ("+teamID+")")
	return CleanName(name)
}
