//! Site icons for badges, by host.
//!
//! The icon of a site comes from the site itself: Sayso reads the page at
//! `https://<host>/` for its `<link rel="icon">` tags, then tries
//! `/favicon.ico`. Some sites refuse each client that is not a browser
//! (ChatGPT, for example). For a site that answers and gives no icon, Sayso
//! asks the icon service of DuckDuckGo, which then learns the host. A host
//! that looks private, and a site that does not answer, never go to the
//! service. The result is a PNG in `<cache>/site-icons`.
//!
//! A view asks while it renders, so the answer is what the disk has now. A
//! file older than [`MAX_AGE`] (or no file) starts one fetch on the worker
//! thread, and the old icon shows until the new one comes. A site with no
//! icon gets an empty file, so Sayso asks it again only after [`RETRY_AGE`].
//! A site that does not answer gets no file: the next run tries again.

use crate::icons::{cache_file, is_fresh};
use gpui_kit::{Image, ImageFormat};
use regex::Regex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// Pixel size of the cached PNG. The icon shows at 12 to 14 points.
const ICON_PX: u32 = 64;
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long Sayso waits before it asks a site with no icon again.
const RETRY_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const PAGE_LIMIT: usize = 512 * 1024;
const IMAGE_LIMIT: usize = 1024 * 1024;
/// The most icon addresses that Sayso tries for one site.
const MAX_CANDIDATES: usize = 4;
/// The icon service for a site that gives no icon: this address, the host, and `.ico`.
const ICON_SERVICE: &str = "https://icons.duckduckgo.com/ip3/";

/// A new icon from the worker thread: the host and its PNG.
pub type Fetched = (String, Vec<u8>);

pub struct SiteIcons {
    dir: PathBuf,
    /// Main thread only: icons are read while views render.
    memory: RefCell<HashMap<String, Option<Arc<Image>>>>,
    queue: crossbeam_channel::Sender<String>,
}

impl SiteIcons {
    /// The icons in `dir`, and the channel on which new icons come. Give each
    /// of them to [`SiteIcons::fetched`].
    pub fn new(dir: PathBuf) -> (Self, async_channel::Receiver<Fetched>) {
        let (queue, hosts) = crossbeam_channel::unbounded::<String>();
        let (done, fetched) = async_channel::unbounded();
        let worker_dir = dir.clone();
        let worker = std::thread::Builder::new().name("site-icons".into()).spawn(move || {
            for host in hosts {
                if let Some(png) = refresh(&worker_dir, &host, &http_get)
                    && done.send_blocking((host, png)).is_err()
                {
                    break;
                }
            }
        });
        if let Err(e) = worker {
            log::warn!("no thread for site icons: {e}");
        }
        (Self { dir, memory: RefCell::default(), queue }, fetched)
    }

    /// The icon of `host` that Sayso has now, or None. The first request for
    /// a host in a run starts a fetch when the file is old or missing.
    pub fn get(&self, host: &str) -> Option<Arc<Image>> {
        if !is_host(host) {
            return None;
        }
        if let Some(hit) = self.memory.borrow().get(host) {
            return hit.clone();
        }
        let file = cache_file(&self.dir, host);
        let png = std::fs::read(&file).ok();
        let max_age = if png.as_ref().is_some_and(|b| b.is_empty()) { RETRY_AGE } else { MAX_AGE };
        if !is_fresh(&file, max_age) {
            let _ = self.queue.send(host.to_string());
        }
        let icon = png.filter(|b| !b.is_empty()).map(|b| Arc::new(Image::from_bytes(ImageFormat::Png, b)));
        self.memory.borrow_mut().insert(host.to_string(), icon.clone());
        icon
    }

    /// Show a new icon from the worker thread.
    pub fn fetched(&self, host: &str, png: Vec<u8>) {
        self.memory.borrow_mut().insert(host.to_string(), Some(Arc::new(Image::from_bytes(ImageFormat::Png, png))));
    }
}

