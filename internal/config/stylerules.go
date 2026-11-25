package config

import (
	"fmt"
	"strings"
	"unicode"
)

// Style rules: the style of a dictation follows the app or the site that the
// text goes into.

// StyleRule is the apps and sites of one style.
type StyleRule struct {
	Style string
	// Apps: a bundle id on macOS, an exe name on Windows, a desktop entry on Linux.
	Apps []string
	// Sites: a host with an optional path ("facebook.com/messages").
	Sites []string
}

// StyleRules is [ai.style_rules].
type StyleRules struct {
	Enabled bool
	// Seeded: the starter set was added (one time only).
	Seeded bool
	Rules  []StyleRule
}

func sameApp(a, b string) bool { return strings.EqualFold(a, b) }

// Resolve gives the style for a dictation into app, on page url. A site rule
// wins over an app rule, and the most specific site wins.
func (s StyleRules) Resolve(app, url string) (string, bool) {
	if !s.Enabled {
		return "", false
	}
	if url != "" {
		if host, path, ok := splitURL(url); ok {
			best, bestHost, bestPath, found := "", -1, -1, false
			for _, r := range s.Rules {
				for _, site := range r.Sites {
					h, p, fits := siteSpecificity(site, host, path)
					// max_by_key keeps the last of equal maxima.
					if fits && (h > bestHost || (h == bestHost && p >= bestPath)) {
						best, bestHost, bestPath, found = r.Style, h, p, true
					}
				}
			}
			if found {
				return best, true
			}
		}
	}
	if app == "" {
		return "", false
	}
	return s.StyleOfApp(app)
}

// Rule is the rule of a style.
func (s StyleRules) Rule(style string) (StyleRule, bool) {
	for _, r := range s.Rules {
		if r.Style == style {
			return r, true
		}
	}
	return StyleRule{}, false
}

// StyleOfApp is the style that has this app.
func (s StyleRules) StyleOfApp(app string) (string, bool) {
	for _, r := range s.Rules {
		for _, a := range r.Apps {
			if sameApp(a, app) {
				return r.Style, true
			}
		}
	}
	return "", false
}

// StyleOfSite is the style that has this site pattern.
func (s StyleRules) StyleOfSite(site string) (string, bool) {
	for _, r := range s.Rules {
		for _, x := range r.Sites {
			if x == site {
				return r.Style, true
			}
		}
	}
	return "", false
}

// SetTargets gives style exactly these apps and sites. Each app or site
// leaves the rule that had it.
func (s *StyleRules) SetTargets(style string, apps, sites []string) {
	for i := range s.Rules {
		r := &s.Rules[i]
		r.Apps = filter(r.Apps, func(a string) bool {
			for _, b := range apps {
				if sameApp(a, b) {
					return false
				}
			}
			return true
		})
		r.Sites = filter(r.Sites, func(x string) bool {
			for _, y := range sites {
				if x == y {
					return false
				}
			}
			return true
		})
	}
	s.Rules = filterRules(s.Rules, func(r StyleRule) bool { return r.Style != style })
	s.Rules = append(s.Rules, StyleRule{Style: style, Apps: apps, Sites: sites})
	s.Rules = filterRules(s.Rules, func(r StyleRule) bool { return len(r.Apps) > 0 || len(r.Sites) > 0 })
}

// RemoveStyle drops the rule of a style.
func (s *StyleRules) RemoveStyle(style string) {
	s.Rules = filterRules(s.Rules, func(r StyleRule) bool { return r.Style != style })
}

// TurnOn turns the rules on; the first time it adds the starter apps and
// sites that no rule has.
func (s *StyleRules) TurnOn(starter []StyleRule) {
	s.Enabled = true
	if s.Seeded {
		return
	}
	s.Seeded = true
	for _, n := range starter {
		n.Apps = filter(n.Apps, func(a string) bool { _, ok := s.StyleOfApp(a); return !ok })
		n.Sites = filter(n.Sites, func(x string) bool { _, ok := s.StyleOfSite(x); return !ok })
		idx := -1
		for i, r := range s.Rules {
			if r.Style == n.Style {
				idx = i
				break
			}
		}
		switch {
		case idx >= 0:
			s.Rules[idx].Apps = append(s.Rules[idx].Apps, n.Apps...)
			s.Rules[idx].Sites = append(s.Rules[idx].Sites, n.Sites...)
		case len(n.Apps) > 0 || len(n.Sites) > 0:
			s.Rules = append(s.Rules, n)
		}
	}
}

