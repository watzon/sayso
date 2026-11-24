// Command emojigen writes the emoji keyboard's data file from Unicode's
// emoji-test.txt and CLDR's English annotations. See DATA_SOURCES.md.
package main

import (
	"bufio"
	"crypto/sha256"
	"encoding/hex"
	"encoding/xml"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type source struct {
	URL, File, SHA256 string
}

var (
	emojiTest = source{
		URL:    "https://www.unicode.org/Public/18.0.0/emoji/emoji-test.txt",
		File:   "emoji-test-18.0.0.txt",
		SHA256: "8f3735cda1f92a779d78af67cf86066bb1f07143dc22f2ac29394d9bc57ab21a",
	}
	annotations = source{
		URL:    "https://raw.githubusercontent.com/unicode-org/cldr/release-49/common/annotations/en.xml",
		File:   "cldr-49-annotations-en.xml",
		SHA256: "14d0e9bb5003c085250df4ac7bd7ba1e26d92e74174d7a3a2f42dc16e07fb155",
	}
	derived = source{
		URL:    "https://raw.githubusercontent.com/unicode-org/cldr/release-49/common/annotationsDerived/en.xml",
		File:   "cldr-49-annotationsDerived-en.xml",
		SHA256: "a4a6c1a4c341d94179cb116623c5f44c47400a5bc55da483423ef7430ecf622e",
	}
)

func main() {
	userCache, _ := os.UserCacheDir()
	cache := flag.String("cache", filepath.Join(userCache, "sayso-emojigen"), "download folder")
	out := flag.String("out", "android/app/src/main/assets/emoji/emoji.txt", "output file")
	strict := flag.Bool("strict", false, "fail when a download does not match its pinned sha256")
	flag.Parse()

	if err := os.MkdirAll(*cache, 0o755); err != nil {
		log.Fatal(err)
	}
	testPath := must(fetch(*cache, emojiTest, *strict))
	keywords := map[string][]string{}
	for _, src := range []source{annotations, derived} {
		f := must(os.Open(must(fetch(*cache, src, *strict))))
		if err := readAnnotations(f, keywords); err != nil {
			log.Fatalf("%s: %v", src.File, err)
		}
		f.Close()
	}
	f := must(os.Open(testPath))
	groups, err := parseEmojiTest(f)
	f.Close()
	if err != nil {
		log.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(*out), 0o755); err != nil {
		log.Fatal(err)
	}
	w := must(os.Create(*out))
	n, toned := write(w, groups, keywords)
	if err := w.Close(); err != nil {
		log.Fatal(err)
	}
	log.Printf("wrote %s: %d emoji in %d groups, %d with skin tones", *out, n, len(groups), toned)
}

func must[T any](v T, err error) T {
	if err != nil {
		log.Fatal(err)
	}
	return v
}

// Emoji is one emoji a keyboard shows: fully qualified, with no skin tone.
// Tones holds its five one-tone variants, lightest first, or none.
type Emoji struct {
	Value, Name string
	Tones       []string
}

type Group struct {
	Name  string
	Emoji []*Emoji
}

func isTone(r rune) bool { return r >= 0x1F3FB && r <= 0x1F3FF }

// key identifies an emoji without skin tones or variation selectors.
func key(s string) string {
	var b strings.Builder
	for _, r := range s {
		if r != 0xFE0F && !isTone(r) {
			b.WriteRune(r)
		}
	}
	return b.String()
}

// parseEmojiTest reads emoji-test.txt: the fully-qualified emoji by group,
// leaving out the Component group. A sequence with one skin tone (possibly
// repeated, as in people holding hands) is a variant of the emoji it has
// without the tone; sequences that mix tones are left out.
func parseEmojiTest(r io.Reader) ([]*Group, error) {
	var groups []*Group
	byKey := map[string]*Emoji{}
	type variant struct {
		key   string
		tone  int
		value string
	}
	var variants []variant
	var group *Group
	sc := bufio.NewScanner(r)
	for sc.Scan() {
		line := sc.Text()
		if name, ok := strings.CutPrefix(line, "# group: "); ok {
			group = nil
			if name != "Component" {
				group = &Group{Name: name}
				groups = append(groups, group)
			}
			continue
		}
		if line == "" || strings.HasPrefix(line, "#") || group == nil {
			continue
		}
		fields, comment, ok := strings.Cut(line, "#")
		if !ok {
			return nil, fmt.Errorf("no comment: %q", line)
		}
		cps, status, ok := strings.Cut(fields, ";")
		if !ok || strings.TrimSpace(status) != "fully-qualified" {
			continue
		}
		var value strings.Builder
		for _, cp := range strings.Fields(cps) {
			var r rune
			if _, err := fmt.Sscanf(cp, "%X", &r); err != nil {
				return nil, fmt.Errorf("%q: %v", line, err)
			}
			value.WriteRune(r)
		}
		// The comment is "<emoji> E<version> <name>".
		parts := strings.SplitN(strings.TrimSpace(comment), " ", 3)
		if len(parts) != 3 {
			return nil, fmt.Errorf("bad comment: %q", line)
		}
		v := value.String()
		tones := map[rune]bool{}
		for _, r := range v {
			if isTone(r) {
				tones[r] = true
			}
		}
		switch len(tones) {
		case 0:
			e := &Emoji{Value: v, Name: parts[2]}
			group.Emoji = append(group.Emoji, e)
			byKey[key(v)] = e
		case 1:
			for t := range tones {
				variants = append(variants, variant{key(v), int(t - 0x1F3FB), v})
			}
		}
	}
	if err := sc.Err(); err != nil {
		return nil, err
	}
	found := map[*Emoji][]string{}
	for _, v := range variants {
		e := byKey[v.key]
		if e == nil {
			continue
		}
		if found[e] == nil {
			found[e] = make([]string, 5)
		}
		found[e][v.tone] = v.value
	}
	for e, tones := range found {
		complete := true
		for _, t := range tones {
			complete = complete && t != ""
		}
		if complete {
			e.Tones = tones
		}
	}
	return groups, nil
}

type ldml struct {
	Annotations []struct {
		CP   string `xml:"cp,attr"`
		Type string `xml:"type,attr"`
		Text string `xml:",chardata"`
	} `xml:"annotations>annotation"`
}

// readAnnotations adds CLDR's keywords ("face | grin | ...") by emoji key.
func readAnnotations(r io.Reader, into map[string][]string) error {
	var doc ldml
	if err := xml.NewDecoder(r).Decode(&doc); err != nil {
		return err
	}
	for _, a := range doc.Annotations {
		// Tone variants share their base's key; their keywords only add the tone's name.
		if a.Type == "tts" || strings.IndexFunc(a.CP, isTone) >= 0 {
			continue
		}
		k := key(a.CP)
		for _, kw := range strings.Split(a.Text, "|") {
			if kw = strings.TrimSpace(kw); kw != "" {
				into[k] = append(into[k], kw)
			}
		}
	}
	return nil
}

// searchWords are the lower-case words of the keywords that the name lacks, sorted.
func searchWords(name string, keywords []string) string {
	have := map[string]bool{}
	for _, w := range strings.Fields(strings.ToLower(name)) {
		have[strings.Trim(w, ":,")] = true
	}
	var out []string
	for _, kw := range keywords {
		for _, w := range strings.Fields(strings.ToLower(kw)) {
			if !have[w] {
				have[w] = true
				out = append(out, w)
			}
		}
	}
	sort.Strings(out)
	return strings.Join(out, " ")
}

// write prints the data file:
//
//	@<group name>
//	<emoji> TAB <name> TAB <keywords> [TAB <tone 1> SPACE ... <tone 5>]
func write(w io.Writer, groups []*Group, keywords map[string][]string) (count, toned int) {
	bw := bufio.NewWriter(w)
	fmt.Fprintf(bw, "# Sayso emoji data, generated by mobile/tools/emojigen. Do not edit.\n")
	fmt.Fprintf(bw, "# From Unicode emoji-test.txt 18.0 and CLDR 49 English annotations,\n")
	fmt.Fprintf(bw, "# Copyright Unicode, Inc., under the Unicode License v3.\n")
	for _, g := range groups {
		fmt.Fprintf(bw, "@%s\n", g.Name)
		for _, e := range g.Emoji {
			fmt.Fprintf(bw, "%s\t%s\t%s", e.Value, e.Name, searchWords(e.Name, keywords[key(e.Value)]))
			if e.Tones != nil {
				fmt.Fprintf(bw, "\t%s", strings.Join(e.Tones, " "))
				toned++
			}
			fmt.Fprintln(bw)
			count++
		}
	}
	bw.Flush()
	return count, toned
}

func fetch(cache string, src source, strict bool) (string, error) {
	path := filepath.Join(cache, src.File)
	if _, err := os.Stat(path); err != nil {
		log.Printf("downloading %s", src.URL)
		if err := download(src.URL, path); err != nil {
			return "", err
		}
	}
	b, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(b)
	got := hex.EncodeToString(sum[:])
	if src.SHA256 == "" {
		log.Printf("%s: sha256 %s (not pinned)", src.File, got)
	} else if got != src.SHA256 {
		msg := fmt.Sprintf("%s: sha256 %s, pinned %s", src.File, got, src.SHA256)
		if strict {
			return "", fmt.Errorf("%s", msg)
		}
		log.Printf("warning: %s", msg)
	}
	return path, nil
}

func download(url, path string) error {
	resp, err := http.Get(url)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("%s: %s", url, resp.Status)
	}
	tmp := path + ".part"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if _, err := io.Copy(f, resp.Body); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
