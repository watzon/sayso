package config

import (
	"fmt"
	"math"
	"sort"
	"strconv"
	"strings"
	"time"

	toml "github.com/pelletier/go-toml/v2"
)

// The TOML layer. Reading goes through go-toml into plain values, then each
// section reads its keys with serde's rules: a missing key keeps the default,
// an unknown key is ignored, a wrong type fails the section. Writing builds an
// ordered table and prints it the way toml-rs `to_string_pretty` does: plain
// values first, then sub-tables and arrays of tables, in field order.

// Table is an ordered TOML table for writing.
type Table struct {
	keys []string
	vals map[string]any
}

func newTable() *Table { return &Table{vals: map[string]any{}} }

// Set adds or replaces a key, keeping the first position.
func (t *Table) Set(key string, v any) *Table {
	if _, ok := t.vals[key]; !ok {
		t.keys = append(t.keys, key)
	}
	t.vals[key] = v
	return t
}

// tableFromMap makes a Table of a decoded value, keys sorted, for values that
// are written back as they were read (an unknown provider, a bad style hotkey).
func tableFromMap(m map[string]any) *Table {
	t := newTable()
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		t.Set(k, fromDecoded(m[k]))
	}
	return t
}

func fromDecoded(v any) any {
	switch x := v.(type) {
	case map[string]any:
		return tableFromMap(x)
	case []any:
		out := make([]any, len(x))
		for i, e := range x {
			out[i] = fromDecoded(e)
		}
		return out
	}
	return v
}

func isTableArray(v any) bool {
	arr, ok := v.([]any)
	if !ok || len(arr) == 0 {
		return false
	}
	for _, e := range arr {
		if _, ok := e.(*Table); !ok {
			return false
		}
	}
	return true
}

// Encode prints a root table.
func (t *Table) Encode() string {
	var b strings.Builder
	t.write(&b, "", true)
	return b.String()
}

func (t *Table) write(b *strings.Builder, path string, root bool) {
	wroteHeader := false
	header := func() {
		if wroteHeader || root {
			return
		}
		if b.Len() > 0 {
			b.WriteString("\n")
		}
		b.WriteString("[" + path + "]\n")
		wroteHeader = true
	}
	hasPlain := false
	for _, k := range t.keys {
		v := t.vals[k]
		if _, sub := v.(*Table); sub || isTableArray(v) {
			continue
		}
		hasPlain = true
		header()
		b.WriteString(quoteKey(k) + " = " + formatTopValue(v) + "\n")
	}
	if !hasPlain {
		// A table with only sub-tables gets no header of its own, unless it is empty.
		empty := true
		for _, k := range t.keys {
			if sub, ok := t.vals[k].(*Table); ok || isTableArray(t.vals[k]) {
				_ = sub
				empty = false
			}
		}
		if empty {
			header()
		}
	}
	for _, k := range t.keys {
		v := t.vals[k]
		child := joinPath(path, k)
		switch x := v.(type) {
		case *Table:
			x.write(b, child, false)
		case []any:
			if !isTableArray(x) {
				continue
			}
			for _, e := range x {
				if b.Len() > 0 {
					b.WriteString("\n")
				}
				b.WriteString("[[" + child + "]]\n")
				e.(*Table).writeBody(b, child)
			}
		}
	}
}

// writeBody prints the values of an element of an array of tables, then its sub-tables.
func (t *Table) writeBody(b *strings.Builder, path string) {
	for _, k := range t.keys {
		v := t.vals[k]
		if _, sub := v.(*Table); sub || isTableArray(v) {
			continue
		}
		b.WriteString(quoteKey(k) + " = " + formatTopValue(v) + "\n")
	}
	for _, k := range t.keys {
		v := t.vals[k]
		child := joinPath(path, k)
		switch x := v.(type) {
		case *Table:
			x.write(b, child, false)
		case []any:
			if isTableArray(x) {
				for _, e := range x {
					b.WriteString("\n[[" + child + "]]\n")
					e.(*Table).writeBody(b, child)
				}
			}
		}
	}
}

func joinPath(path, key string) string {
	if path == "" {
		return quoteKey(key)
	}
	return path + "." + quoteKey(key)
}