func filter(in []string, keep func(string) bool) []string {
	out := []string{}
	for _, s := range in {
		if keep(s) {
			out = append(out, s)
		}
	}
	return out
}

func filterRules(in []StyleRule, keep func(StyleRule) bool) []StyleRule {
	out := []StyleRule{}
	for _, r := range in {
		if keep(r) {
			out = append(out, r)
		}
	}
	return out
}

// splitURL gives the lowercase host without www. and the path without its
// trailing slash. ok is false for a non-http(s) address or a host without a dot.
func splitURL(url string) (string, string, bool) {
	url = strings.TrimSpace(url)
	rest := url
	if scheme, after, found := strings.Cut(url, "://"); found {
		if !strings.EqualFold(scheme, "http") && !strings.EqualFold(scheme, "https") {
			return "", "", false
		}
		rest = after
	}
	if i := strings.IndexAny(rest, "?#"); i >= 0 {
		rest = rest[:i]
	}
	authority, path := rest, ""
	if i := strings.Index(rest, "/"); i >= 0 {
		authority, path = rest[:i], rest[i:]
	}
	host := authority
	if i := strings.LastIndex(host, "@"); i >= 0 {
		host = host[i+1:]
	}
	if i := strings.Index(host, ":"); i >= 0 {
		host = host[:i]
	}
	host = strings.TrimPrefix(asciiLower(host), "www.")
	valid := strings.Contains(host, ".") && !strings.HasPrefix(host, ".") && !strings.HasSuffix(host, ".")
	for _, c := range host {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '-') {
			valid = false
		}
	}
	if !valid {
		return "", "", false
	}
	return host, strings.TrimRight(path, "/"), true
}

// NormalizeSite gives the site pattern of typed text: host or host/path.
func NormalizeSite(text string) (string, bool) {
	if strings.IndexFunc(strings.TrimSpace(text), unicode.IsSpace) >= 0 {
		return "", false
	}
	host, path, ok := splitURL(text)
	return host + path, ok
}

// SiteChoices are the patterns to offer for a typed address, widest first.
func SiteChoices(text string) []string {
	site, ok := NormalizeSite(text)
	if !ok {
		return nil
	}
	parts := strings.SplitN(site, "/", 3)
	if len(parts) > 1 && parts[1] != "" {
		return []string{parts[0], parts[0] + "/" + parts[1]}
	}
	return []string{parts[0]}
}

func siteSpecificity(pattern, host, path string) (int, int, bool) {
	pHost, pPath := pattern, ""
	if i := strings.Index(pattern, "/"); i >= 0 {
		pHost, pPath = pattern[:i], strings.TrimRight(pattern[i:], "/")
	}
	hostFits := host == pHost || (strings.HasSuffix(host, pHost) && strings.HasSuffix(strings.TrimSuffix(host, pHost), "."))
	pathFits := pPath == "" || path == pPath || (strings.HasPrefix(path, pPath) && strings.HasPrefix(path[len(pPath):], "/"))
	return len(pHost), len(pPath), hostFits && pathFits
}

// HostOf is the host of a page, for the history entry.
func HostOf(url string) (string, bool) {
	host, _, ok := splitURL(url)
	return host, ok
}

// TypedTarget is an app or a site that typed text can be.
type TypedTarget struct {
	IsSite bool
	Value  string
}

var reverseDNSStarts = []string{"com.", "org.", "net.", "io.", "dev.", "app.", "co.", "ai."}

