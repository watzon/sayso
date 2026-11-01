//! Where the model store is (`docs/sdk.md`, "Layout"). These are the rules of
//! `sayso_core::paths`, which an MIT crate cannot use: `XDG_DATA_HOME/sayso` when
//! the variable is set, else the data folder of the system. The test
//! `sdk_paths` of `sayso-host` compares the two.

use std::ffi::OsString;
use std::path::PathBuf;

/// What the rules read from the system. A function, so tests do not touch the real system.
pub(crate) type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn system_env(key: &str) -> Option<OsString> {
    std::env::var_os(key).filter(|value| !value.is_empty())
}

/// The model store of this user: `<models folder of Sayso>/v1`. Local mode uses it also
/// when Sayso is not installed, so a later Sayso finds the models.
pub fn model_store() -> PathBuf {
    store_for(std::env::consts::OS, &system_env)
}

/// The folder of an app for an engine without the store: such an engine must not
/// use the models folder of Sayso. Each `name` has its own.
pub(crate) fn app_models(name: &str) -> PathBuf {
    app_models_for(std::env::consts::OS, &system_env, name)
}

fn home(env: Env<'_>) -> PathBuf {
    env("HOME").or_else(|| env("USERPROFILE")).map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The data folder of the system, without the name of an app.
fn system_data(os: &str, env: Env<'_>) -> PathBuf {
    match os {
        "macos" => home(env).join("Library").join("Application Support"),
        "windows" => env("LOCALAPPDATA").map_or_else(|| home(env).join("AppData").join("Local"), PathBuf::from),
        _ => home(env).join(".local").join("share"),
    }
}

pub(crate) fn store_for(os: &str, env: Env<'_>) -> PathBuf {
    let data = match env("XDG_DATA_HOME") {
        Some(xdg) => PathBuf::from(xdg).join("sayso"),
        None => system_data(os, env).join(if matches!(os, "macos" | "windows") { "Sayso" } else { "sayso" }),
    };
    data.join("models").join("v1")
}

fn app_models_for(os: &str, env: Env<'_>, name: &str) -> PathBuf {
    let base = env("XDG_DATA_HOME").map_or_else(|| system_data(os, env), PathBuf::from);
    let folder: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
    let folder = if folder.trim_matches('.').is_empty() { "sayso-sdk-app".to_string() } else { folder };
    base.join(folder).join("sayso-models")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        let vars: HashMap<&str, &str> = vars.iter().copied().collect();
        move |key| vars.get(key).map(OsString::from)
    }

    #[test]
    fn each_system_has_its_store() {
        let home = [("HOME", "/h")];
        assert_eq!(store_for("macos", &env(&home)), PathBuf::from("/h/Library/Application Support/Sayso/models/v1"));
        assert_eq!(store_for("linux", &env(&home)), PathBuf::from("/h/.local/share/sayso/models/v1"));
        let windows = store_for("windows", &env(&[("LOCALAPPDATA", "C"), ("USERPROFILE", "/u")]));
        assert_eq!(windows, PathBuf::from("C").join("Sayso").join("models").join("v1"));
        assert_eq!(store_for("windows", &env(&[("USERPROFILE", "/u")])), PathBuf::from("/u/AppData/Local/Sayso/models/v1"));
    }

    #[test]
    fn xdg_data_home_counts_on_each_system_as_in_sayso() {
        for os in ["macos", "linux", "windows"] {
            assert_eq!(store_for(os, &env(&[("HOME", "/h"), ("XDG_DATA_HOME", "/d")])), PathBuf::from("/d/sayso/models/v1"), "{os}");
        }
    }

    #[test]
    fn an_app_folder_has_a_safe_name_and_is_not_the_folder_of_sayso() {
        let vars = [("HOME", "/h")];
        assert_eq!(app_models_for("linux", &env(&vars), "My App/2"), PathBuf::from("/h/.local/share/My-App-2/sayso-models"));
        assert_eq!(app_models_for("macos", &env(&vars), ".."), PathBuf::from("/h/Library/Application Support/sayso-sdk-app/sayso-models"));
    }
}
