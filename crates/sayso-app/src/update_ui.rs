//! The words and the actions for each update status. Settings › General, the
//! Hub sidebar, and the Popover use them, so the three places agree.

use crate::hub::{Route, SettingsPage};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;
use sayso_update::{Manual, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Accent,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Check,
    Install,
    Cancel,
    Restart,
    DownloadPage,
    ShowApp,
    /// Open Settings › General, where the status has its full text.
    OpenSettings,
}

/// How one status reads.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// The row in Settings › General.
    pub title: String,
    pub detail: String,
    pub tone: Tone,
    /// The part of the download that arrived, 0 to 1.
    pub progress: Option<f32>,
    pub notes_url: Option<String>,
    /// The button in Settings.
    pub action: Option<(&'static str, Action)>,
    /// The line in the sidebar and the Popover, with its action there. None
    /// when the user has nothing to do or to watch.
    pub short: Option<(String, &'static str, Action)>,
}

pub fn view(status: &Status) -> View {
    let version = sayso_update::VERSION;
    let plain = |title: String, detail: String| View { title, detail, tone: Tone::Plain, progress: None, notes_url: None, action: None, short: None };
    match status {
        Status::Idle { last_check } => View {
            action: Some(("Check for updates", Action::Check)),
            ..plain(
                "Sayso is up to date".into(),
                match last_check {
                    Some(at) => format!("Version {version}. Last check {}.", when(*at)),
                    None => format!("Version {version}. Sayso did not check yet."),
                },
            )
        },
        Status::Checking => plain("Checking for updates…".into(), format!("Version {version}")),
        Status::Available { version: new, notes_url, manual } => {
            let title = format!("Sayso {new} is available");
            let (detail, action, short) = match manual {
                None => (format!("You have {version}."), ("Update now", Action::Install), ("Update now", Action::Install)),
                Some(Manual::WrongFolder) => (
                    "Sayso cannot update itself from this folder. Move Sayso to the Applications folder, then open it again.".into(),
                    ("Show in Finder", Action::ShowApp),
                    ("How to update", Action::OpenSettings),
                ),
                Some(Manual::NoPermission) => (
                    format!("You have {version}. Sayso cannot write to its folder, so it cannot update itself. Download the new version and install it."),
                    ("Download", Action::DownloadPage),
                    ("How to update", Action::OpenSettings),
                ),
                Some(Manual::NoExchange) => (
                    format!("You have {version}. The disk of Sayso cannot replace an app in one step, so Sayso cannot update itself. Download the new version and install it."),
                    ("Download", Action::DownloadPage),
                    ("How to update", Action::OpenSettings),
                ),
                Some(Manual::Unsupported) => (format!("You have {version}. Download the new version and install it."), ("Download", Action::DownloadPage), ("Download", Action::DownloadPage)),
            };
            View { tone: Tone::Accent, notes_url: Some(notes_url.clone()), action: Some(action), short: Some((title.clone(), short.0, short.1)), ..plain(title, detail) }
        }
        Status::Downloading { version: new, received, total } => View {
            progress: Some(if *total == 0 { 0. } else { *received as f32 / *total as f32 }),
            action: Some(("Cancel", Action::Cancel)),
            short: Some((format!("Downloading {new}"), "", Action::OpenSettings)),
            ..plain(format!("Downloading Sayso {new}"), format!("{} of {}", megabytes(*received), megabytes(*total)))
        },
        Status::Preparing { version: new } => View {
            short: Some((format!("Preparing {new}…"), "", Action::OpenSettings)),
            ..plain(format!("Preparing Sayso {new}…"), "Sayso unpacks the new version and checks its signature.".into())
        },
        Status::Ready { version: new } => View {
            tone: Tone::Accent,
            action: Some(("Restart to update", Action::Restart)),
            short: Some((format!("Sayso {new} is ready"), "Restart to update", Action::Restart)),
            ..plain(format!("Sayso {new} is ready to install"), "Sayso closes and opens again. This takes a few seconds.".into())
        },
        Status::Unreadable => View {
            tone: Tone::Accent,
            action: Some(("Open the download page", Action::DownloadPage)),
            short: Some(("A newer Sayso is available".into(), "Download", Action::DownloadPage)),
            ..plain(
                "A newer Sayso is available".into(),
                format!("Version {version} cannot read the update information. Download the new version and install it."),
            )
        },
        Status::Failed { message } => View {
            tone: Tone::Danger,
            action: Some(("Try again", Action::Check)),
            short: Some(("Sayso could not update".into(), "See why", Action::OpenSettings)),
            ..plain("Sayso could not update".into(), message.clone())
        },
    }
}

/// Do what a button for `action` says.
pub fn perform(action: Action, model: &Entity<AppModel>, cx: &mut App) {
    match action {
        Action::Check => model.read(cx).check_for_updates(),
        Action::Install => model.read(cx).install_update(),
        Action::Cancel => model.read(cx).cancel_update(),
        Action::Restart => model.update(cx, |m, cx| m.restart_to_update(cx)),
        Action::DownloadPage => crate::shell::open(sayso_update::DOWNLOAD_PAGE),
        Action::ShowApp => {
            if let Ok(exe) = std::env::current_exe() {
                // The bundle, when Sayso runs from one.
                let app = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).unwrap_or(&exe);
                crate::shell::reveal(app);
            }
        }
        Action::OpenSettings => model.update(cx, |m, cx| m.open_hub(Route::Settings(SettingsPage::General), cx)),
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1_000_000.).round() as u64)
}

