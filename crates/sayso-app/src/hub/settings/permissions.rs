//! Settings › Permissions: live state for each permission.

use super::kit::{self, group};
use crate::model::AppModel;
use gpui_kit::*;
use sayso_platform::{Permission, PermissionState};
use sayso_ui::components::*;

pub struct PermissionSettings {
    model: Entity<AppModel>,
}

impl PermissionSettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { model }
    }
}

impl Render for PermissionSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.model.read(cx);
        let mut rows = vec![
            (Permission::Microphone, "Microphone", "Lets Sayso hear you while you dictate. Required."),
            (Permission::Accessibility, crate::shell::permission_name(Permission::Accessibility), "Lets Sayso paste text into the app you are using. Required."),
            (
                Permission::InputMonitoring,
                crate::shell::permission_name(Permission::InputMonitoring),
                "Needed for push to talk, for Esc to cancel, and to record keys in other apps.",
            ),
        ];
        if !crate::shell::HAS_INPUT_PERMISSIONS {
            rows.retain(|(p, _, _)| *p == Permission::Microphone);
        }
        let states: Vec<PermissionState> = rows.iter().map(|(p, _, _)| m.permission(*p)).collect();
        let mut g = group("Permissions", cx);
        if !crate::dev::running_from_bundle() {
            g = g.child(kit::banner(BannerKind::Warning, crate::dev::UNBUNDLED_NOTE, cx));
        }
        for (i, ((perm, title, desc), state)) in rows.into_iter().zip(states).enumerate() {
            let perm2 = perm;
            let mut controls = div().flex().flex_none().items_center().gap(px(12.)).child(kit::permission_badge(state, cx).w(px(84.)));
            if state == PermissionState::NotDetermined && perm != Permission::Accessibility {
                controls = controls.child(Button::new(("perm-request", i), "Request").small().on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.request_permission(perm2));
                })));
            }
            controls = controls.child(Button::new(("perm-open", i), crate::shell::OPEN_PERMISSION_SETTINGS).small().on_click(cx.listener(
                move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.open_permission_settings(perm));
                },
            )));
            g = g.child(kit::row(title, desc, controls, cx));
        }
        g = g.child(kit::banner(
            BannerKind::Info,
            crate::shell::os_text!(
                "This page updates by itself when you change a permission in System Settings. If a permission stays off after you turn it on, quit Sayso and open it again.",
                "This page updates by itself when you change a permission in Settings. If a permission stays off after you turn it on, quit Sayso and open it again.",
                "This page updates by itself when a permission changes. After you join the input group, log out and log in again.",
            ), cx));
        kit::page("permissions-page", "Permissions", crate::shell::os_text!("What Sayso can do on this Mac, and why.", "What Sayso can do on this PC, and why.", "What Sayso can do on this computer, and why."), kit::body().child(g), cx)
    }
}