// quoteKey writes a key as toml_writer's TomlKeyBuilder::as_default does:
// bare, else a basic string without escapes, else a literal string, else a
// basic string with escapes.
func quoteKey(k string) string {
	bare := k != ""
	single, double, escape, codes := false, false, false, false
	for i := 0; i < len(k); i++ {
		c := k[i]
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '_') {
			bare = false
		}
		switch {
		case c == '\'':
			single = true
		case c == '"':
			double = true
		case c == '\\':
			escape = true
		case c == '\t':
		case c <= 0x1f || c == 0x7f:
			codes = true
		}
	}
	switch {
	case bare:
		return k
	case !codes && !escape && !double:
		return writeTomlString(k, encBasic, false)
	case !codes && !single:
		return writeTomlString(k, encLiteral, false)
	}
	return writeTomlString(k, encBasic, false)
}

type tomlEncoding int

const (
	encLiteral tomlEncoding = iota
	encBasic
	encMlLiteral
	encMlBasic
)

// quoteString writes a string value as toml_writer's
// TomlStringBuilder::as_default does (the writer toml-rs 0.9 uses): a basic
// string when no escape is needed, else a literal string, else a multi-line
// basic or literal string, else an escaped basic string.
func quoteString(s string) string {
	var maxSingle, maxDouble, single, double int
	escape, codes, newline := false, false, false
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c == '\'' {
			single++
			maxSingle = max(maxSingle, single)
		} else {
			single = 0
		}
		if c == '"' {
			double++
			maxDouble = max(maxDouble, double)
		} else {
			double = 0
		}
		switch {
		case c == '\\':
			escape = true
		case c == '\t':
		case c == '\n':
			newline = true
		case c <= 0x1f || c == 0x7f:
			codes = true
		}
	}
	switch {
	case !codes && !escape && maxDouble == 0 && !newline:
		return writeTomlString(s, encBasic, newline)
	case !codes && maxSingle == 0 && !newline:
		return writeTomlString(s, encLiteral, newline)
	case !codes && !escape && maxDouble <= 2:
		return writeTomlString(s, encMlBasic, newline)
	case !codes && maxSingle <= 2:
		return writeTomlString(s, encMlLiteral, newline)
	case newline:
		return writeTomlString(s, encMlBasic, newline)
	}
	return writeTomlString(s, encBasic, newline)
}

func writeTomlString(s string, enc tomlEncoding, newline bool) string {
	delim := map[tomlEncoding]string{encLiteral: "'", encBasic: `"`, encMlLiteral: "'''", encMlBasic: `"""`}[enc]
	ml := enc == encMlLiteral || enc == encMlBasic
	var b strings.Builder
	b.WriteString(delim)
	if newline && ml {
		b.WriteByte('\n')
	}
	if enc == encLiteral || enc == encMlLiteral {
		b.WriteString(s)
		b.WriteString(delim)
		return b.String()
	}
	maxDouble := 0
	if ml {
		maxDouble = 2
	}
	stream := s
	for stream != "" {
		end, esc, seq := 0, "", 0
	scan:
		for i := 0; i < len(stream); i++ {
			c := stream[i]
			if c == '"' {
				seq++
				if seq > maxDouble {
					esc = `\"`
					break
				}
			} else {
				seq = 0
			}
			switch {
			case c == 0x8:
				esc = `\b`
				break scan
			case c == 0x9:
				esc = `\t`
				break scan
			case c == 0xa:
				if !ml {
					esc = `\n`
					break scan
				}
			case c == 0xc:
				esc = `\f`
				break scan
			case c == 0xd:
				esc = `\r`
				break scan
			case c == '"':
			case c == '\\':
				esc = `\\`
				break scan
			case c <= 0x1f || c == 0x7f:
				break scan
			}
			end = i + 1
		}
		b.WriteString(stream[:end])
		b.WriteString(esc)
		if esc != "" {
			stream = stream[end+1:]
		} else {
			stream = stream[end:]
			if stream != "" {
				fmt.Fprintf(&b, `\u%04X`, stream[0])
				stream = stream[1:]
			}
		}
	}
	b.WriteString(delim)
	return b.String()
}

// f32 marks a float that Rust holds as f32, so it prints in its shortest f32 form.
type f32 float32

func formatFloat(v float64, bits int) string {
	switch {
	case math.IsNaN(v):
		return "nan"
	case math.IsInf(v, 1):
		return "inf"
	case math.IsInf(v, -1):
		return "-inf"
	}
	s := strconv.FormatFloat(v, 'g', -1, bits)
	if strings.ContainsAny(s, "e") {
		// toml-rs prints the exponent form with a decimal point: 1e20 → 1e20.
		return s
	}
	if !strings.Contains(s, ".") {
		s += ".0"
	}
	return s
}

