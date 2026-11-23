package main

import (
	"archive/tar"
	"bufio"
	"compress/gzip"
	"io"
	"os"
	"strconv"
	"strings"
)

// scowlWord is a SCOWL spelling and the smallest size level that has it
// (10 is the most common words, 95 the rarest).
type scowlWords map[string]int

// readSCOWL reads the en_US lists of the SCOWL tarball up to maxLevel:
// english and american words, upper-case words, proper names, and
// contractions, and the abbreviations that have no dot ("app", "TV").
func readSCOWL(path string, maxLevel int) (scowlWords, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	gz, err := gzip.NewReader(f)
	if err != nil {
		return nil, err
	}
	words := scowlWords{}
	tr := tar.NewReader(gz)
	for {
		h, err := tr.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, err
		}
		name := h.Name[strings.LastIndex(h.Name, "/")+1:]
		if !strings.Contains(h.Name, "/final/") {
			continue
		}
		dot := strings.LastIndex(name, ".")
		if dot < 0 {
			continue
		}
		level, err := strconv.Atoi(name[dot+1:])
		if err != nil || level > maxLevel {
			continue
		}
		kind := name[:dot]
		switch kind {
		case "english-words", "american-words", "english-upper", "american-upper",
			"english-proper-names", "american-proper-names", "english-contractions", "american-contractions",
			"english-abbreviations", "american-abbreviations":
		default:
			continue
		}
		sc := bufio.NewScanner(tr)
		for sc.Scan() {
			w := latin1(sc.Bytes())
			if w == "" || strings.ContainsRune(w, '.') {
				continue
			}
			if old, ok := words[w]; !ok || level < old {
				words[w] = level
			}
		}
		if err := sc.Err(); err != nil {
			return nil, err
		}
	}
	return words, nil
}

// latin1 decodes ISO-8859-1, SCOWL's encoding.
func latin1(b []byte) string {
	var sb strings.Builder
	for _, c := range b {
		sb.WriteRune(rune(c))
	}
	return strings.TrimSpace(sb.String())
}
