//! Controls built from the paper material. Each follows the Paper design file.

mod basics;
mod controls;
mod waveform;

pub use basics::*;
pub use controls::*;
pub use waveform::*;

use gpui_kit::{App, ClickEvent, Window};
use std::rc::Rc;

/// A click callback shared by the components.
pub type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

pub fn on_click(f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> OnClick {
    Rc::new(f)
}
