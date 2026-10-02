//! Settings pages. The sidebar shows the sections (see `hub/sidebar.rs`).

mod advanced;
mod appearance;
mod audio;
mod dictation;
pub mod ext;
mod general;
mod history_privacy;
pub mod kit;
mod overlay;
mod permissions;
pub mod recorder;

use super::{Route, SettingsPage};
use crate::model::AppModel;
use gpui_kit::*;

pub struct SettingsView {
    model: Entity<AppModel>,
    general: Entity<general::GeneralSettings>,
    dictation: Entity<dictation::DictationSettings>,
    overlay: Entity<overlay::OverlaySettings>,
    audio: Entity<audio::AudioSettings>,
    appearance: Entity<appearance::AppearanceSettings>,
    history: Entity<history_privacy::HistorySettings>,
    permissions: Entity<permissions::PermissionSettings>,
    advanced: Entity<advanced::AdvancedSettings>,
    _observe: Subscription,
}

impl SettingsView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            general: cx.new(|cx| general::GeneralSettings::new(model.clone(), window, cx)),
            dictation: cx.new(|cx| dictation::DictationSettings::new(model.clone(), window, cx)),
            overlay: cx.new(|cx| overlay::OverlaySettings::new(model.clone(), window, cx)),
            audio: cx.new(|cx| audio::AudioSettings::new(model.clone(), window, cx)),
            appearance: cx.new(|cx| appearance::AppearanceSettings::new(model.clone(), window, cx)),
            history: cx.new(|cx| history_privacy::HistorySettings::new(model.clone(), window, cx)),
            permissions: cx.new(|cx| permissions::PermissionSettings::new(model.clone(), window, cx)),
            advanced: cx.new(|cx| advanced::AdvancedSettings::new(model.clone(), window, cx)),
            model,
            _observe: observe,
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Route::Settings(page) = self.model.read(cx).route else { return div().into_any_element() };
        match page {
            SettingsPage::General => self.general.clone().into_any_element(),
            SettingsPage::Dictation => self.dictation.clone().into_any_element(),
            SettingsPage::Overlay => self.overlay.clone().into_any_element(),
            SettingsPage::Audio => self.audio.clone().into_any_element(),
            SettingsPage::Appearance => self.appearance.clone().into_any_element(),
            SettingsPage::HistoryPrivacy => self.history.clone().into_any_element(),
            SettingsPage::Permissions => self.permissions.clone().into_any_element(),
            SettingsPage::Advanced => self.advanced.clone().into_any_element(),
        }
    }
}
