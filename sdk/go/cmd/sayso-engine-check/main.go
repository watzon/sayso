// Command sayso-engine-check checks that an engine follows the engine
// protocol v1.
//
//	sayso-engine-check --engine PROGRAM [--levels core,model,store] [--model ID] [--json]
//
// The levels model and store download a model. The exit code is 1 when a
// check fails.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/watzon/sayso/sdk/go/conformance"
)

func main() {
	program := flag.String("engine", "", "the engine program")
	levels := flag.String("levels", "core", "the levels: core, model, store")
	model := flag.String("model", "", "the model of the levels model and store")
	models := flag.String("models", "", "the models folder; empty is a new temporary folder")
	asJSON := flag.Bool("json", false, "print the report as JSON")
	flag.Parse()
	if *program == "" {
		fmt.Fprintln(os.Stderr, "sayso-engine-check: --engine is required")
		os.Exit(2)
	}
	path, err := conformance.LookPath(*program)
	if err != nil {
		fmt.Fprintln(os.Stderr, "sayso-engine-check:", err)
		os.Exit(2)
	}
	var ls []conformance.Level
	for _, l := range strings.Split(*levels, ",") {
		ls = append(ls, conformance.Level(strings.TrimSpace(l)))
	}
	report, err := conformance.Check(context.Background(), conformance.Options{Program: path, Levels: ls, Model: *model, ModelsDir: *models})
	if err != nil {
		fmt.Fprintln(os.Stderr, "sayso-engine-check:", err)
		os.Exit(1)
	}
	if *asJSON {
		b, _ := json.MarshalIndent(report, "", "  ")
		fmt.Println(string(b))
	} else {
		fmt.Printf("engine %s, protocol %d, features %s\n", report.Engine.Version, report.Engine.Protocol, strings.Join(report.Engine.Features, ","))
		for _, r := range report.Results {
			mark := "ok  "
			switch {
			case r.Skip:
				mark = "skip"
			case !r.Pass:
				mark = "FAIL"
			}
			fmt.Printf("%s %-28s %s\n", mark, r.ID, r.Detail)
		}
	}
	if len(report.Failed()) > 0 {
		os.Exit(1)
	}
}
