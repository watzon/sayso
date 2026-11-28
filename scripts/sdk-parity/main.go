// Command sdk-parity checks that the Go, TypeScript, and Rust SDKs pass the
// same scenarios. Each suite, run with SAYSO_SCENARIO_REPORT=<file>, writes
// the ids of the scenarios that passed; this compares those files with
// sdk/conformance/scenarios.json and fails on a scenario that one SDK did
// not pass.
//
//	go run ./scripts/sdk-parity -go go.txt -typescript ts.txt -rust rust.txt
package main

import (
	"flag"
	"fmt"
	"os"
	"runtime"
)

func main() {
	manifest := flag.String("manifest", "sdk/conformance/scenarios.json", "the list of scenarios")
	goos := flag.String("os", runtime.GOOS, "the system the suites ran on: darwin, linux, or windows")
	files := map[string]*string{}
	for _, sdk := range []string{"go", "typescript", "rust"} {
		files[sdk] = flag.String(sdk, "", "the scenario report of the "+sdk+" suite")
	}
	flag.Parse()
	os.Exit(run(*manifest, *goos, files))
}

func run(manifestPath, goos string, files map[string]*string) int {
	f, err := os.Open(manifestPath)
	if err != nil {
		fmt.Fprintln(os.Stderr, "sdk-parity:", err)
		return 2
	}
	m, err := ReadManifest(f)
	f.Close()
	if err != nil {
		fmt.Fprintln(os.Stderr, "sdk-parity:", err)
		return 2
	}
	reports := map[string]map[string]bool{}
	for sdk, path := range files {
		if *path == "" {
			continue
		}
		r, err := os.Open(*path)
		if err != nil {
			fmt.Fprintln(os.Stderr, "sdk-parity:", err)
			return 2
		}
		ids, err := ReadReport(r)
		r.Close()
		if err != nil {
			fmt.Fprintln(os.Stderr, "sdk-parity:", err)
			return 2
		}
		reports[sdk] = ids
	}
	problems := Check(m, goos, reports)
	for _, p := range problems {
		fmt.Fprintln(os.Stderr, "sdk-parity:", p)
	}
	if len(problems) > 0 {
		fmt.Fprintf(os.Stderr, "sdk-parity: %d problems\n", len(problems))
		return 1
	}
	for _, sdk := range m.SDKs {
		n := 0
		for _, s := range m.Scenarios {
			if s.Applies(sdk, goos) {
				n++
			}
		}
		fmt.Printf("sdk-parity: %s passed all %d scenarios that apply on %s\n", sdk, n, goos)
	}
	return 0
}
