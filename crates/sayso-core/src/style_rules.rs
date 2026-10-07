//! Style rules: the style of a dictation follows the app or the site that
//! the text goes into.
//!
//! The rules are part of `config.toml`, not of the style files: an app id is
//! different on each system, and a style file is the same on all of them.

use serde::{Deserialize, Serialize};

/// The apps and sites that use one style.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleRule {
    /// The id of the style.
    pub style: String,
    /// App ids: a bundle id on macOS, an exe name on Windows, and the name of
    /// a desktop entry on Linux.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
    /// Site patterns: a host with an optional path (`facebook.com/messages`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleRules {
    pub enabled: bool,
    /// The starter set was added. It is added one time only, so a rule that
    /// the user deleted does not come back.
    pub seeded: bool,
    pub rules: Vec<StyleRule>,
}

fn same_app(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl StyleRules {
    /// The style id for a dictation into `app`, on the page `url` when the
    /// app shows one. A site rule wins over an app rule, and the most
    /// specific site wins. None when no rule applies.
    pub fn resolve(&self, app: Option<&str>, url: Option<&str>) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        let by_site = url.and_then(split_url).and_then(|(host, path)| {
            self.rules
                .iter()
                .flat_map(|r| r.sites.iter().map(move |s| (r, s)))
                .filter_map(|(r, s)| site_specificity(s, &host, &path).map(|score| (score, r)))
                .max_by_key(|(score, _)| *score)
                .map(|(_, r)| r.style.as_str())
        });
        by_site.or_else(|| {
            let app = app?;
            self.rules.iter().find(|r| r.apps.iter().any(|a| same_app(a, app))).map(|r| r.style.as_str())
        })
    }

    pub fn rule(&self, style: &str) -> Option<&StyleRule> {
        self.rules.iter().find(|r| r.style == style)
    }

    /// The style that has this app, if any.
    pub fn style_of_app(&self, app: &str) -> Option<&str> {
        self.rules.iter().find(|r| r.apps.iter().any(|a| same_app(a, app))).map(|r| r.style.as_str())
    }

    /// The style that has this site pattern, if any.
    pub fn style_of_site(&self, site: &str) -> Option<&str> {
        self.rules.iter().find(|r| r.sites.iter().any(|s| s == site)).map(|r| r.style.as_str())
    }

    /// Give `style` exactly these apps and sites. An app or site has one
    /// style only, so each one leaves the rule that had it.
    pub fn set_targets(&mut self, style: &str, apps: Vec<String>, sites: Vec<String>) {
        for rule in &mut self.rules {
            rule.apps.retain(|a| !apps.iter().any(|b| same_app(a, b)));
            rule.sites.retain(|s| !sites.contains(s));
        }
        self.rules.retain(|r| r.style != style);
        self.rules.push(StyleRule { style: style.to_string(), apps, sites });
        self.rules.retain(|r| !r.apps.is_empty() || !r.sites.is_empty());
    }

    pub fn remove_style(&mut self, style: &str) {
        self.rules.retain(|r| r.style != style);
    }

    /// Turn the rules on. The first time, this adds the apps and sites of
    /// `starter` that no rule has.
    pub fn turn_on(&mut self, starter: Vec<StyleRule>) {
        self.enabled = true;
        if std::mem::replace(&mut self.seeded, true) {
            return;
        }
        for mut new in starter {
            new.apps.retain(|a| self.style_of_app(a).is_none());
            new.sites.retain(|s| self.style_of_site(s).is_none());
            match self.rules.iter_mut().find(|r| r.style == new.style) {
                Some(rule) => {
                    rule.apps.append(&mut new.apps);
                    rule.sites.append(&mut new.sites);
                }
                None if !new.apps.is_empty() || !new.sites.is_empty() => self.rules.push(new),
                None => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sites
// ---------------------------------------------------------------------------

/// The host and the path of a web address, both in the form that a site
/// pattern has: a lowercase host without `www.` and a path without the slash
/// at its end. None for an address that is not http(s), or that has no host
/// with a dot.
fn split_url(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") => rest,
        Some(_) => return None,
        None => url,
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    let valid = host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    valid.then(|| (host, path.trim_end_matches('/').to_string()))
}

/// The site pattern for a text that the user typed or pasted: `host` or
/// `host/path`. None when the text is not a site.
pub fn normalize_site(text: &str) -> Option<String> {
    if text.trim().chars().any(char::is_whitespace) {
        return None;
    }
    split_url(text).map(|(host, path)| host + &path)
}

/// The patterns to offer for a typed address, the widest first: the host,
/// then the host with the first part of the path.
pub fn site_choices(text: &str) -> Vec<String> {
    let Some(site) = normalize_site(text) else { return Vec::new() };
    let mut parts = site.splitn(3, '/');
    let host = parts.next().unwrap_or_default().to_string();
    match parts.next() {
        Some(first) if !first.is_empty() => vec![host.clone(), format!("{host}/{first}")],
        _ => vec![host],
    }
}

/// How well a pattern fits a page: the length of its host and of its path.
/// None when it does not fit. A host also fits its subdomains, and a path
/// fits the paths below it.
fn site_specificity(pattern: &str, host: &str, path: &str) -> Option<(usize, usize)> {
    let (p_host, p_path) = match pattern.find('/') {
        Some(i) => (&pattern[..i], pattern[i..].trim_end_matches('/')),
        None => (pattern, ""),
    };
    let host_fits = host == p_host || host.strip_suffix(p_host).is_some_and(|sub| sub.ends_with('.'));
    let path_fits = p_path.is_empty() || path == p_path || path.strip_prefix(p_path).is_some_and(|below| below.starts_with('/'));
    (host_fits && path_fits).then_some((p_host.len(), p_path.len()))
}

/// The host of a page, for the history entry.
pub fn host_of(url: &str) -> Option<String> {
    split_url(url).map(|(host, _)| host)
}

// ---------------------------------------------------------------------------
// Typed text
// ---------------------------------------------------------------------------

/// What a text in the "add" field can be, when it is not the name of an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedTarget {
    App(String),
    Site(String),
}

/// The first parts of an app id that are also the last parts of many hosts.
const REVERSE_DNS_STARTS: [&str; 8] = ["com.", "org.", "net.", "io.", "dev.", "app.", "co.", "ai."];

/// The targets to offer for a typed text, the most probable first.
///
/// An app id and a host look the same (`com.apple.mail`, `mail.google.com`),
/// so the user gets both and chooses. A text with `/` or `://` is a site only.
pub fn typed_targets(text: &str) -> Vec<TypedTarget> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let sites = site_choices(text).into_iter().map(TypedTarget::Site);
    if text.contains('/') {
        return sites.collect();
    }
    let app = TypedTarget::App(text.to_string());
    let lower = text.to_ascii_lowercase();
    if REVERSE_DNS_STARTS.iter().any(|s| lower.starts_with(s)) {
        std::iter::once(app).chain(sites).collect()
    } else {
        sites.chain(std::iter::once(app)).collect()
    }
}

// ---------------------------------------------------------------------------
// Starter set
// ---------------------------------------------------------------------------

/// The rules that Sayso adds when the user turns the rules on for the first
/// time. `os` is `std::env::consts::OS`. `sites` is false on a system where
/// Sayso cannot read the page of a browser.
pub fn starter_rules(os: &str, sites: bool) -> Vec<StyleRule> {
    let (email, message): (&[&str], &[&str]) = match os {
        "macos" => (
            &["com.apple.mail", "com.microsoft.Outlook", "com.readdle.smartemail-Mac", "com.mimestream.Mimestream", "com.superhuman.electron"],
            &[
                "com.apple.MobileSMS",
                "com.tinyspeck.slackmacgap",
                "com.hnc.Discord",
                "ru.keepcoder.Telegram",
                "net.whatsapp.WhatsApp",
                "org.whispersystems.signal-desktop",
                "com.microsoft.teams2",
            ],
        ),
        "windows" => (
            &["outlook.exe", "olk.exe", "thunderbird.exe"],
            &["slack.exe", "discord.exe", "telegram.exe", "whatsapp.exe", "whatsapp.root.exe", "signal.exe", "ms-teams.exe"],
        ),
        _ => (
            &["org.mozilla.Thunderbird", "thunderbird", "org.gnome.Geary", "org.gnome.Evolution"],
            &["com.slack.Slack", "slack", "com.discordapp.Discord", "discord", "org.telegram.desktop", "org.signal.Signal", "signal-desktop"],
        ),
    };
    let email_sites: &[&str] = &["mail.google.com", "outlook.office.com", "outlook.live.com", "mail.proton.me", "app.fastmail.com", "mail.yahoo.com"];
    let message_sites: &[&str] = &[
        "messenger.com",
        "facebook.com/messages",
        "web.whatsapp.com",
        "web.telegram.org",
        "app.slack.com",
        "discord.com/channels",
        "teams.microsoft.com",
        "messages.google.com",
    ];
    let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let rule = |style: &str, apps: &[&str], site_list: &[&str]| StyleRule {
        style: style.into(),
        apps: owned(apps),
        sites: if sites { owned(site_list) } else { Vec::new() },
    };
    vec![rule("email", email, email_sites), rule("message", message, message_sites)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> StyleRules {
        StyleRules {
            enabled: true,
            seeded: true,
            rules: vec![
                StyleRule { style: "email".into(), apps: vec!["com.apple.mail".into()], sites: vec!["mail.google.com".into()] },
                StyleRule {
                    style: "message".into(),
                    apps: vec!["com.tinyspeck.slackmacgap".into()],
                    sites: vec!["facebook.com/messages".into(), "google.com".into()],
                },
                StyleRule { style: "raw".into(), apps: vec!["com.google.Chrome".into()], sites: vec![] },
            ],
        }
    }

    #[test]
    fn an_app_rule_gives_its_style() {
        let r = rules();
        assert_eq!(r.resolve(Some("com.apple.mail"), None), Some("email"));
        // An app id has no fixed case on macOS.
        assert_eq!(r.resolve(Some("com.apple.Mail"), None), Some("email"));
        assert_eq!(r.resolve(Some("com.apple.Notes"), None), None);
        assert_eq!(r.resolve(None, None), None);
    }

    #[test]
    fn a_site_rule_wins_over_the_rule_of_the_browser() {
        let r = rules();
        assert_eq!(r.resolve(Some("com.google.Chrome"), Some("https://mail.google.com/mail/u/0/#inbox")), Some("email"));
        // No site rule fits: the rule of the app applies.
        assert_eq!(r.resolve(Some("com.google.Chrome"), Some("https://example.org/")), Some("raw"));
        // A page that is not a web page does not count.
        assert_eq!(r.resolve(Some("com.google.Chrome"), Some("file:///Users/a/mail.google.com")), Some("raw"));
    }

    #[test]
    fn the_most_specific_site_wins() {
        let r = rules();
        // `mail.google.com` is longer than `google.com`.
        assert_eq!(r.resolve(None, Some("https://mail.google.com/")), Some("email"));
        assert_eq!(r.resolve(None, Some("https://docs.google.com/document/d/1")), Some("message"));
    }

    #[test]
    fn a_site_fits_subdomains_and_paths_below_it() {
        let r = rules();
        assert_eq!(r.resolve(None, Some("https://www.facebook.com/messages/t/123?x=1")), Some("message"));
        assert_eq!(r.resolve(None, Some("https://m.facebook.com/messages")), Some("message"));
        // Not below the path, and not a subdomain.
        assert_eq!(r.resolve(None, Some("https://facebook.com/messagesfoo")), None);
        assert_eq!(r.resolve(None, Some("https://facebook.com/")), None);
        assert_eq!(r.resolve(None, Some("https://notfacebook.com/messages")), None);
    }

    #[test]
    fn rules_that_are_off_give_no_style() {
        let mut r = rules();
        r.enabled = false;
        assert_eq!(r.resolve(Some("com.apple.mail"), Some("https://mail.google.com/")), None);
    }

    #[test]
    fn an_app_moves_to_the_style_that_takes_it() {
        let mut r = rules();
        r.set_targets("email", vec!["com.apple.mail".into(), "com.tinyspeck.slackmacgap".into()], vec!["google.com".into()]);
        assert_eq!(r.style_of_app("com.tinyspeck.slackmacgap"), Some("email"));
        assert_eq!(r.style_of_site("google.com"), Some("email"));
        // The old site of the style is gone, because the new list does not have it.
        assert_eq!(r.style_of_site("mail.google.com"), None);
        assert_eq!(r.rule("message").unwrap().apps, Vec::<String>::new());
        // A style with no targets has no rule.
        r.set_targets("message", vec![], vec![]);
        assert!(r.rule("message").is_none());
    }

    #[test]
    fn the_starter_set_is_added_one_time_and_keeps_the_rules_of_the_user() {
        let mut r = StyleRules { rules: vec![StyleRule { style: "raw".into(), apps: vec!["com.apple.mail".into()], sites: vec![] }], ..Default::default() };
        r.turn_on(starter_rules("macos", true));
        assert!(r.enabled && r.seeded);
        // The user's rule keeps its app.
        assert_eq!(r.style_of_app("com.apple.mail"), Some("raw"));
        assert_eq!(r.style_of_app("com.microsoft.Outlook"), Some("email"));
        assert_eq!(r.style_of_site("facebook.com/messages"), Some("message"));

        r.remove_style("email");
        r.enabled = false;
        r.turn_on(starter_rules("macos", true));
        assert!(r.rule("email").is_none(), "a deleted rule does not come back");
    }

    #[test]
    fn the_starter_set_has_no_sites_where_sayso_cannot_read_them() {
        assert!(starter_rules("linux", false).iter().all(|r| r.sites.is_empty() && !r.apps.is_empty()));
    }

    #[test]
    fn a_typed_address_becomes_a_site_pattern() {
        assert_eq!(normalize_site("https://www.Fastmail.com/").as_deref(), Some("fastmail.com"));
        assert_eq!(normalize_site("app.fastmail.com:443/mail/Inbox/?a=1#b").as_deref(), Some("app.fastmail.com/mail/Inbox"));
        assert_eq!(normalize_site("localhost"), None);
        assert_eq!(normalize_site("ftp://example.com"), None);
        assert_eq!(normalize_site("two words.com"), None);
        assert_eq!(site_choices("https://app.fastmail.com/mail/Inbox"), ["app.fastmail.com", "app.fastmail.com/mail"]);
        assert_eq!(site_choices("github.com"), ["github.com"]);
        assert_eq!(host_of("https://mail.google.com/mail/u/0").as_deref(), Some("mail.google.com"));
    }

    #[test]
    fn typed_text_offers_an_app_id_and_a_site_in_the_probable_order() {
        use TypedTarget::{App, Site};
        assert_eq!(typed_targets("com.superhuman.electron"), [App("com.superhuman.electron".into()), Site("com.superhuman.electron".into())]);
        assert_eq!(typed_targets("mail.google.com"), [Site("mail.google.com".into()), App("mail.google.com".into())]);
        assert_eq!(typed_targets("https://x.com/messages"), [Site("x.com".into()), Site("x.com/messages".into())]);
        // No dot: it cannot be a site.
        assert_eq!(typed_targets("firefox"), [App("firefox".into())]);
        assert_eq!(typed_targets("code.exe"), [Site("code.exe".into()), App("code.exe".into())]);
        assert!(typed_targets("two words").is_empty());
    }
}
