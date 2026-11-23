// Package journal is the append-only text file the mobile core keeps its
// state in: the learner's typing journal and the voice store. One record
// per line, fields separated by tabs, with \ escaping tab, newline, and
// itself. The first field names the record; what records mean is up to the
// owner, who also decides when to compact (Rewrite) the file.
package journal

import (
	"bufio"
	"errors"
	"io"
	"os"
	"strings"
)

// header is the first record of every journal.
const header = "V\t1"

// File is an open journal, appended to from the end.
type File struct {
	path string
	f    *os.File
	size int64
}

func escape(s string) string {
	if !strings.ContainsAny(s, "\\\t\n\r") {
		return s
	}
	r := strings.NewReplacer("\\", `\\`, "\t", `\t`, "\n", `\n`, "\r", `\r`)
	return r.Replace(s)
}

func unescape(s string) string {
	if !strings.Contains(s, "\\") {
		return s
	}
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		if s[i] == '\\' && i+1 < len(s) {
			i++
			switch s[i] {
			case 't':
				b.WriteByte('\t')
			case 'n':
				b.WriteByte('\n')
			case 'r':
				b.WriteByte('\r')
			default:
				b.WriteByte(s[i])
			}
			continue
		}
		b.WriteByte(s[i])
	}
	return b.String()
}

// Record encodes one record as a line. It escapes fields in place.
func Record(fields ...string) string {
	for i, f := range fields {
		fields[i] = escape(f)
	}
	return strings.Join(fields, "\t") + "\n"
}

// Read calls fn for each record of the file at path, in order, the header
// included. A missing file has no records; a torn last line (a crash
// mid-write) is skipped.
func Read(path string, fn func(fields []string)) error {
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	defer f.Close()
	r := bufio.NewReader(f)
	for {
		line, err := r.ReadString('\n')
		if err == io.EOF {
			return nil // a line without its newline is torn: skip it
		}
		if err != nil {
			return err
		}
		line = strings.TrimSuffix(line, "\n")
		if line == "" {
			continue
		}
		fields := strings.Split(line, "\t")
		for i := range fields {
			fields[i] = unescape(fields[i])
		}
		fn(fields)
	}
}

// Open opens the journal at path for appending, creating it with its
// header when it is missing.
func Open(path string) (*File, error) {
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil, err
	}
	st, err := f.Stat()
	if err != nil {
		f.Close()
		return nil, err
	}
	j := &File{path: path, f: f, size: st.Size()}
	if j.size == 0 {
		if err := j.Append(header + "\n"); err != nil {
			f.Close()
			return nil, err
		}
	}
	return j, nil
}

// Append writes one encoded record (see Record).
func (j *File) Append(line string) error {
	n, err := j.f.WriteString(line)
	j.size += int64(n)
	return err
}

// Rewrite replaces the file with the header and lines, atomically.
func (j *File) Rewrite(lines []string) error {
	tmp := j.path + ".tmp"
	f, err := os.OpenFile(tmp, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o600)
	if err != nil {
		return err
	}
	w := bufio.NewWriter(f)
	w.WriteString(header + "\n")
	for _, l := range lines {
		w.WriteString(l)
	}
	if err := w.Flush(); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmp, j.path); err != nil {
		return err
	}
	j.f.Close()
	nf, err := os.OpenFile(j.path, os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	st, err := nf.Stat()
	if err != nil {
		nf.Close()
		return err
	}
	j.f, j.size = nf, st.Size()
	return nil
}

// Size is the file's length in bytes.
func (j *File) Size() int64 { return j.size }

// Close closes the file.
func (j *File) Close() error { return j.f.Close() }
