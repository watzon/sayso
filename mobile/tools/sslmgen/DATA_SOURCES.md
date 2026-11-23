# Data sources of the typing model

`mobile/models/en-US.sslm` is built only from the open data below, by
`tools/sslmgen`. The model holds no code or data from any keyboard project
(Hex, FUTO, AOSP LatinIME, or others).

## Inputs

| Input | URL | License | Used for |
|---|---|---|---|
| SCOWL 2020.12.07 (Spell Checker Oriented Word Lists), Kevin Atkinson | https://downloads.sourceforge.net/project/wordlist/SCOWL/2020.12.07/scowl-2020.12.07.tar.gz (project: http://wordlist.aspell.net/) | MIT-like permissive license; full notice in [`mobile/models/SCOWL-Copyright.txt`](../../models/SCOWL-Copyright.txt) | which spellings are words (en_US lists up to level 60, plus level 70 when the corpus has the word), their capitalization, and a frequency prior by level |
| Tatoeba English sentences, weekly export | https://downloads.tatoeba.org/exports/per_language/eng/eng_sentences.tsv.bz2 (project: https://tatoeba.org) | CC BY 2.0 FR (https://creativecommons.org/licenses/by/2.0/fr/); some sentences CC0 | unigram, bigram, and trigram counts; the case of words mid-sentence; the held-out replay text |
| `extra-words.txt` (this directory) | — | written for Sayso, GPL-3.0 with the repo | a few chat words SCOWL lacks (`wifi`, `lol`, `btw`) |

Pinned files (sha256):

- `scowl-2020.12.07.tar.gz`: `5587667caa20c4891390c2d42dbb4d5c4c3f41bee77af1457ece3ba23fb859cc`
- `eng_sentences.tsv.bz2`, the export of 2026-11-21: `0c4d3b18152bd6507bbf77ab690532b25de1e60da950f68705b3c88935c01fb4`

Tatoeba replaces its export every week. The generator keeps only sentences with
IDs up to 14127918, the largest in the pinned snapshot, so newer sentences do
not change the model; edits and deletions of old sentences still can. A
non-matching download prints a warning (`-strict` makes it an error).

## Attribution

The model is a derived work of Tatoeba sentences: "Sentences from Tatoeba
(https://tatoeba.org), licensed under CC BY 2.0 FR." Apps that ship the model
show that line and the SCOWL notice in their open-source acknowledgements.

## Held-out text

Sentences whose Tatoeba ID is a multiple of 50 are not used for the model, and
training sentences with the same text as one of them are dropped as well. The
first 3,000 are written to `mobile/core/eval/testdata/heldout-en.txt` for the
replay benchmark (same CC BY 2.0 FR license).

## Rebuilding

```sh
cd mobile
go run ./tools/sslmgen            # downloads into the user cache dir, writes models/en-US.sslm
go run ./tools/sslminfo don\'t im # look inside
```

Two runs on the same inputs give a byte-identical file. The committed model:
7,964,896 bytes, 91,074 words, 210,337 trie nodes, 380,000 bigrams, 200,000
trigrams, from 15,443,409 training tokens; sha256
`26029533e2aaedeca0a9315a2aec30f20f5e8f04fcd16193196bb03a4bdc52d8`.

## Rejected sources

- Google Books Ngram (CC BY 3.0): suitable, but the English 1-grams alone are
  about 6.4 GB; not worth it for a reproducible first model.
- OpenSubtitles frequency lists, Wikipedia, wordfreq: CC BY-SA or unclear terms.
- Norvig's Google Web counts: derived from an LDC-licensed corpus.
- Any keyboard's shipped dictionaries or models.
