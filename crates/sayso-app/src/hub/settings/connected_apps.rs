//! Settings › Connected apps (`docs/sdk.md`, "In Sayso"): the answers of the
//! user in `apps.toml`. A switch takes back a grant at the next request of the
//! app. **Remove** and **Forget** delete the record, and the app is asked again
//! at its next connection.

use super::kit;
use crate::approval_ask::{capability_text, ordered};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_host::{AppRecord, Identity, visible_path, visible_text};
use sayso_i18n::t;
use sayso_protocol::Capability;
use sayso_ui::ActivePaper;
use sayso_ui::a11y::Pressable;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;
use std::io;

pub struct ConnectedAppsSettings {
    model: Entity<AppModel>,
    /// The key of the card that shows its switches. None: the first one.
    expanded: Option<String>,
    /// The last change that could not be saved.
    save_failed: bool,
}

/// The team (or the path) and the identifier, under the name of an app.
fn id_line(record: &AppRecord, details: &str) -> String {
    match (&record.signer, &record.team_id, &record.identifier, &record.path) {
        (Some(signer), _, Some(identifier), _) => visible_text(&format!("{signer} · {identifier}")),
        (None, Some(team), Some(identifier), _) => visible_text(&format!("{team} · {identifier}")),
        (_, _, _, Some(path)) if details.is_empty() => short_path(path),
        (_, _, _, Some(path)) => format!("{} · {details}", short_path(path)),
        _ => String::new(),
    }
}

