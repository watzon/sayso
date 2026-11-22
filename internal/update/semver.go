package update

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
)

// Semver is a semantic version, as the semver crate parses it.
type Semver struct {
	Major, Minor, Patch uint64
	Pre                 []string // pre-release identifiers
	Build               string
}

// ParseVersion reads "1.2.3", "1.2.3-beta.1", or "1.2.3+build".
func ParseVersion(s string) (Semver, error) {
	var v Semver
	bad := fmt.Errorf("unexpected version %q", s)
	rest := s
	if i := strings.IndexByte(rest, '+'); i >= 0 {
		v.Build = rest[i+1:]
		rest = rest[:i]
		if v.Build == "" || !idents(v.Build, false) {
			return v, bad
		}
	}
	if i := strings.IndexByte(rest, '-'); i >= 0 {
		pre := rest[i+1:]
		rest = rest[:i]
		if pre == "" || !idents(pre, true) {
			return v, bad
		}
		v.Pre = strings.Split(pre, ".")
	}
	parts := strings.Split(rest, ".")
	if len(parts) != 3 {
		return v, bad
	}
	nums := []*uint64{&v.Major, &v.Minor, &v.Patch}
	for i, p := range parts {
		if p == "" || (len(p) > 1 && p[0] == '0') || strings.Trim(p, "0123456789") != "" {
			return v, bad
		}
		n, err := strconv.ParseUint(p, 10, 64)
		if err != nil {
			return v, bad
		}
		*nums[i] = n
	}
	return v, nil
}

func idents(s string, noLeadingZero bool) bool {
	for _, id := range strings.Split(s, ".") {
		if id == "" {
			return false
		}
		for _, c := range id {
			if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c == '-') {
				return false
			}
		}
		if noLeadingZero && len(id) > 1 && id[0] == '0' && strings.Trim(id, "0123456789") == "" {
			return false
		}
	}
	return true
}

// MustVersion parses a version known to be valid.
func MustVersion(s string) Semver {
	v, err := ParseVersion(s)
	if err != nil {
		panic(err)
	}
	return v
}

func (v Semver) String() string {
	s := fmt.Sprintf("%d.%d.%d", v.Major, v.Minor, v.Patch)
	if len(v.Pre) > 0 {
		s += "-" + strings.Join(v.Pre, ".")
	}
	if v.Build != "" {
		s += "+" + v.Build
	}
	return s
}

// Equal compares every part, build metadata included (Rust's ==).
func (v Semver) Equal(o Semver) bool { return v.String() == o.String() }

func cmpU(a, b uint64) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	}
	return 0
}

// CmpPrecedence orders versions by semver precedence (build ignored).
func (v Semver) CmpPrecedence(o Semver) int {
	for _, c := range []int{cmpU(v.Major, o.Major), cmpU(v.Minor, o.Minor), cmpU(v.Patch, o.Patch)} {
		if c != 0 {
			return c
		}
	}
	switch {
	case len(v.Pre) == 0 && len(o.Pre) == 0:
		return 0
	case len(v.Pre) == 0:
		return 1
	case len(o.Pre) == 0:
		return -1
	}
	for i := 0; i < len(v.Pre) && i < len(o.Pre); i++ {
		a, b := v.Pre[i], o.Pre[i]
		an, aerr := strconv.ParseUint(a, 10, 64)
		bn, berr := strconv.ParseUint(b, 10, 64)
		var c int
		switch {
		case aerr == nil && berr == nil:
			c = cmpU(an, bn)
		case aerr == nil:
			c = -1
		case berr == nil:
			c = 1
		default:
			c = strings.Compare(a, b)
		}
		if c != 0 {
			return c
		}
	}
	return cmpU(uint64(len(v.Pre)), uint64(len(o.Pre)))
}

func (v Semver) MarshalJSON() ([]byte, error) { return json.Marshal(v.String()) }

func (v *Semver) UnmarshalJSON(b []byte) error {
	var s string
	if err := json.Unmarshal(b, &s); err != nil {
		return err
	}
	p, err := ParseVersion(s)
	if err != nil {
		return err
	}
	*v = p
	return nil
}
