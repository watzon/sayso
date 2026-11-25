// Command winres makes the Windows resources of Sayso.exe and checks a built
// exe for them (internal/winres).
//
//	go run ./scripts/winres generate [-ico assets/app/AppIcon.ico] [-out cmd/sayso]
//	go run ./scripts/winres check <exe>
//
// generate writes rsrc_windows_<arch>.syso into the folder. Run it after a
// change to AppIcon.ico; a test fails while the committed files differ from
// its output. check fails unless the exe has icon group 1 with every size;
// the Windows bundle script runs it on each build.
package main

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"slices"

	"github.com/watzon/sayso/internal/winres"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	if len(args) == 0 {
		return errors.New("usage: winres generate [-ico file] [-out dir] | winres check <exe>")
	}
	switch args[0] {
	case "generate":
		fs := flag.NewFlagSet("generate", flag.ContinueOnError)
		ico := fs.String("ico", filepath.Join("assets", "app", "AppIcon.ico"), "the app icon")
		out := fs.String("out", filepath.Join("cmd", "sayso"), "the folder of the main package")
		if err := fs.Parse(args[1:]); err != nil {
			return err
		}
		files, err := winres.Generate(*ico)
		if err != nil {
			return err
		}
		names := make([]string, 0, len(files))
		for name := range files {
			names = append(names, name)
		}
		slices.Sort(names)
		for _, name := range names {
			if err := os.WriteFile(filepath.Join(*out, name), files[name], 0o644); err != nil {
				return err
			}
			fmt.Println("wrote", filepath.Join(*out, name))
		}
		return nil
	case "check":
		if len(args) != 2 {
			return errors.New("usage: winres check <exe>")
		}
		sizes, err := winres.Check(args[1])
		if err != nil {
			return fmt.Errorf("%s: %w", args[1], err)
		}
		fmt.Printf("%s: icon group 1 with %v px images\n", args[1], sizes)
		return nil
	}
	return fmt.Errorf("unknown command %q", args[0])
}