/// A path in the home folder starts with `~`.
fn short_path(path: &std::path::Path) -> String {
    match dirs_home().and_then(|home| path.strip_prefix(home).ok().map(visible_path)) {
        Some(rest) => format!("~/{rest}"),
        None => visible_path(path),
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(Into::into)
}

/// What a card can switch: what the app asked for, in the order of the question.
fn switchable(record: &AppRecord) -> Vec<(Capability, bool)> {
    let granted = record.capabilities();
    let all: Vec<Capability> = granted.iter().cloned().chain(record.denied_capabilities()).collect();
    ordered(&all).into_iter().map(|capability| {
        let on = granted.contains(&capability);
        (capability, on)
    }).collect()
}

fn said_no(record: &AppRecord) -> String {
    let date = record.first_seen.as_ref().and_then(|seen| chrono::DateTime::parse_from_rfc3339(&seen.to_string()).ok());
    match date {
        Some(date) => t!("apps.settings.said_no", date = sayso_i18n::dates::day_month_short(&date.with_timezone(&chrono::Local))),
        None => t!("apps.settings.said_no_undated"),
    }
}

impl ConnectedAppsSettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { model, expanded: None, save_failed: false }
    }

    /// Do `change`, and show what happened at once.
    fn change(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&sayso_host::AppsFile) -> io::Result<()>) {
        let file = self.model.read(cx).apps_file.clone();
        let saved = change(&file);
        if let Err(e) = &saved {
            log::warn!("settings: cannot change {}: {e}", file.path().display());
        }
        self.save_failed = saved.is_err();
        self.model.update(cx, |m, cx| m.apps_changed(cx));
        cx.notify();
    }

    fn pill(label: String, background: Hsla, color: Hsla, dot: bool) -> Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .py(px(3.))
            .px(px(9.))
            .rounded_full()
            .bg(background)
            .when(dot, |d| d.child(div().size(px(7.)).rounded_full().bg(color)))
            .child(text::ui(label, 12., FontWeight::SEMIBOLD, color).line_height(px(14.)))
    }

    fn card_base(cx: &App) -> Div {
        let c = cx.paper().colors;
        div().flex().w_full().min_w_0().rounded(px(14.)).bg(c.sheet_raised).raised(&c)
    }

    /// The square at the left of a card: filled for a signed app, debossed for the others.
    fn mark(signed: bool, cx: &App) -> Div {
        let c = cx.paper().colors;
        let square = div().flex().flex_none().items_center().justify_center().size(px(40.)).rounded(px(10.));
        if signed { square.bg(c.ink_fill).child(icon(Icon::Activity, 20., c.on_ink)) } else { square.debossed(&c).child(icon(Icon::Terminal, 20., c.graphite)) }
    }

    fn head(record: &AppRecord, recording: bool, details: &str, cx: &App) -> Div {
        let c = cx.paper().colors;
        let signed = record.team_id.is_some();
        let pill = if recording {
            Some(Self::pill(t!("apps.settings.recording"), c.accent_wash, c.accent, true))
        } else if !signed {
            Some(Self::pill(t!("apps.approval.not_signed"), c.deboss, c.graphite, false))
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(3.))
            .child(div().flex().items_center().gap(px(10.)).child(text::ui(record.shown_name(), 17., FontWeight::SEMIBOLD, c.ink).line_height(px(20.))).children(pill))
            .child(text::mono(id_line(record, details), 12., c.graphite).line_height(px(16.)))
    }

    fn expanded_card(&self, id: &str, record: &AppRecord, recording: bool, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let remove_id = id.to_string();
        let remove = Pressable::new(("app-remove", index))
            .label(format!("{}, {}", t!("apps.settings.remove"), record.shown_name()))
            .flex_none()
            .py(px(8.))
            .px(px(4.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = remove_id.clone();
                this.change(cx, |file| file.remove(&id));
            }))
            .child(text::ui(t!("apps.settings.remove"), 13., FontWeight::SEMIBOLD, c.danger).line_height(px(16.)));
        let mut card = Self::card_base(cx)
            .flex_col()
            .gap(px(4.))
            .pt(px(20.))
            .px(px(24.))
            .pb(px(8.))
            .child(div().flex().items_center().gap(px(14.)).pb(px(12.)).child(Self::mark(record.team_id.is_some(), cx)).child(Self::head(record, recording, "", cx)).child(remove));
        let rows = switchable(record);
        if rows.is_empty() {
            card = card.child(div().py(px(12.)).border_t_1().border_color(c.rule).child(text::ui(t!("apps.settings.no_capabilities"), 13., FontWeight::NORMAL, c.graphite).line_height(px(18.))));
        }
        for (row, (capability, on)) in rows.into_iter().enumerate() {
            let label = capability_text(&capability);
            let color = if on { c.ink } else { c.graphite };
            let id = id.to_string();
            let page = cx.entity();
            let toggle = Switch::new(SharedString::from(format!("app-{index}-grant-{row}")), on).label(label.clone()).on_toggle(move |turned_on, _, cx| {
                let (id, capability) = (id.clone(), capability.clone());
                page.update(cx, |this, cx| this.change(cx, |file| file.set_grant(&id, &capability, turned_on)));
            });
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(24.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(c.rule)
                    .child(div().flex_1().min_w_0().child(text::ui(label, 15., FontWeight::MEDIUM, color).line_height(px(18.))))
                    .child(toggle),
            );
        }
        card.into_any_element()
    }

    fn collapsed_card(&self, id: &str, record: &AppRecord, recording: bool, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let id = id.to_string();
        let details = switchable(record).into_iter().filter(|(_, on)| *on).map(|(capability, _)| capability_text(&capability)).collect::<Vec<_>>().join(", ");
        let show = t!("apps.settings.show_app", app = record.shown_name());
        Pressable::new(("app-show", index))
            .label(show)
            .w_full()
            .cursor_pointer()
            .rounded(px(14.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.expanded = Some(id.clone());
                cx.notify();
            }))
            .child(
                Self::card_base(cx)
                    .items_center()
                    .gap(px(14.))
                    .py(px(16.))
                    .px(px(24.))
                    .child(Self::mark(record.team_id.is_some(), cx))
                    .child(Self::head(record, recording, &details, cx))
                    .child(icon(Icon::ChevronRight, 16., c.pencil)),
            )
            .into_any_element()
    }

    fn refused_row(&self, id: &str, record: &AppRecord, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let identity = match record.identity() {
            Some(Identity::Signed { team_id, identifier }) => format!("{team_id} · {identifier}"),
            Some(Identity::Path(path)) => short_path(&path),
            None => String::new(),
        };
        let (allow_id, forget_id) = (id.to_string(), id.to_string());
        let allow = Button::new(("app-allow", index), t!("apps.settings.allow")).a11y_label(format!("{}, {}", t!("apps.settings.allow"), record.name)).small().on_click(cx.listener(move |this, _, _, cx| {
            let id = allow_id.clone();
            this.expanded = Some(id.clone());
            this.change(cx, |file| file.allow(&id));
        }));
        let forget = Pressable::new(("app-forget", index))
            .label(format!("{}, {}", t!("apps.settings.forget"), record.shown_name()))
            .flex_none()
            .py(px(8.))
            .px(px(4.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = forget_id.clone();
                this.change(cx, |file| file.remove(&id));
            }))
            .child(text::ui(t!("apps.settings.forget"), 13., FontWeight::SEMIBOLD, c.graphite).line_height(px(16.)));
        div()
            .flex()
            .items_center()
            .w_full()
            .gap(px(24.))
            .py(px(14.))
            .border_b_1()
            .border_color(c.rule)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(text::ui(record.shown_name(), 15., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
                    .child(text::mono(format!("{identity} · {}", said_no(record)), 12., c.graphite).line_height(px(16.))),
            )
            .child(div().flex().flex_none().items_center().gap(px(12.)).child(allow).child(forget))
            .into_any_element()
    }
}

