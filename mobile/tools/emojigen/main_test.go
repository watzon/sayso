package main

import (
	"strings"
	"testing"
)

const sample = `# group: Smileys & Emotion
# subgroup: face-smiling
1F600                                                  ; fully-qualified     # 😀 E1.0 grinning face
263A FE0F                                              ; fully-qualified     # ☺️ E0.6 smiling face
263A                                                   ; unqualified         # ☺ E0.6 smiling face

# group: People & Body
1F44B                                                  ; fully-qualified     # 👋 E0.6 waving hand
1F44B 1F3FB                                            ; fully-qualified     # 👋🏻 E1.0 waving hand: light skin tone
1F44B 1F3FC                                            ; fully-qualified     # 👋🏼 E1.0 waving hand: medium-light skin tone
1F44B 1F3FD                                            ; fully-qualified     # 👋🏽 E1.0 waving hand: medium skin tone
1F44B 1F3FE                                            ; fully-qualified     # 👋🏾 E1.0 waving hand: medium-dark skin tone
1F44B 1F3FF                                            ; fully-qualified     # 👋🏿 E1.0 waving hand: dark skin tone
261D FE0F                                              ; fully-qualified     # ☝️ E0.6 index pointing up
261D 1F3FB                                             ; fully-qualified     # ☝🏻 E1.0 index pointing up: light skin tone
1F9D1 200D 1F91D 200D 1F9D1                            ; fully-qualified     # 🧑‍🤝‍🧑 E12.0 people holding hands
1F9D1 1F3FB 200D 1F91D 200D 1F9D1 1F3FC                ; fully-qualified     # 🧑🏻‍🤝‍🧑🏼 E12.1 people holding hands: light skin tone, medium-light skin tone

# group: Component
1F3FB                                                  ; component           # 🏻 E1.0 light skin tone
`

func TestParseKeepsFullyQualifiedBaseEmojiAndCompleteToneSets(t *testing.T) {
	groups, err := parseEmojiTest(strings.NewReader(sample))
	if err != nil {
		t.Fatal(err)
	}
	if len(groups) != 2 || groups[0].Name != "Smileys & Emotion" || groups[1].Name != "People & Body" {
		t.Fatalf("groups: %+v", groups)
	}
	if got := len(groups[0].Emoji); got != 2 {
		t.Fatalf("smileys: %d, want 2 (unqualified left out)", got)
	}
	people := groups[1].Emoji
	if len(people) != 3 {
		t.Fatalf("people: %d, want 3 (no tone variants as their own emoji)", len(people))
	}
	wave := people[0]
	if len(wave.Tones) != 5 || wave.Tones[0] != "👋🏻" || wave.Tones[4] != "👋🏿" {
		t.Errorf("waving hand tones: %q", wave.Tones)
	}
	if people[1].Tones != nil {
		t.Errorf("an incomplete tone set is dropped, got %q", people[1].Tones)
	}
	if people[2].Tones != nil {
		t.Errorf("mixed tones are not a variant, got %q", people[2].Tones)
	}
}

func TestAnnotationsGiveKeywordsTheNameLacksAndIgnoreToneVariants(t *testing.T) {
	xmlDoc := `<ldml><annotations>
<annotation cp="😀">face | grin | grinning face | happy</annotation>
<annotation cp="😀" type="tts">grinning face</annotation>
<annotation cp="👋🏻">hand | light skin tone | hello</annotation>
<annotation cp="☺">outlined | relaxed</annotation>
</annotations></ldml>`
	kw := map[string][]string{}
	if err := readAnnotations(strings.NewReader(xmlDoc), kw); err != nil {
		t.Fatal(err)
	}
	if got := searchWords("grinning face", kw[key("😀")]); got != "grin happy" {
		t.Errorf("grinning face: %q", got)
	}
	if _, ok := kw[key("👋")]; ok {
		t.Errorf("tone variant keywords leaked into the base: %q", kw[key("👋")])
	}
	if got := searchWords("smiling face", kw[key("☺️")]); got != "outlined relaxed" {
		t.Errorf("a cp without FE0F matches the qualified emoji: %q", got)
	}
}

func TestWriteFormat(t *testing.T) {
	groups := []*Group{{Name: "People & Body", Emoji: []*Emoji{
		{Value: "👋", Name: "waving hand", Tones: []string{"a", "b", "c", "d", "e"}},
		{Value: "👀", Name: "eyes"},
	}}}
	var b strings.Builder
	n, toned := write(&b, groups, map[string][]string{"👀": {"look"}})
	if n != 2 || toned != 1 {
		t.Fatalf("n=%d toned=%d", n, toned)
	}
	lines := strings.Split(strings.TrimSpace(b.String()), "\n")
	tail := lines[len(lines)-3:]
	want := []string{"@People & Body", "👋\twaving hand\t\ta b c d e", "👀\teyes\tlook"}
	for i := range want {
		if tail[i] != want[i] {
			t.Errorf("line %d: %q, want %q", i, tail[i], want[i])
		}
	}
}