/// "today at 9:41" or "on Oct 2 at 9:41", in local time.
fn when(unix_seconds: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_opt(unix_seconds as i64, 0).single() else {
        return "at an unknown time".into();
    };
    if at.date_naive() == Local::now().date_naive() {
        format!("today at {}", at.format("%-H:%M"))
    } else {
        format!("on {}", at.format("%b %-d at %-H:%M"))
    }
}

/// The page of a release on GitHub.
pub fn release_notes(version: &sayso_update::manifest::Version) -> String {
    format!("{}tag/v{version}", sayso_update::manifest::URL_PREFIX)
}

/// The slip in the Hub sidebar, below the engine status.
pub fn sidebar_slip(model: &Entity<AppModel>, cx: &App) -> Option<AnyElement> {
    let c = cx.paper().colors;
    let m = model.read(cx);
    let v = view(&m.update);
    let (headline, label, action) = v.short?;
    let can_restart = m.can_restart_to_update();
    let model = model.clone();
    let slip = div().id("update-slip").flex().w_full().rounded(px(10.)).py(px(9.)).pl(px(10.)).pr(px(8.));
    // Ready: the slip is the button.
    if action == Action::Restart {
        return Some(
            slip.items_center()
                .gap(px(10.))
                .pl(px(12.))
                .ink_button(&c)
                .when(can_restart, |d| d.cursor_pointer().on_click(move |_, _, cx| perform(Action::Restart, &model, cx)))
                .when(!can_restart, |d| d.opacity(0.6))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(text::ui(label, 13., FontWeight::SEMIBOLD, c.on_ink))
                        .child(text::ui(headline, 12., FontWeight::NORMAL, c.on_ink.opacity(0.7))),
                )
                .into_any_element(),
        );
    }
    let slip = slip.raised_small(&c).cursor_pointer().on_click(move |_, _, cx| perform(Action::OpenSettings, &model, cx));
    if let Some(progress) = v.progress {
        return Some(
            slip.flex_col()
                .gap(px(7.))
                .pr(px(10.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(text::ui(headline, 13., FontWeight::SEMIBOLD, c.ink).flex_1().truncate())
                        .child(text::mono(format!("{}%", (progress * 100.) as u32), 12., c.graphite).font_weight(FontWeight::NORMAL)),
                )
                .child(Progress::new(progress).height(4.))
                .into_any_element(),
        );
    }
    let (title, detail) = match &m.update {
        Status::Available { version, .. } => ("Update available".to_string(), format!("Sayso {version}")),
        Status::Unreadable => ("Update available".to_string(), "A newer Sayso".to_string()),
        Status::Failed { .. } => (headline, label.to_string()),
        _ => (headline, String::new()),
    };
    let dot = match v.tone {
        Tone::Danger => Some(c.danger),
        Tone::Accent => Some(c.accent),
        Tone::Plain => None,
    };
    Some(
        slip.items_center()
            .gap(px(10.))
            .when_some(dot, |d, color| d.child(status_dot(color, 8.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(text::ui(title, 13., FontWeight::SEMIBOLD, c.ink).truncate())
                    .when(!detail.is_empty(), |d| d.child(text::ui(detail, 12., FontWeight::NORMAL, c.graphite))),
            )
            .child(icon(Icon::ChevronRight, 12., c.pencil))
            .into_any_element(),
    )
}

/// The line in the Popover, above the footer. `hide` closes the Popover.
pub fn popover_line(model: &Entity<AppModel>, hide: fn(&mut App), cx: &App) -> Option<AnyElement> {
    let c = cx.paper().colors;
    let m = model.read(cx);
    let line = || div().flex().flex_1().items_center().gap(px(10.)).h(px(38.)).pl(px(12.)).pr(px(4.)).rounded(px(9.));
    let wrap = |inner: Div| div().flex().px(px(4.)).pt(px(8.)).pb(px(6.)).child(inner).into_any_element();
    let button = |id: &'static str, label: &'static str, color: Hsla| {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .h(px(30.))
            .px(px(10.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(move |s| s.bg(color.opacity(0.08)))
            .child(text::ui(label, 13., FontWeight::SEMIBOLD, color))
    };
    let Some((headline, label, action)) = view(&m.update).short else {
        // Nothing to do. Say one time that an update happened.
        let version = m.update_notice.clone()?;
        let notes = release_notes(&version);
        let model = model.clone();
        return Some(wrap(
            line()
                .raised_small(&c)
                .pr(px(6.))
                .child(icon(Icon::Check, 14., c.graphite))
                .child(text::ui(format!("Sayso is now {version}"), 13., FontWeight::MEDIUM, c.ink).flex_1().truncate())
                .child(button("update-notes", "What is new", c.accent).on_click({
                    let model = model.clone();
                    move |_, _, cx| {
                        // The notes in the app, when this version has some.
                        if crate::whats_new::current().is_some() {
                            hide(cx);
                            crate::whats_new::open(&model, cx);
                        } else {
                            crate::shell::open(&notes);
                        }
                    }
                }))
                .child(
                    div()
                        .id("update-notice-dismiss")
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(26.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.deboss.opacity(0.6)))
                        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.dismiss_update_notice(cx)))
                        .child(icon(Icon::Close, 10., c.pencil)),
                ),
        ));
    };
    let v = view(&m.update);
    if let Some(progress) = v.progress {
        return Some(wrap(
            line()
                .debossed(&c)
                .pr(px(12.))
                .child(text::ui(headline, 13., FontWeight::MEDIUM, c.ink).flex_1().truncate())
                .child(div().flex_none().w(px(96.)).child(Progress::new(progress).height(4.)))
                .child(text::mono(format!("{}%", (progress * 100.) as u32), 12., c.graphite).font_weight(FontWeight::NORMAL)),
        ));
    }
    if label.is_empty() {
        return Some(wrap(line().debossed(&c).pr(px(12.)).child(text::ui(headline, 13., FontWeight::MEDIUM, c.ink).flex_1().truncate())));
    }
    let (wash, ink, strong) = match v.tone {
        Tone::Danger => (c.danger_wash, c.danger, c.danger),
        _ => (c.accent_wash, c.ink, c.accent),
    };
    let off = action == Action::Restart && !m.can_restart_to_update();
    let model = model.clone();
    Some(wrap(
        line()
            .bg(wash)
            .border_1()
            .border_color(strong.opacity(0.14))
            .child(div().flex_none().size(px(7.)).rounded_full().bg(strong))
            .child(text::ui(headline, 13., FontWeight::MEDIUM, ink).flex_1().truncate())
            .child(button("update-action", label, strong).when(off, |d| d.opacity(0.5)).on_click(move |_, _, cx| {
                if off {
                    return;
                }
                // The download stays in view. Every other action leaves the Popover.
                if action != Action::Install {
                    hide(cx);
                }
                perform(action, &model, cx);
            })),
    ))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in the `test` attribute of GPUI.
    use super::{Action, view};
    use sayso_update::{Manual, Status};

    fn available(manual: Option<Manual>) -> Status {
        Status::Available { version: "9.9.9".parse().unwrap(), notes_url: "https://example.com/notes".into(), manual }
    }

    #[test]
    fn the_user_sees_a_line_only_when_there_is_something_to_do_or_watch() {
        assert!(view(&Status::Idle { last_check: None }).short.is_none());
        assert!(view(&Status::Checking).short.is_none());
        for status in [
            available(None),
            Status::Downloading { version: "9.9.9".parse().unwrap(), received: 1, total: 2 },
            Status::Preparing { version: "9.9.9".parse().unwrap() },
            Status::Ready { version: "9.9.9".parse().unwrap() },
            Status::Unreadable,
            Status::Failed { message: "x".into() },
        ] {
            assert!(view(&status).short.is_some(), "{status:?}");
        }
    }

    #[test]
    fn an_install_that_can_update_itself_offers_update_now() {
        let v = view(&available(None));
        assert_eq!(v.title, "Sayso 9.9.9 is available");
        assert_eq!(v.detail, format!("You have {}.", sayso_update::VERSION));
        assert_eq!(v.action, Some(("Update now", Action::Install)));
        assert_eq!(v.notes_url.as_deref(), Some("https://example.com/notes"));
    }

    #[test]
    fn an_install_that_cannot_update_itself_never_offers_update_now() {
        for manual in [Manual::Unsupported, Manual::WrongFolder, Manual::NoPermission, Manual::NoExchange] {
            let v = view(&available(Some(manual)));
            assert!(!matches!(v.action, Some((_, Action::Install))), "{manual:?}");
            assert!(!matches!(v.short, Some((_, _, Action::Install))), "{manual:?}");
        }
    }

    #[test]
    fn the_download_shows_its_part_and_its_size() {
        let v = view(&Status::Downloading { version: "9.9.9".parse().unwrap(), received: 18_000_000, total: 41_000_000 });
        assert_eq!(v.detail, "18 MB of 41 MB");
        assert!((v.progress.unwrap() - 18. / 41.).abs() < 0.001);
        assert_eq!(v.action, Some(("Cancel", Action::Cancel)));
    }
}