/// A host as a history entry stores it: letters, digits, dots, and hyphens.
fn is_host(host: &str) -> bool {
    host.contains('.') && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// The answer of a server.
struct Page {
    /// The address after redirects.
    url: String,
    status: u16,
    body: Vec<u8>,
}

/// Get `url`, with at most `limit` bytes of its body. None when the server
/// did not answer.
type Get<'a> = &'a dyn Fn(&str, usize) -> Option<Page>;

#[derive(Debug, PartialEq)]
enum Lookup {
    Found(Vec<u8>),
    /// The site answered, and it has no icon that Sayso can show.
    NoIcon,
    Unreachable,
}

/// Fetch the icon of `host` and write it to the cache. Some for a new icon.
fn refresh(dir: &Path, host: &str, get: Get) -> Option<Vec<u8>> {
    let file = cache_file(dir, host);
    let write = |bytes: &[u8]| {
        let _ = std::fs::create_dir_all(dir);
        if let Err(e) = std::fs::write(&file, bytes) {
            log::debug!("could not cache the icon of {host}: {e}");
        }
    };
    match find_icon(host, get) {
        Lookup::Found(png) => {
            write(&png);
            Some(png)
        }
        Lookup::NoIcon => {
            // An icon from before stays: a stale icon is better than none.
            if !std::fs::metadata(&file).is_ok_and(|m| m.len() > 0) {
                write(&[]);
            }
            None
        }
        Lookup::Unreachable => None,
    }
}

fn find_icon(host: &str, get: Get) -> Lookup {
    // A history entry stores the host without `www.`, and some sites answer
    // only with it.
    let Some(page) = get(&format!("https://{host}/"), PAGE_LIMIT).or_else(|| get(&format!("https://www.{host}/"), PAGE_LIMIT)) else {
        return Lookup::Unreachable;
    };
    let mut candidates = if page.status == 200 { link_icons(&String::from_utf8_lossy(&page.body), &page.url) } else { Vec::new() };
    candidates.extend(join(&page.url, "/favicon.ico"));
    let mut tried: Vec<&String> = Vec::new();
    for url in &candidates {
        if tried.contains(&url) {
            continue;
        }
        if tried.len() == MAX_CANDIDATES {
            break;
        }
        tried.push(url);
        let png = get(url, IMAGE_LIMIT).filter(|p| p.status == 200).and_then(|p| normalize(&p.body));
        if let Some(png) = png {
            return Lookup::Found(png);
        }
    }
    let service = public_host(host).then(|| get(&format!("{ICON_SERVICE}{host}.ico"), IMAGE_LIMIT)).flatten();
    match service.filter(|p| p.status == 200).and_then(|p| normalize(&p.body)) {
        Some(png) => Lookup::Found(png),
        None => Lookup::NoIcon,
    }
}

/// False for a host that can be a name on a private network: an IP address,
/// or a name below a top-level domain that is not public.
fn public_host(host: &str) -> bool {
    const PRIVATE: [&str; 9] = ["local", "localhost", "internal", "intranet", "lan", "home", "corp", "test", "arpa"];
    let top = host.rsplit('.').next().unwrap_or("");
    !top.is_empty() && !top.chars().all(|c| c.is_ascii_digit()) && !PRIVATE.contains(&top.to_ascii_lowercase().as_str())
}

/// `href` as an https address, seen from the page at `base`.
fn join(base: &str, href: &str) -> Option<String> {
    let url = url::Url::parse(base).ok()?.join(href.trim()).ok()?;
    (url.scheme() == "https").then(|| url.into())
}

/// The addresses of the icons that the page names, best first. The best icon
/// is the smallest one that is at least [`ICON_PX`] wide, then the largest
/// of the smaller ones. An SVG icon is left out: the cache holds PNG only.
fn link_icons(html: &str, base: &str) -> Vec<String> {
    static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<link\b[^>]*>").unwrap());
    static ATTRIBUTE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?is)([a-z-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).unwrap());

    let mut icons: Vec<(u32, String)> = Vec::new();
    for link in LINK.find_iter(html) {
        let attribute = |name: &str| {
            ATTRIBUTE
                .captures_iter(link.as_str())
                .find(|c| c[1].eq_ignore_ascii_case(name))
                .and_then(|c| c.get(2).or(c.get(3)).or(c.get(4)))
                .map(|m| m.as_str().trim().to_ascii_lowercase())
        };
        let Some(rel) = attribute("rel") else { continue };
        let apple = rel.split_whitespace().any(|r| r == "apple-touch-icon" || r == "apple-touch-icon-precomposed");
        if !apple && !rel.split_whitespace().any(|r| r == "icon") {
            continue;
        }
        if attribute("type").is_some_and(|t| t.contains("svg")) {
            continue;
        }
        // The address keeps its case, so read it again without the lowercase.
        let href = ATTRIBUTE
            .captures_iter(link.as_str())
            .find(|c| c[1].eq_ignore_ascii_case("href"))
            .and_then(|c| c.get(2).or(c.get(3)).or(c.get(4)))
            .map(|m| m.as_str().replace("&amp;", "&"));
        let Some(url) = href.and_then(|h| join(base, &h)) else { continue };
        if url.split(['?', '#']).next().is_some_and(|path| path.to_ascii_lowercase().ends_with(".svg")) {
            continue;
        }
        // Without `sizes`, a touch icon is 180 wide and a plain icon is small.
        let declared = attribute("sizes").and_then(|s| s.split(['x', ' ']).next().and_then(|w| w.parse::<u32>().ok()));
        icons.push((declared.unwrap_or(if apple { 180 } else { 32 }), url));
    }
    icons.sort_by_key(|(width, _)| if *width >= ICON_PX { (0, *width) } else { (1, u32::MAX - *width) });
    icons.into_iter().map(|(_, url)| url).collect()
}

