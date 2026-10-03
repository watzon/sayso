//! [`SystemPrefs`] from the settings portal (`org.freedesktop.portal.Settings`).

use sayso_platform::SystemPrefs;

pub struct LinuxPrefs;

impl SystemPrefs for LinuxPrefs {
    /// `org.freedesktop.appearance color-scheme`: 1 means "prefer dark".
    fn dark_mode(&self) -> bool {
        match read_setting("org.freedesktop.appearance", "color-scheme") {
            Some(Value::U32(scheme)) => scheme == 1,
            _ => std::env::var("GTK_THEME").is_ok_and(|t| t.to_ascii_lowercase().contains("dark")),
        }
    }

    /// GNOME's `enable-animations` off, or KDE's animation speed at zero.
    fn reduce_motion(&self) -> bool {
        if let Some(Value::Bool(on)) = read_setting("org.gnome.desktop.interface", "enable-animations") {
            return !on;
        }
        matches!(read_setting("org.kde.kdeglobals.KDE", "AnimationDurationFactor"), Some(Value::F64(f)) if f == 0.0)
    }
}

/// The value types the settings above use.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    U32(u32),
    Bool(bool),
    F64(f64),
}

/// Read one setting through the portal. None when the portal or the key is missing.
pub fn read_setting(namespace: &str, key: &str) -> Option<Value> {
    use zbus::zvariant::OwnedValue;
    // The UI asks often (it follows the system appearance), so keep one connection.
    static CONNECTION: std::sync::OnceLock<Option<zbus::blocking::Connection>> = std::sync::OnceLock::new();
    let connection = CONNECTION.get_or_init(|| zbus::blocking::Connection::session().ok()).as_ref()?;
    let proxy = zbus::blocking::Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )
    .ok()?;
    // `ReadOne` (version 2) returns the value; the older `Read` wraps it once more.
    let value: OwnedValue = match proxy.call("ReadOne", &(namespace, key)) {
        Ok(v) => v,
        Err(_) => {
            let outer: OwnedValue = proxy.call("Read", &(namespace, key)).ok()?;
            unwrap_variant(outer)
        }
    };
    convert(unwrap_variant(value))
}

fn unwrap_variant(value: zbus::zvariant::OwnedValue) -> zbus::zvariant::OwnedValue {
    match &*value {
        zbus::zvariant::Value::Value(inner) => inner.try_to_owned().unwrap_or(value),
        _ => value,
    }
}

fn convert(value: zbus::zvariant::OwnedValue) -> Option<Value> {
    match &*value {
        zbus::zvariant::Value::U32(v) => Some(Value::U32(*v)),
        zbus::zvariant::Value::I32(v) => u32::try_from(*v).ok().map(Value::U32),
        zbus::zvariant::Value::Bool(v) => Some(Value::Bool(*v)),
        zbus::zvariant::Value::F64(v) => Some(Value::F64(*v)),
        zbus::zvariant::Value::Str(s) => s.parse::<f64>().ok().map(Value::F64),
        _ => None,
    }
}
