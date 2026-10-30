//! The quit question (`docs/extensions.md`, "Lifecycle"). When an app has
//! a live session or a job, the Quit of the Popover and of the tray menu names
//! the app, by its record in `apps.toml`, and asks first: "Notetaker is recording. Quit
//! anyway?". Without such work, Sayso quits at once.
//!
//! A quit that the system starts (log out, shut down, Quit in the Dock on
//! macOS) does not ask: the system does not wait for an answer. The host still
//! sends `host_closing` (`app.rs`).

use crate::model::AppModel;
use crate::question::{self, Texts};
use gpui_kit::*;
use sayso_host::Hold;
use sayso_i18n::t;
use std::cell::RefCell;

thread_local! {
    /// The open quit question. A second Quit brings it to the front.
    static OPEN: RefCell<Option<AnyWindowHandle>> = const { RefCell::new(None) };
}

/// Quit, or ask first when an app works.
pub fn request(model: &Entity<AppModel>, cx: &mut App) {
    let holds = model.read(cx).app_holds();
    if holds.is_empty() {
        cx.quit();
        return;
    }
    if let Some(handle) = OPEN.with(|open| *open.borrow())
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let opened = question::open(model, "quit-ask", size(px(520.), px(340.)), move || texts(&holds), |yes, cx| {
        OPEN.with(|open| open.borrow_mut().take());
        if yes {
            cx.quit();
        }
    }, cx);
    match opened {
        Ok(handle) => OPEN.with(|open| *open.borrow_mut() = Some(handle)),
        // Without the window, the user cannot answer. Quit as asked.
        Err(e) => {
            log::error!("could not open the quit question: {e:#}");
            cx.quit();
        }
    }
}

fn texts(holds: &[Hold]) -> Texts {
    let title = crate::host::quit_question(holds).unwrap_or_default();
    // The other apps that work now, when there are more.
    let others: Vec<&str> = holds.iter().skip(1).map(|hold| hold.name.as_str()).filter(|name| *name != holds[0].name).collect();
    Texts {
        window_title: t!("apps.quit_ask.window_title"),
        title,
        body: t!("apps.quit_ask.body"),
        details: None,
        quote: (!others.is_empty()).then(|| (t!("apps.quit_ask.others_label"), others.join(", "))),
        cancel: t!("apps.quit_ask.cancel"),
        confirm: t!("apps.quit_ask.quit"),
        danger: true,
    }
}
