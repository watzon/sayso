package ui

import (
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

func TestMonoFontKeepsJoinedScriptsInSans(t *testing.T) {
	t.Cleanup(func() { i18n.SetLocale(i18n.DefaultLocale) })
	i18n.SetLocale("ar")
	sans := theme.UIFor("ar")
	for s, want := range map[string]string{
		"1.3 GB":               theme.Mono,
		"12:04":                theme.Mono,
		"٣٢":                   theme.Mono, // Arabic-Indic digits do not join
		"646 MB · 99 Sprachen": theme.Mono,
		"1.3 GB · English":     theme.Mono,
		"1,3 ГБ · русский":     theme.Mono,
		"1.3 GB · 英语":          theme.Mono,
		"1.3 GB · الإنجليزية":  sans,
		"لغة 99":               sans,
		"لم يبدأ":              sans,
		"22 نوفمبر":            sans,
		"עברית":                sans,
		"हिन्दी":               sans,
	} {
		if got := monoFont(s); got != want {
			t.Errorf("monoFont(%q) = %q, want %q", s, got, want)
		}
	}
	if size := float32(40); tracking("Welcome", -0.03, size) != -0.03*size {
		got := tracking("Welcome", -0.03, size)
		t.Errorf("tracking of Latin: %v", got)
	}
	if got := tracking("اختر نموذج كلام", -0.03, 40); got != 0 {
		t.Errorf("tracking of Arabic: %v, want none", got)
	}
}

// Text the interface shows in a monospace face or with letter spacing goes
// through monoFont or tracking, so a translation into a script whose
// letters join never comes apart. Only what is pinned left to right (ltr)
// or typed into an input (a path, a key, a hex color) may name theme.Mono.
func TestNoMonoOrTrackingOnTranslatedText(t *testing.T) {
	files, err := filepath.Glob("*.go")
	if err != nil {
		t.Fatal(err)
	}
	fset := token.NewFileSet()
	for _, name := range files {
		if strings.HasSuffix(name, "_test.go") {
			continue
		}
		src, err := os.ReadFile(name)
		if err != nil {
			t.Fatal(err)
		}
		f, err := parser.ParseFile(fset, name, src, 0)
		if err != nil {
			t.Fatal(err)
		}
		ast.Inspect(f, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			sel, ok := call.Fun.(*ast.SelectorExpr)
			if !ok || len(call.Args) != 1 {
				return true
			}
			switch sel.Sel.Name {
			case "Font":
				if isSel(call.Args[0], "theme", "Mono") && chainRoot(sel.X) == "Text" {
					t.Errorf("%s: text in theme.Mono outside ltr: use monoFont(s)", fset.Position(call.Pos()))
				}
			case "LetterSpacing":
				if inner, ok := call.Args[0].(*ast.CallExpr); !ok || !isIdent(inner.Fun, "tracking") {
					t.Errorf("%s: letter spacing outside tracking(s, …)", fset.Position(call.Pos()))
				}
			}
			return true
		})
	}
}

// chainRoot follows a builder chain (x.A().B()) down to the call that made
// the element: "Text" for mu.Text, "ltr" when pinned left to right on the
// way, or the name of whatever else (an input, a helper).
func chainRoot(e ast.Expr) string {
	for {
		call, ok := e.(*ast.CallExpr)
		if !ok {
			return ""
		}
		switch fn := call.Fun.(type) {
		case *ast.Ident:
			return fn.Name
		case *ast.SelectorExpr:
			if id, ok := fn.X.(*ast.Ident); ok && id.Name == "mu" {
				return fn.Sel.Name
			}
			e = fn.X
		default:
			return ""
		}
	}
}

func isSel(e ast.Expr, pkg, name string) bool {
	s, ok := e.(*ast.SelectorExpr)
	return ok && isIdent(s.X, pkg) && s.Sel.Name == name
}

func isIdent(e ast.Expr, name string) bool {
	id, ok := e.(*ast.Ident)
	return ok && id.Name == name
}
