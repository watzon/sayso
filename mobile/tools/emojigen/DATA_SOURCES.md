# Data sources of the emoji keyboard

`android/app/src/main/assets/emoji/emoji.txt` is built only from the Unicode
data below, by `tools/emojigen`. It holds no data from any keyboard project.

## Inputs

| Input | URL | License | Used for |
|---|---|---|---|
| Unicode emoji-test.txt, Emoji 18.0 | https://www.unicode.org/Public/18.0.0/emoji/emoji-test.txt | Unicode License v3 (https://www.unicode.org/license.txt) | the emoji, their order and groups, their names, and the skin-tone variants |
| CLDR 49 English annotations | https://raw.githubusercontent.com/unicode-org/cldr/release-49/common/annotations/en.xml | Unicode License v3 | search keywords |
| CLDR 49 English derived annotations | https://raw.githubusercontent.com/unicode-org/cldr/release-49/common/annotationsDerived/en.xml | Unicode License v3 | search keywords of sequences (ZWJ, keycaps, flags) |

Pinned files (sha256):

- `emoji-test.txt` 18.0.0: `8f3735cda1f92a779d78af67cf86066bb1f07143dc22f2ac29394d9bc57ab21a`
- `annotations/en.xml` release-49: `14d0e9bb5003c085250df4ac7bd7ba1e26d92e74174d7a3a2f42dc16e07fb155`
- `annotationsDerived/en.xml` release-49: `a4a6c1a4c341d94179cb116623c5f44c47400a5bc55da483423ef7430ecf622e`

## What is kept

Fully-qualified emoji of every group but Component, in Unicode's order. An
emoji with a single skin tone (repeated in multi-person emoji) is not listed
on its own; it is one of the five tone variants of the emoji without it. An
emoji gets variants only when all five exist. Sequences that mix two tones
are left out. Keywords are CLDR's, split into lower-case words, without the
words of the name and without the keywords of tone variants.

The app hides, at run time, every emoji the phone's font cannot draw as one
glyph (`Paint.hasGlyph`), so an older Android shows no empty boxes.

## Format

UTF-8 text. `#` lines are comments. `@<group>` starts a group. Every other
line is `<emoji> TAB <name> TAB <keywords>`, plus `TAB <five variants>`
(space separated, lightest first) for emoji with skin tones.

## Rebuilding

```sh
cd mobile
go run ./tools/emojigen -strict
```

Two runs on the same inputs give a byte-identical file. The committed file:
114,659 bytes, 1,923 emoji in 9 groups, 332 with skin tones; sha256
`ec39b73a15b69e39834bf8c2452949006372aefd5730ef687de24daa10f07441`.
