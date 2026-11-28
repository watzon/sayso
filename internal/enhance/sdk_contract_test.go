package enhance

import (
	"reflect"
	"testing"

	"github.com/watzon/sayso/sdk/go/cleanup"
)

// The cleanup catalog of the app and of the SDK name the same files, so a
// model that one downloads is ready for the other.
func TestTheCleanupCatalogIsTheCatalogOfTheSDK(t *testing.T) {
	type entry struct {
		ID, Repo, Revision, File, SHA256 string
		SizeBytes                        uint64
		LicenseFiles, Languages          []string
		NCtx, MaxPartTokens              uint32
	}
	var app, sdk []entry
	for _, m := range CleanupCatalog() {
		app = append(app, entry{m.ID, m.Repo, m.Revision, m.File, m.SHA256, m.SizeBytes, m.LicenseFiles, m.Languages, NCtx, MaxPartTokens})
	}
	for _, m := range cleanup.Catalog() {
		sdk = append(sdk, entry{m.ID, m.Repo, m.Revision, m.File, m.SHA256, m.SizeBytes, m.LicenseFiles, m.Languages, cleanup.NCtx, cleanup.MaxPartTokens})
	}
	if !reflect.DeepEqual(app, sdk) {
		t.Fatalf("app %+v\nSDK %+v", app, sdk)
	}
	for _, m := range CleanupCatalog() {
		s, _ := cleanup.Find(m.ID)
		if CleanupModelDir("/m", &m) != cleanup.ModelDir("/m", s) {
			t.Fatal("the folder of", m.ID)
		}
		p := Render(&m, "send it", Settings{}, "Mail", "", 3)
		q := cleanup.Render(s, "send it", cleanup.Settings{}, "Mail", "", 3)
		if p.Text != q.Text || !reflect.DeepEqual(p.End, q.End) || p.MaxTokens != q.MaxTokens {
			t.Fatalf("the prompt of %s:\n%q\n%q", m.ID, p.Text, q.Text)
		}
	}
}
