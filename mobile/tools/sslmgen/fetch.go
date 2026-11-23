package main

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
)

// source is one downloaded input. SHA256 pins the exact file; an empty pin
// accepts any file and logs its hash.
type source struct {
	Name, URL, File, SHA256 string
}

// Pinned inputs; DATA_SOURCES.md gives their licenses.
var (
	scowlSource = source{
		Name:   "SCOWL 2020.12.07",
		URL:    "https://downloads.sourceforge.net/project/wordlist/SCOWL/2020.12.07/scowl-2020.12.07.tar.gz",
		File:   "scowl-2020.12.07.tar.gz",
		SHA256: "5587667caa20c4891390c2d42dbb4d5c4c3f41bee77af1457ece3ba23fb859cc",
	}
	// Tatoeba publishes a rolling weekly export. The pin is the snapshot of
	// 2026-11-21; with a newer export the generator keeps only sentences with
	// IDs up to tatoebaMaxID, so additions do not change the model.
	tatoebaSource = source{
		Name:   "Tatoeba English sentences",
		URL:    "https://downloads.tatoeba.org/exports/per_language/eng/eng_sentences.tsv.bz2",
		File:   "eng_sentences.tsv.bz2",
		SHA256: "0c4d3b18152bd6507bbf77ab690532b25de1e60da950f68705b3c88935c01fb4",
	}
)

// fetch makes sure src is in the cache directory and checks its hash.
// strict turns a hash mismatch into an error.
func fetch(cache string, src source, strict bool) (string, error) {
	path := filepath.Join(cache, src.File)
	if _, err := os.Stat(path); err != nil {
		log.Printf("downloading %s", src.URL)
		if err := download(src.URL, path); err != nil {
			return "", err
		}
	}
	sum, err := fileSHA256(path)
	if err != nil {
		return "", err
	}
	if src.SHA256 != "" && sum != src.SHA256 {
		msg := fmt.Sprintf("%s: sha256 %s, pinned %s", src.File, sum, src.SHA256)
		if strict {
			return "", fmt.Errorf("%s (delete it to download again, or run without -strict)", msg)
		}
		log.Printf("warning: %s; the output can differ from the committed model", msg)
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

func fileSHA256(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
