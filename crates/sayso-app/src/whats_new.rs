//! The "What is new" window. Sayso shows it one time, at the first start of
//! a version that has an entry in `release-notes.toml`.
//!
//! The notes are part of the build, so the window needs no network. It does
//! not depend on the updater: an update from Homebrew, a package, or an
//! installer shows it too. `state.json` keeps the last version that Sayso
//! showed (or had nothing to show for) under `whats_new`.

use crate::app::Windows;
use crate::model::AppModel;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::*;
use sayso_core::paths::Paths;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::texture::{Grain, grain};
use sayso_ui::{ActivePaper, Colors, text};
use sayso_update::manifest::Version;
use serde::Deserialize;
use std::sync::LazyLock;

const NOTES: &str = include_str!("../../../release-notes.toml");
const STATE_KEY: &str = "whats_new";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Notes {
    release: Vec<Release>,
}

/// The notes of one version.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: Version,
    pub title: String,
    #[serde(default)]
    pub item: Vec<Item>,
    pub note: Option<Note>,
}

/// One new feature.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub icon: String,
    pub title: String,
    pub text: String,
    /// The systems that have the feature. Empty for all systems.
    #[serde(default)]
    pub systems: Vec<String>,
}

/// An announcement or a request, with one button that opens `url`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub title: String,
    pub text: String,
    pub action: String,
    pub url: String,
}

fn parse(text: &str) -> Result<Vec<Release>, toml::de::Error> {
    toml::from_str::<Notes>(text).map(|notes| notes.release)
}

static RELEASES: LazyLock<Vec<Release>> = LazyLock::new(|| {
    parse(NOTES).unwrap_or_else(|e| {
        log::error!("release-notes.toml is not readable: {e}");
        Vec::new()
    })
});

/// The icon of an item. A test makes sure that each name in the file is here.
fn icon_named(name: &str) -> Option<Icon> {
    Some(match name {
        "sparkle" => Icon::Sparkle,
        "lock" => Icon::Lock,
        "shield" => Icon::Shield,
        "history" => Icon::History,
        "search" => Icon::Search,
        "mic" => Icon::Mic,
        "audio" => Icon::Audio,
        "styles" => Icon::Styles,
        "models" => Icon::Models,
        "dictionary" => Icon::Dictionary,
        "keyboard" => Icon::Keyboard,
        "overlay" => Icon::Overlay,
        "cloud" => Icon::Cloud,
        "download" => Icon::Download,
        "settings" => Icon::Settings,
        "code" => Icon::Code,
        _ => return None,
    })
}

/// The notes of `version` for the system `os`, without the items of other
/// systems. None when nothing is left to show.
fn release_in(releases: &[Release], version: &Version, os: &str) -> Option<Release> {
    let mut release = releases.iter().find(|r| &r.version == version)?.clone();
    release.item.retain(|item| item.systems.is_empty() || item.systems.iter().any(|s| s == os));
    (!release.item.is_empty() || release.note.is_some()).then_some(release)
}

fn running() -> Option<Version> {
    sayso_update::VERSION.parse().ok()
}

/// The notes of this build, when it has some.
pub fn current() -> Option<Release> {
    release_in(&RELEASES, &running()?, std::env::consts::OS)
}

/// True when this start is the first one of a newer version. A first install
/// is not an update: onboarding records the version (see [`mark_seen`]).
fn is_due(seen: Option<&Version>, running: &Version) -> bool {
    seen.is_none_or(|seen| seen.cmp_precedence(running).is_lt())
}