/// The image as a PNG of at most [`ICON_PX`] on each side. None when the
/// bytes are not an image that Sayso can read.
fn normalize(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut icon = reader.decode().ok()?;
    if icon.width() == 0 || icon.height() == 0 {
        return None;
    }
    if icon.width() > ICON_PX || icon.height() > ICON_PX {
        icon = icon.resize(ICON_PX, ICON_PX, image::imageops::FilterType::CatmullRom);
    }
    let mut png = Vec::new();
    icon.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

/// One request with no cookies and no referrer. Some sites refuse a client
/// that does not name a browser engine.
fn http_get(url: &str, limit: usize) -> Option<Page> {
    use ureq::ResponseExt as _;
    static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
        let config = ureq::Agent::config_builder()
            .user_agent(format!("Mozilla/5.0 (compatible; Sayso/{})", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .max_redirects(5)
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build();
        ureq::Agent::new_with_config(config)
    });
    let mut response = AGENT.get(url).call().map_err(|e| log::debug!("site icon: {url}: {e}")).ok()?;
    let (status, url) = (response.status().as_u16(), response.get_uri().to_string());
    let mut body = Vec::new();
    // A page is cut at the limit: its icons are in the head.
    response.body_mut().as_reader().take(limit as u64).read_to_end(&mut body).ok()?;
    Some(Page { url, status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(side: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::new_rgba8(side, side).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn side_of(png: &[u8]) -> u32 {
        image::load_from_memory(png).unwrap().width()
    }

    type Asked = Arc<parking_lot::Mutex<Vec<String>>>;

    /// A site from a list of addresses and their answers. It records each request.
    fn site(pages: Vec<(&'static str, u16, Vec<u8>)>) -> (impl Fn(&str, usize) -> Option<Page>, Asked) {
        let asked = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let log = asked.clone();
        let get = move |url: &str, _: usize| {
            log.lock().push(url.to_string());
            pages.iter().find(|p| p.0 == url).map(|p| Page { url: url.to_string(), status: p.1, body: p.2.clone() })
        };
        (get, asked)
    }

    #[test]
    fn link_icons_come_best_first() {
        let html = r#"<head>
            <LINK REL="shortcut icon" HREF="/small.ico">
            <link rel="icon" type="image/svg+xml" href="/icon.svg">
            <link rel=icon sizes=96x96 href=/Icon-96.png?v=2&amp;x=1>
            <link rel='icon' sizes='512x512' href='https://cdn.example.com/512.png'>
            <link rel="apple-touch-icon" href="touch.png">
            <link rel="mask-icon" href="/mask.png">
            <link rel="stylesheet" href="/site.css">
            <link rel="icon" href="http://example.com/plain.png">
            <link rel="icon" href="data:image/png;base64,AAAA">
        </head>"#;
        assert_eq!(
            link_icons(html, "https://example.com/app/"),
            [
                "https://example.com/Icon-96.png?v=2&x=1",
                "https://example.com/app/touch.png",
                "https://cdn.example.com/512.png",
                "https://example.com/small.ico",
            ]
        );
    }

    #[test]
    fn the_icon_of_the_page_wins_over_favicon_ico() {
        let (get, asked) = site(vec![
            ("https://example.com/", 200, br#"<link rel="icon" sizes="128x128" href="/big.png">"#.to_vec()),
            ("https://example.com/big.png", 200, png(128)),
            ("https://example.com/favicon.ico", 200, png(16)),
        ]);
        let Lookup::Found(icon) = find_icon("example.com", &get) else { panic!("no icon") };
        assert_eq!(side_of(&icon), ICON_PX, "a large icon is made smaller");
        assert_eq!(*asked.lock(), ["https://example.com/", "https://example.com/big.png"]);
    }

    #[test]
    fn favicon_ico_is_the_fallback_also_for_a_page_that_fails() {
        let (get, _) = site(vec![
            ("https://example.com/", 200, br#"<link rel="icon" href="/broken.png">"#.to_vec()),
            ("https://example.com/broken.png", 200, b"not an image".to_vec()),
            ("https://example.com/favicon.ico", 200, png(16)),
        ]);
        let Lookup::Found(icon) = find_icon("example.com", &get) else { panic!("no icon") };
        assert_eq!(side_of(&icon), 16, "a small icon keeps its size");

        let (get, _) = site(vec![("https://example.com/", 403, Vec::new()), ("https://example.com/favicon.ico", 200, png(16))]);
        assert!(matches!(find_icon("example.com", &get), Lookup::Found(_)));
    }

    #[test]
    fn a_site_that_answers_only_with_www_is_found() {
        let (get, _) = site(vec![("https://www.example.com/", 200, Vec::new()), ("https://www.example.com/favicon.ico", 200, png(16))]);
        assert!(matches!(find_icon("example.com", &get), Lookup::Found(_)));
    }

    #[test]
    fn a_site_tells_no_icon_from_no_answer() {
        let (get, asked) = site(vec![("https://example.com/", 200, Vec::new())]);
        assert_eq!(find_icon("example.com", &get), Lookup::NoIcon);
        assert_eq!(asked.lock().len(), 3, "the page, favicon.ico, and the icon service");

        let (get, _) = site(Vec::new());
        assert_eq!(find_icon("example.com", &get), Lookup::Unreachable);
    }

    #[test]
    fn the_icon_service_is_for_a_public_site_that_refuses() {
        let service = "https://icons.duckduckgo.com/ip3/example.com.ico";
        let (get, asked) = site(vec![("https://example.com/", 403, Vec::new()), ("https://example.com/favicon.ico", 403, Vec::new()), (service, 200, png(32))]);
        assert!(matches!(find_icon("example.com", &get), Lookup::Found(_)));
        assert_eq!(asked.lock().last().unwrap(), service);

        // The site has an icon: the service does not learn the host.
        let (get, asked) = site(vec![("https://example.com/", 200, Vec::new()), ("https://example.com/favicon.ico", 200, png(16))]);
        assert!(matches!(find_icon("example.com", &get), Lookup::Found(_)));
        assert!(asked.lock().iter().all(|url| url.starts_with("https://example.com/")));

        // A private name and a site that does not answer stay on this computer.
        for host in ["wiki.corp", "printer.local", "192.168.1.1"] {
            let (get, asked) = site(vec![("https://wiki.corp/", 200, Vec::new()), ("https://printer.local/", 200, Vec::new()), ("https://192.168.1.1/", 200, Vec::new())]);
            assert_eq!(find_icon(host, &get), Lookup::NoIcon);
            assert!(asked.lock().iter().all(|url| !url.contains("duckduckgo")), "{host}");
        }
        let (get, asked) = site(Vec::new());
        assert_eq!(find_icon("example.com", &get), Lookup::Unreachable);
        assert!(asked.lock().iter().all(|url| !url.contains("duckduckgo")));
    }

    #[test]
    fn refresh_writes_the_icon_or_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let (get, _) = site(vec![("https://a.example/", 200, Vec::new()), ("https://a.example/favicon.ico", 200, png(16))]);
        assert!(refresh(dir.path(), "a.example", &get).is_some());
        assert_eq!(side_of(&std::fs::read(dir.path().join("a.example.png")).unwrap()), 16);

        // The site lost its icon: the file from before stays.
        let (get, _) = site(vec![("https://a.example/", 200, Vec::new())]);
        assert!(refresh(dir.path(), "a.example", &get).is_none());
        assert!(!std::fs::read(dir.path().join("a.example.png")).unwrap().is_empty());

        let (get, _) = site(vec![("https://b.example/", 200, Vec::new())]);
        assert!(refresh(dir.path(), "b.example", &get).is_none());
        assert!(std::fs::read(dir.path().join("b.example.png")).unwrap().is_empty(), "the mark of a site with no icon");

        let (get, _) = site(Vec::new());
        assert!(refresh(dir.path(), "c.example", &get).is_none());
        assert!(!dir.path().join("c.example.png").exists(), "a site that did not answer is tried again");
    }

    #[test]
    fn get_reads_the_disk_and_asks_one_time_for_a_missing_icon() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fresh.example.png"), png(16)).unwrap();
        std::fs::write(dir.path().join("none.example.png"), []).unwrap();
        // The test has its own queue, so no request leaves the computer.
        let (queue, hosts) = crossbeam_channel::unbounded();
        let icons = SiteIcons { dir: dir.path().to_path_buf(), memory: RefCell::default(), queue };

        assert!(icons.get("fresh.example").is_some());
        assert!(icons.get("none.example").is_none());
        assert!(hosts.try_recv().is_err(), "fresh files start no fetch");

        assert!(icons.get("new.example").is_none());
        assert!(icons.get("new.example").is_none());
        assert_eq!(hosts.try_iter().collect::<Vec<_>>(), ["new.example"]);

        icons.fetched("new.example", png(16));
        assert!(icons.get("new.example").is_some());

        assert!(icons.get("../etc").is_none());
        assert!(icons.get("localhost").is_none());
        assert!(hosts.try_recv().is_err(), "a text that is not a host starts no fetch");
    }
}