// formatTopValue is the value of a key line: toml-rs pretty prints an array
// of more than one element with one element per line.
func formatTopValue(v any) string {
	if arr, ok := v.([]any); ok && len(arr) > 1 {
		var b strings.Builder
		b.WriteString("[\n")
		for _, e := range arr {
			b.WriteString("    " + formatValue(e) + ",\n")
		}
		b.WriteString("]")
		return b.String()
	}
	return formatValue(v)
}

func formatValue(v any) string {
	switch x := v.(type) {
	case string:
		return quoteString(x)
	case bool:
		return strconv.FormatBool(x)
	case int:
		return strconv.Itoa(x)
	case int64:
		return strconv.FormatInt(x, 10)
	case uint64:
		return strconv.FormatUint(x, 10)
	case uint32:
		return strconv.FormatUint(uint64(x), 10)
	case uint8:
		return strconv.FormatUint(uint64(x), 10)
	case f32:
		return formatFloat(float64(x), 32)
	case float64:
		return formatFloat(x, 64)
	case []string:
		parts := make([]any, len(x))
		for i, s := range x {
			parts[i] = s
		}
		return formatValue(parts)
	case []any:
		if len(x) == 0 {
			return "[]"
		}
		parts := make([]string, len(x))
		for i, e := range x {
			parts[i] = formatValue(e)
		}
		return "[" + strings.Join(parts, ", ") + "]"
	case *Table:
		parts := make([]string, 0, len(x.keys))
		for _, k := range x.keys {
			parts = append(parts, quoteKey(k)+" = "+formatValue(x.vals[k]))
		}
		if len(parts) == 0 {
			return "{}"
		}
		return "{ " + strings.Join(parts, ", ") + " }"
	case toml.LocalDate, toml.LocalDateTime, toml.LocalTime:
		return fmt.Sprint(x)
	case time.Time:
		return x.Format(time.RFC3339Nano)
	}
	return quoteString(fmt.Sprint(v))
}

// parseDocument reads TOML text into plain values.
func parseDocument(text string) (map[string]any, error) {
	var doc map[string]any
	if err := toml.Unmarshal([]byte(text), &doc); err != nil {
		return nil, err
	}
	if doc == nil {
		doc = map[string]any{}
	}
	return doc, nil
}

// reader reads the keys of one table with serde's rules. The first error
// stops it; the caller then keeps the defaults of the whole section.
type reader struct {
	m   map[string]any
	err error
}

func newReader(v any) *reader {
	m, ok := v.(map[string]any)
	r := &reader{m: m}
	if !ok {
		r.err = fmt.Errorf("invalid type: %s, expected a table", describe(v))
	}
	return r
}

func describe(v any) string {
	switch x := v.(type) {
	case string:
		return fmt.Sprintf("string %q", x)
	case int64:
		return fmt.Sprintf("integer `%d`", x)
	case float64:
		return fmt.Sprintf("floating point `%v`", x)
	case bool:
		return fmt.Sprintf("boolean `%v`", x)
	case []any:
		return "sequence"
	case map[string]any:
		return "map"
	}
	return fmt.Sprintf("%T", v)
}

func (r *reader) fail(key string, err error) {
	if r.err == nil {
		r.err = err
	}
}

func (r *reader) get(key string) (any, bool) {
	if r.err != nil || r.m == nil {
		return nil, false
	}
	v, ok := r.m[key]
	return v, ok
}

func (r *reader) str(key string, dst *string) {
	if v, ok := r.get(key); ok {
		s, isStr := v.(string)
		if !isStr {
			r.fail(key, fmt.Errorf("invalid type: %s, expected a string", describe(v)))
			return
		}
		*dst = s
	}
}

func (r *reader) optStr(key string, dst **string) {
	if v, ok := r.get(key); ok {
		s, isStr := v.(string)
		if !isStr {
			r.fail(key, fmt.Errorf("invalid type: %s, expected a string", describe(v)))
			return
		}
		*dst = &s
	}
}

func (r *reader) boolean(key string, dst *bool) {
	if v, ok := r.get(key); ok {
		b, isBool := v.(bool)
		if !isBool {
			r.fail(key, fmt.Errorf("invalid type: %s, expected a boolean", describe(v)))
			return
		}
		*dst = b
	}
}

func (r *reader) optBool(key string, dst **bool) {
	var b bool
	if _, ok := r.get(key); ok {
		r.boolean(key, &b)
		if r.err == nil {
			*dst = &b
		}
	}
}

