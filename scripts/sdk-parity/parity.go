package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"slices"
	"sort"
	"strings"
)

// Manifest is sdk/conformance/scenarios.json.
type Manifest struct {
	SDKs      []string   `json:"sdks"`
	Scenarios []Scenario `json:"scenarios"`
}

// Scenario is one scenario. NA maps "sdk" or "sdk/goos" to the reason why
// the scenario does not apply there.
type Scenario struct {
	ID    string            `json:"id"`
	Title string            `json:"title"`
	NA    map[string]string `json:"na"`
}

func ReadManifest(r io.Reader) (*Manifest, error) {
	var m Manifest
	dec := json.NewDecoder(r)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&struct {
		About string `json:"about"`
		*Manifest
	}{Manifest: &m}); err != nil {
		return nil, fmt.Errorf("scenarios.json: %v", err)
	}
	if len(m.SDKs) == 0 || len(m.Scenarios) == 0 {
		return nil, fmt.Errorf("scenarios.json: no sdks or no scenarios")
	}
	seen := map[string]bool{}
	for _, s := range m.Scenarios {
		if s.ID == "" || seen[s.ID] {
			return nil, fmt.Errorf("scenarios.json: an empty or repeated id %q", s.ID)
		}
		seen[s.ID] = true
		for key, why := range s.NA {
			sdk, goos, _ := strings.Cut(key, "/")
			if !slices.Contains(m.SDKs, sdk) || strings.Contains(goos, "/") || strings.TrimSpace(why) == "" {
				return nil, fmt.Errorf("scenarios.json: %s: na %q needs a known sdk, an optional system, and a reason", s.ID, key)
			}
		}
	}
	return &m, nil
}

// ReadReport gives the ids of a report: one id on each line. An id can come
// more than once.
func ReadReport(r io.Reader) (map[string]bool, error) {
	out := map[string]bool{}
	sc := bufio.NewScanner(r)
	for sc.Scan() {
		if id := strings.TrimSpace(sc.Text()); id != "" {
			out[id] = true
		}
	}
	return out, sc.Err()
}

// Applies says whether the scenario is required of sdk on goos.
func (s Scenario) Applies(sdk, goos string) bool {
	_, all := s.NA[sdk]
	_, here := s.NA[sdk+"/"+goos]
	return !all && !here
}

// Problem is a scenario that an SDK did not pass, or an id that the
// manifest does not have.
type Problem struct {
	SDK, ID, Text string
}

func (p Problem) String() string { return p.SDK + ": " + p.ID + ": " + p.Text }

// Check compares the report of each SDK with the manifest on goos. Every
// SDK of the manifest needs a report.
func Check(m *Manifest, goos string, reports map[string]map[string]bool) []Problem {
	var out []Problem
	known := map[string]bool{}
	for _, s := range m.Scenarios {
		known[s.ID] = true
	}
	for _, sdk := range m.SDKs {
		passed, ok := reports[sdk]
		if !ok {
			out = append(out, Problem{sdk, "-", "no report"})
			continue
		}
		for _, s := range m.Scenarios {
			if s.Applies(sdk, goos) && !passed[s.ID] {
				out = append(out, Problem{sdk, s.ID, "missing: " + s.Title})
			}
		}
		var extra []string
		for id := range passed {
			if !known[id] {
				extra = append(extra, id)
			}
		}
		sort.Strings(extra)
		for _, id := range extra {
			out = append(out, Problem{sdk, id, "not in scenarios.json"})
		}
	}
	for sdk := range reports {
		if !slices.Contains(m.SDKs, sdk) {
			out = append(out, Problem{sdk, "-", "not an SDK of scenarios.json"})
		}
	}
	return out
}
