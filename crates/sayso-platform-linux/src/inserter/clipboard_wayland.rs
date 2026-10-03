//! The clipboard through the Wayland data-control protocol, for compositors
//! that have it: KDE Plasma, sway, Hyprland, and other wlroots compositors.
//!
//! A data-control client can set the clipboard and the primary selection
//! without a focused window. Sayso offers its text as a data source and
//! writes the text each time the compositor sends a `send` event: each such
//! event for a text type is a read for the receipt. A `cancelled` event
//! means that another app took the selection. A thread keeps the source
//! alive until then.
//!
//! Two versions of the protocol exist: `ext_data_control_v1` (the standard
//! one) and `zwlr_data_control_v1` (the older wlroots one). The same code
//! serves both; `data_control.rs` holds it.

use crate::session::Session;

/// The standard protocol, `ext_data_control_v1`.
mod ext {
    use wayland_protocols::ext::data_control::v1::client::{
        ext_data_control_device_v1 as device, ext_data_control_manager_v1 as manager, ext_data_control_offer_v1 as offer,
        ext_data_control_source_v1 as source,
    };
    type Manager = manager::ExtDataControlManagerV1;
    type Device = device::ExtDataControlDeviceV1;
    type Source = source::ExtDataControlSourceV1;
    type Offer = offer::ExtDataControlOfferV1;
    const MAX_VERSION: u32 = 1;
    const PRIMARY_SINCE: u32 = 1;
    include!("data_control.rs");
}

/// The wlroots protocol, `zwlr_data_control_v1`.
mod wlr {
    use wayland_protocols_wlr::data_control::v1::client::{
        zwlr_data_control_device_v1 as device, zwlr_data_control_manager_v1 as manager,
        zwlr_data_control_offer_v1 as offer, zwlr_data_control_source_v1 as source,
    };
    type Manager = manager::ZwlrDataControlManagerV1;
    type Device = device::ZwlrDataControlDeviceV1;
    type Source = source::ZwlrDataControlSourceV1;
    type Offer = offer::ZwlrDataControlOfferV1;
    const MAX_VERSION: u32 = 2;
    /// The primary selection came in version 2.
    const PRIMARY_SINCE: u32 = 2;
    include!("data_control.rs");
}

pub use ext::DataControl as ExtDataControl;
pub use wlr::DataControl as WlrDataControl;

pub fn start_ext(session: &Session) -> Result<ExtDataControl, String> {
    ext::start(session)
}

pub fn start_wlr(session: &Session) -> Result<WlrDataControl, String> {
    wlr::start(session)
}