// TypedTargets are the targets to offer for typed text, most probable first.
func TypedTargets(text string) []TypedTarget {
	text = strings.TrimSpace(text)
	if text == "" || strings.IndexFunc(text, unicode.IsSpace) >= 0 {
		return nil
	}
	var sites []TypedTarget
	for _, s := range SiteChoices(text) {
		sites = append(sites, TypedTarget{IsSite: true, Value: s})
	}
	if strings.Contains(text, "/") {
		return sites
	}
	app := TypedTarget{Value: text}
	lower := asciiLower(text)
	for _, p := range reverseDNSStarts {
		if strings.HasPrefix(lower, p) {
			return append([]TypedTarget{app}, sites...)
		}
	}
	return append(sites, app)
}

// StarterRules are the rules added when the user turns rules on the first
// time. goos is runtime.GOOS; sites is false where Sayso cannot read the page.
func StarterRules(goos string, sites bool) []StyleRule {
	var email, message []string
	switch goos {
	case "darwin":
		email = []string{"com.apple.mail", "com.microsoft.Outlook", "com.readdle.smartemail-Mac", "com.mimestream.Mimestream", "com.superhuman.electron"}
		message = []string{"com.apple.MobileSMS", "com.tinyspeck.slackmacgap", "com.hnc.Discord", "ru.keepcoder.Telegram", "net.whatsapp.WhatsApp", "org.whispersystems.signal-desktop", "com.microsoft.teams2"}
	case "windows":
		email = []string{"outlook.exe", "olk.exe", "thunderbird.exe"}
		message = []string{"slack.exe", "discord.exe", "telegram.exe", "whatsapp.exe", "whatsapp.root.exe", "signal.exe", "ms-teams.exe"}
	default:
		email = []string{"org.mozilla.Thunderbird", "thunderbird", "org.gnome.Geary", "org.gnome.Evolution"}
		message = []string{"com.slack.Slack", "slack", "com.discordapp.Discord", "discord", "org.telegram.desktop", "org.signal.Signal", "signal-desktop"}
	}
	emailSites := []string{"mail.google.com", "outlook.office.com", "outlook.live.com", "mail.proton.me", "app.fastmail.com", "mail.yahoo.com"}
	messageSites := []string{"messenger.com", "facebook.com/messages", "web.whatsapp.com", "web.telegram.org", "app.slack.com", "discord.com/channels", "teams.microsoft.com", "messages.google.com"}
	rule := func(style string, apps, list []string) StyleRule {
		r := StyleRule{Style: style, Apps: append([]string(nil), apps...), Sites: []string{}}
		if sites {
			r.Sites = append([]string(nil), list...)
		}
		return r
	}
	return []StyleRule{rule("email", email, emailSites), rule("message", message, messageSites)}
}

func readStyleRules(v any) (StyleRules, error) {
	s := StyleRules{Rules: []StyleRule{}}
	r := newReader(v)
	r.boolean("enabled", &s.Enabled)
	r.boolean("seeded", &s.Seeded)
	for _, e := range r.tables("rules") {
		rule := StyleRule{Apps: []string{}, Sites: []string{}}
		rr := newReader(e)
		rr.str("style", &rule.Style)
		rr.strings("apps", &rule.Apps)
		rr.strings("sites", &rule.Sites)
		if rr.err != nil {
			return StyleRules{Rules: []StyleRule{}}, fmt.Errorf("rules: %w", rr.err)
		}
		s.Rules = append(s.Rules, rule)
	}
	if r.err != nil {
		return StyleRules{Rules: []StyleRule{}}, r.err
	}
	return s, nil
}

func (s StyleRules) toTable() *Table {
	t := newTable().Set("enabled", s.Enabled).Set("seeded", s.Seeded)
	rules := make([]any, 0, len(s.Rules))
	for _, r := range s.Rules {
		rt := newTable().Set("style", r.Style)
		if len(r.Apps) > 0 {
			rt.Set("apps", stringsToAny(r.Apps))
		}
		if len(r.Sites) > 0 {
			rt.Set("sites", stringsToAny(r.Sites))
		}
		rules = append(rules, rt)
	}
	return t.Set("rules", rules)
}
