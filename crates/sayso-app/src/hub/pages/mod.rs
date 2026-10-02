//! Hub pages. Each page is its own view with its own local state.

pub mod dictionary;
pub mod ext;
pub mod history;
pub mod home;
pub mod kit;
pub mod models;
pub mod player;
pub mod styles;

use super::Route;
use super::settings::SettingsView;
use crate::model::AppModel;
use gpui_kit::*;

pub struct Pages {
    home: Entity<home::HomePage>,
    history: Entity<history::HistoryPage>,
    dictionary: Entity<dictionary::DictionaryPage>,
    styles: Entity<styles::StylesPage>,
    models: Entity<models::ModelsPage>,
    settings: Entity<SettingsView>,
}

impl Pages {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut App) -> Self {
        Self {
            home: cx.new(|cx| home::HomePage::new(model.clone(), window, cx)),
            history: cx.new(|cx| history::HistoryPage::new(model.clone(), window, cx)),
            dictionary: cx.new(|cx| dictionary::DictionaryPage::new(model.clone(), window, cx)),
            styles: cx.new(|cx| styles::StylesPage::new(model.clone(), window, cx)),
            models: cx.new(|cx| models::ModelsPage::new(model.clone(), window, cx)),
            settings: cx.new(|cx| SettingsView::new(model.clone(), window, cx)),
        }
    }

    pub fn view(&self, route: Route, _window: &mut Window, _cx: &mut App) -> AnyElement {
        match route {
            Route::Home => self.home.clone().into_any_element(),
            Route::History => self.history.clone().into_any_element(),
            Route::Dictionary => self.dictionary.clone().into_any_element(),
            Route::Styles => self.styles.clone().into_any_element(),
            Route::Models => self.models.clone().into_any_element(),
            Route::Settings(_) => self.settings.clone().into_any_element(),
        }
    }
}