func (r *reader) uint(key string, max uint64, what string) (uint64, bool) {
	v, ok := r.get(key)
	if !ok {
		return 0, false
	}
	n, isInt := v.(int64)
	if !isInt {
		r.fail(key, fmt.Errorf("invalid type: %s, expected %s", describe(v), what))
		return 0, false
	}
	if n < 0 || uint64(n) > max {
		r.fail(key, fmt.Errorf("invalid value: integer `%d`, expected %s", n, what))
		return 0, false
	}
	return uint64(n), true
}

func (r *reader) u64(key string, dst *uint64) {
	if n, ok := r.uint(key, math.MaxUint64, "u64"); ok {
		*dst = n
	}
}

func (r *reader) u32(key string, dst *uint32) {
	if n, ok := r.uint(key, math.MaxUint32, "u32"); ok {
		*dst = uint32(n)
	}
}

func (r *reader) optU32(key string, dst **uint32) {
	if n, ok := r.uint(key, math.MaxUint32, "u32"); ok {
		v := uint32(n)
		*dst = &v
	}
}

func (r *reader) u8(key string, dst *uint8) {
	if n, ok := r.uint(key, math.MaxUint8, "u8"); ok {
		*dst = uint8(n)
	}
}

func (r *reader) f32(key string, dst *float32) {
	v, ok := r.get(key)
	if !ok {
		return
	}
	switch x := v.(type) {
	case float64:
		*dst = float32(x)
	case int64:
		*dst = float32(x)
	default:
		r.fail(key, fmt.Errorf("invalid type: %s, expected f32", describe(v)))
	}
}

func (r *reader) strings(key string, dst *[]string) {
	v, ok := r.get(key)
	if !ok {
		return
	}
	arr, isArr := v.([]any)
	if !isArr {
		r.fail(key, fmt.Errorf("invalid type: %s, expected a sequence", describe(v)))
		return
	}
	out := make([]string, 0, len(arr))
	for _, e := range arr {
		s, isStr := e.(string)
		if !isStr {
			r.fail(key, fmt.Errorf("invalid type: %s, expected a string", describe(e)))
			return
		}
		out = append(out, s)
	}
	*dst = out
}

func (r *reader) stringMap(key string, dst *map[string]string) {
	v, ok := r.get(key)
	if !ok {
		return
	}
	m, isMap := v.(map[string]any)
	if !isMap {
		r.fail(key, fmt.Errorf("invalid type: %s, expected a map", describe(v)))
		return
	}
	out := map[string]string{}
	for k, e := range m {
		s, isStr := e.(string)
		if !isStr {
			r.fail(key, fmt.Errorf("invalid type: %s, expected a string", describe(e)))
			return
		}
		out[k] = s
	}
	*dst = out
}

// enum reads a snake_case variant name.
func (r *reader) enum(key string, variants []string, set func(i int)) {
	v, ok := r.get(key)
	if !ok {
		return
	}
	s, isStr := v.(string)
	if !isStr {
		r.fail(key, fmt.Errorf("invalid type: %s, expected a string", describe(v)))
		return
	}
	for i, name := range variants {
		if name == s {
			set(i)
			return
		}
	}
	quoted := make([]string, len(variants))
	for i, name := range variants {
		quoted[i] = "`" + name + "`"
	}
	r.fail(key, fmt.Errorf("unknown variant `%s`, expected one of %s", s, strings.Join(quoted, ", ")))
}

// tables reads an array of tables.
func (r *reader) tables(key string) []any {
	v, ok := r.get(key)
	if !ok {
		return nil
	}
	arr, isArr := v.([]any)
	if !isArr {
		r.fail(key, fmt.Errorf("invalid type: %s, expected a sequence", describe(v)))
		return nil
	}
	return arr
}

func (r *reader) table(key string) (any, bool) { return r.get(key) }

// sorted keys of a map.
func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func stringsToAny(s []string) []any {
	out := make([]any, len(s))
	for i, v := range s {
		out[i] = v
	}
	return out
}

// NewTable is an empty TOML table that writes as toml-rs's to_string_pretty
// does. Other packages use it for files that must match the Rust app.
func NewTable() *Table { return newTable() }

// F32 marks a float that the Rust app holds as f32, so it prints in its shortest f32 form.
type F32 = f32

// ParseTOML reads TOML text into plain values.
func ParseTOML(text string) (map[string]any, error) { return parseDocument(text) }