fn read_state(paths: &Paths) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(paths.state_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn seen(paths: &Paths) -> Option<Version> {
    read_state(paths).get(STATE_KEY)?.as_str()?.parse().ok()
}

/// Record that the user has nothing more to see for this version.
pub fn mark_seen(paths: &Paths) {
    let mut state = read_state(paths);
    state.insert(STATE_KEY.into(), sayso_update::VERSION.into());
    let written = serde_json::to_string_pretty(&state).map_err(std::io::Error::other).and_then(|text| std::fs::write(paths.state_file(), text));
    if let Err(e) = written {
        log::warn!("could not write {}: {e}", paths.state_file().display());
    }
}

/// At start, after onboarding is done: show the window when this is the first
/// start of a newer version that has notes.
pub fn show_after_update(model: &Entity<AppModel>, cx: &mut App) {
    let Some(running) = running() else { return };
    let paths = model.read(cx).paths.clone();
    let seen = seen(&paths);
    if seen.as_ref() == Some(&running) {
        return;
    }
    mark_seen(&paths);
    if is_due(seen.as_ref(), &running) {
        open(model, cx);
    }
}

/// Open the window, or bring it to the front. Does nothing when this build
/// has no notes.
pub fn open(model: &Entity<AppModel>, cx: &mut App) {
    open_release(model, current(), cx);
}

/// `--whats-new`: open the window with the newest notes in the file, so the
/// notes of the next version show before `Cargo.toml` has that version.
pub fn open_newest(model: &Entity<AppModel>, cx: &mut App) {
    let newest = RELEASES.first().and_then(|r| release_in(&RELEASES, &r.version, std::env::consts::OS));
    open_release(model, newest, cx);
}

fn open_release(model: &Entity<AppModel>, release: Option<Release>, cx: &mut App) {
    if let Some(handle) = cx.global::<Windows>().whats_new
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        crate::app::appkit_later(cx, crate::shell::show_app_and_activate);
        return;
    }
    let Some(release) = release else { return };
    let bounds = Bounds::centered(None, size(px(560.), px(760.)), cx);
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("What is new in Sayso".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(18.), px(18.))),
            }),
            is_resizable: false,
            focus: true,
            // The window class on Linux, which matches the desktop entry.
            app_id: Some(crate::shell::APP_ID.into()),
            window_decorations: crate::chrome::decorations(&model.read(cx).config),
            window_background: crate::chrome::background(),
            show: true,
            inactive_frame_interval: None,
            ..Default::default()
        },
        cx,
        |window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.global_mut::<Windows>().whats_new = None;
                true
            });
            cx.new(|_| WhatsNewView { release })
        },
    );
    match opened {
        Ok((handle, _)) => {
            cx.global_mut::<Windows>().whats_new = Some(handle);
            crate::app::appkit_later(cx, crate::shell::show_app_and_activate);
        }
        Err(e) => log::error!("could not open the What is new window: {e:#}"),
    }
}

fn close(window: &mut Window, cx: &mut App) {
    cx.global_mut::<Windows>().whats_new = None;
    window.remove_window();
}

pub struct WhatsNewView {
    release: Release,
}

impl WhatsNewView {
    fn top_bar(&self, window: &Window, cx: &App) -> AnyElement {
        let header = div().id("whats-new-title").flex().flex_none().items_center().justify_end().h(px(52.)).px(px(18.));
        if crate::chrome::is_custom(window) {
            // The custom title bar (Linux): the header moves the window.
            return crate::chrome::drag_area(header).child(crate::chrome::window_controls(window, cx, close)).into_any_element();
        }
        header.into_any_element()
    }

    fn item(item: &Item, c: &Colors) -> Div {
        div()
            .flex()
            .items_start()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded(px(10.))
                    .debossed(c)
                    .child(icon(icon_named(&item.icon).unwrap_or(Icon::Sparkle), 18., c.ink)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(text::ui(item.title.clone(), 15., FontWeight::SEMIBOLD, c.ink).line_height(px(20.)))
                    .child(text::ui(item.text.clone(), 14., FontWeight::NORMAL, c.graphite).line_height(px(20.))),
            )
    }

    fn note(note: &Note, c: &Colors) -> Div {
        let url = note.url.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(16.))
            .py(px(14.))
            .px(px(16.))
            .rounded(px(14.))
            .raised(c)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(text::ui(note.title.clone(), 14., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
                    .child(text::ui(note.text.clone(), 13., FontWeight::NORMAL, c.graphite).line_height(px(19.))),
            )
            .child(Button::new("whats-new-note", note.action.clone()).small().on_click(move |_, _, _| crate::shell::open(&url)))
    }
}