impl Render for ConnectedAppsSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let apps = m.apps.clone();
        let broken = m.apps_broken;
        let path = m.apps_file.path().display().to_string();
        let recording = m.recording_app().map(|recording| recording.app_id.clone());
        let approved: Vec<(String, &AppRecord)> = apps.apps.iter().filter(|record| record.allowed).filter_map(|record| record.id().map(|id| (id, record))).collect();
        let refused: Vec<(String, &AppRecord)> = apps.apps.iter().filter(|record| !record.allowed).filter_map(|record| record.id().map(|id| (id, record))).collect();
        // One card shows its switches: the one that the user chose, else the one that records, else the first.
        let open = self
            .expanded
            .clone()
            .filter(|id| approved.iter().any(|(known, _)| known == id))
            .or_else(|| recording.clone().filter(|id| approved.iter().any(|(known, _)| known == id)))
            .or_else(|| approved.first().map(|(id, _)| id.clone()));

        let mut body = div().flex().flex_col().flex_none().w_full().min_w_0().gap(px(12.));
        if broken {
            body = body.child(kit::banner(BannerKind::Warning, t!("apps.settings.broken", path = path), cx));
        } else if self.save_failed {
            body = body.child(kit::banner(BannerKind::Warning, t!("apps.settings.save_failed", path = path), cx));
        }
        if approved.is_empty() && refused.is_empty() && !broken {
            body = body.child(text::ui(t!("apps.settings.empty"), 15., FontWeight::NORMAL, c.graphite).line_height(px(22.)));
        }
        for (index, (id, record)) in approved.iter().enumerate() {
            let is_recording = recording.as_ref() == Some(id);
            let card = if open.as_ref() == Some(id) { self.expanded_card(id, record, is_recording, index, cx) } else { self.collapsed_card(id, record, is_recording, index, cx) };
            body = body.child(card);
        }
        if !refused.is_empty() {
            let mut section = div().flex().flex_col().w_full().min_w_0().when(!approved.is_empty(), |d| d.pt(px(28.))).child(crate::widgets::section(&t!("apps.settings.not_allowed"), cx));
            for (index, (id, record)) in refused.iter().enumerate() {
                section = section.child(self.refused_row(id, record, index, cx));
            }
            body = body.child(section);
        }
        kit::page("connected-apps-page", &t!("apps.settings.title"), &t!("apps.settings.subtitle"), body, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppRecord, Capability, id_line, said_no, switchable};

    fn record(text: &str) -> AppRecord {
        sayso_host::Apps::parse(text).unwrap().apps.remove(0)
    }

    #[test]
    fn a_card_switches_what_the_app_asked_for_in_order() {
        let record = record("[[apps]]\nname = \"N\"\npath = \"/x\"\nallowed = true\ngranted = [\"transcribe.file\", \"audio.mic\"]\ndenied = [\"audio.system\"]\n");
        assert_eq!(switchable(&record), vec![(Capability::AudioMic, true), (Capability::AudioSystem, false), (Capability::TranscribeFile, true)]);
    }

    #[test]
    fn the_id_line_shows_each_character_that_can_mislead() {
        let record = AppRecord { name: "x".into(), team_id: None, identifier: None, signer: None, path: Some("/opt/ev\u{202E}il\nname".into()), allowed: true, granted: vec![], denied: vec![], first_seen: None };
        assert_eq!(id_line(&record, ""), "/opt/ev\u{FFFD}il\u{FFFD}name");
    }

    #[test]
    fn the_id_line_names_the_signer_or_the_path() {
        let signed = record("[[apps]]\nname = \"N\"\nteam_id = \"T\"\nidentifier = \"app.x\"\nsigner = \"Example LLC\"\nallowed = true\n");
        assert_eq!(id_line(&signed, ""), "Example LLC · app.x");
        let path = record("[[apps]]\nname = \"N\"\npath = \"/opt/x\"\nallowed = true\n");
        assert_eq!(id_line(&path, ""), "/opt/x");
        assert_eq!(id_line(&path, "Transcribe files"), "/opt/x · Transcribe files");
    }

    #[test]
    fn a_refused_row_says_when() {
        sayso_i18n::set_locale("en");
        let dated = record("[[apps]]\nname = \"N\"\npath = \"/x\"\nfirst_seen = 2026-10-28T12:00:00Z\n");
        assert!(said_no(&dated).starts_with("You said no on "), "{}", said_no(&dated));
        assert_eq!(said_no(&record("[[apps]]\nname = \"N\"\npath = \"/x\"\n")), "You said no");
    }
}
