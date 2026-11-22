package languages

import "testing"

func TestFromSystem(t *testing.T) {
	cases := []struct {
		in   []string
		want string
	}{
		{[]string{"zh-Hans-CN", "en-US"}, "zh"},
		{[]string{"pt_BR.UTF-8"}, "pt"},
		{[]string{"yue-Hant-HK"}, "yue"},
		{[]string{"nb-NO"}, "no"},
		{[]string{"xx-YY", "de-AT"}, "de"},
		{[]string{"C"}, "en"},
		{nil, "en"},
	}
	for _, c := range cases {
		if got := FromSystem(c.in); got != c.want {
			t.Errorf("%v: got %s want %s", c.in, got, c.want)
		}
	}
}

func TestListsAndNames(t *testing.T) {
	for _, list := range [][]string{English(), Whisper(), ParakeetV3(), Cohere(), SenseVoice(), Nemotron(), AppleSpeech()} {
		for _, c := range list {
			if Name(c) == "" {
				t.Errorf("no name for %s", c)
			}
		}
	}
	if len(Whisper()) != 99 || len(ParakeetV3()) != 25 {
		t.Fatal(len(Whisper()), len(ParakeetV3()))
	}
	seen := map[string]bool{}
	for _, p := range names {
		if seen[p.code] {
			t.Fatal("twice", p.code)
		}
		seen[p.code] = true
	}
	if !IsValid(Auto) || !IsValid("de") || IsValid("klingon") {
		t.Fatal("IsValid")
	}
	if All()[0] != "af" || len(All()) != len(names) {
		t.Fatal(All()[:3])
	}
}
