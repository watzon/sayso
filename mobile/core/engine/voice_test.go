package engine

import "testing"

func TestVoiceDictionary(t *testing.T) {
	f := newField(t, "")
	f.e.AddWord("GitHub")
	if err := f.e.AddReplacement("omw", "on my way", false); err != nil {
		t.Fatal(err)
	}
	d := f.e.VoiceDictionary()
	if len(d.Words) != 1 || d.Words[0] != "GitHub" || len(d.Replacements) != 1 || d.Replacements[0].To != "on my way" || !d.Autocap {
		t.Fatalf("%+v", d)
	}
	for word, want := range map[string]string{"The": "the", "i": "I", "london": "London"} {
		if got, ok := d.Spelling(word); !ok || got != want {
			t.Errorf("Spelling(%q) = %q, %v; want %q", word, got, ok, want)
		}
	}
	if _, ok := d.Spelling("qzxv"); ok {
		t.Error("a word the model lacks has a spelling")
	}
	f.e.SetFlags(0)
	if f.e.VoiceDictionary().Autocap {
		t.Error("Autocap without the field flag")
	}
}