impl Render for WhatsNewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let release = &self.release;
        let notes = crate::update_ui::release_notes(&release.version);
        let heading = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(text::ui(format!("NEW IN SAYSO {}", release.version), 12., FontWeight::SEMIBOLD, c.accent).line_height(px(16.)))
            .child(text::title(release.title.clone(), 36., &c).line_height(px(42.)));
        let items = div().flex().flex_col().gap(px(20.)).children(release.item.iter().map(|item| Self::item(item, &c)));
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(68.))
            .pl(px(44.))
            .pr(px(24.))
            .border_t_1()
            .border_color(c.rule)
            .child(
                div()
                    .id("whats-new-all")
                    .cursor_pointer()
                    .on_click(move |_, _, _| crate::shell::open(&notes))
                    .child(text::ui("See all changes on GitHub", 13., FontWeight::SEMIBOLD, c.accent)),
            )
            .child(Button::new("whats-new-done", "Done").primary().large().on_click(|_, window, cx| close(window, cx)));
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .p(px(10.))
            .pt(px(crate::caption::top_margin(10.)))
            .bg(c.ground)
            .font_family(sayso_ui::fonts::UI)
            .text_color(c.ink)
            .child(grain(Grain::Board, px(0.), cx))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(12.))
                    .bg(c.sheet)
                    .shadow(sayso_ui::paper::sheet(&c))
                    .child(grain(Grain::Sheet, px(12.), cx))
                    .child(self.top_bar(window, cx))
                    .child(
                        div()
                            .id("whats-new-items")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .gap(px(28.))
                            .px(px(44.))
                            .pt(px(12.))
                            .pb(px(28.))
                            .child(heading)
                            .child(items)
                            // The note sits at the bottom when the items leave room.
                            .child(div().flex_1())
                            .children(release.note.as_ref().map(|note| Self::note(note, &c)))
                            .overflow_y_scrollbar(),
                    )
                    .child(footer),
            )
            .children(crate::caption::caption_bar(false, cx))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in the `test` attribute of GPUI.
    use super::{Version, icon_named, is_due, mark_seen, parse, release_in, seen};
    use sayso_core::paths::{PathSource, Paths};

    fn version(text: &str) -> Version {
        text.parse().unwrap()
    }

    const SAMPLE: &str = r#"
        [[release]]
        version = "0.5.0"
        title = "Two things"
        [[release.item]]
        icon = "sparkle"
        title = "Only on the Mac"
        text = "x"
        systems = ["macos"]
        [[release.item]]
        icon = "lock"
        title = "On all systems"
        text = "x"

        [[release]]
        version = "0.4.0"
        title = "Only for the Mac"
        [[release.item]]
        icon = "sparkle"
        title = "Only on the Mac"
        text = "x"
        systems = ["macos"]
    "#;

    #[test]
    fn the_notes_in_the_repository_are_correct() {
        let releases = parse(super::NOTES).unwrap();
        let running = version(sayso_update::VERSION);
        for pair in releases.windows(2) {
            assert!(pair[0].version > pair[1].version, "{} must come before {}", pair[0].version, pair[1].version);
        }
        for release in &releases {
            // The next version gets its notes before Cargo.toml gets the version.
            assert!(!release.title.is_empty(), "{}", release.version);
            assert!(release.version.pre.is_empty(), "{}", release.version);
            for item in &release.item {
                assert!(icon_named(&item.icon).is_some(), "{}: no icon \"{}\"", release.version, item.icon);
                assert!(!item.title.is_empty() && !item.text.is_empty(), "{}", release.version);
                for system in &item.systems {
                    assert!(["macos", "windows", "linux"].contains(&system.as_str()), "{}: no system \"{system}\"", release.version);
                }
            }
            if let Some(note) = &release.note {
                assert!(note.url.starts_with("https://"), "{}: {}", release.version, note.url);
                assert!(!note.title.is_empty() && !note.text.is_empty() && !note.action.is_empty(), "{}", release.version);
            }
        }
        // More than one version ahead is a typing error.
        if let Some(newest) = releases.first() {
            assert!(newest.version.major <= running.major + 1, "{}", newest.version);
        }
    }

    #[test]
    fn a_release_shows_only_the_items_of_this_system() {
        let releases = parse(SAMPLE).unwrap();
        let titles = |os: &str| release_in(&releases, &version("0.5.0"), os).unwrap().item.into_iter().map(|i| i.title).collect::<Vec<_>>();
        assert_eq!(titles("macos"), ["Only on the Mac", "On all systems"]);
        assert_eq!(titles("linux"), ["On all systems"]);
    }

    #[test]
    fn a_release_with_nothing_for_this_system_shows_no_window() {
        let releases = parse(SAMPLE).unwrap();
        assert!(release_in(&releases, &version("0.4.0"), "macos").is_some());
        assert!(release_in(&releases, &version("0.4.0"), "windows").is_none());
        assert!(release_in(&releases, &version("0.4.1"), "macos").is_none());
    }

    #[test]
    fn a_field_with_a_wrong_name_is_an_error() {
        assert!(parse("[[release]]\nversion = \"0.5.0\"\ntitle = \"x\"\ntext = \"y\"\n").is_err());
    }

    #[test]
    fn the_window_is_due_only_for_a_newer_version() {
        let running = version("0.5.0");
        // An install from before this window existed.
        assert!(is_due(None, &running));
        assert!(is_due(Some(&version("0.4.3")), &running));
        assert!(!is_due(Some(&version("0.5.0")), &running));
        // The user went back to an older version.
        assert!(!is_due(Some(&version("0.6.0")), &running));
    }

    #[test]
    fn the_seen_version_stays_beside_the_other_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = Paths { config_dir: root.clone(), config_source: PathSource::XdgVariable, data_dir: root.clone(), data_source: PathSource::XdgVariable, cache_dir: root };
        assert_eq!(seen(&paths), None);
        std::fs::write(paths.state_file(), r#"{"pill":{"1":{"x":0.5}}}"#).unwrap();
        mark_seen(&paths);
        assert_eq!(seen(&paths), Some(version(sayso_update::VERSION)));
        let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(paths.state_file()).unwrap()).unwrap();
        assert_eq!(state["pill"]["1"]["x"], 0.5);
    }
}
