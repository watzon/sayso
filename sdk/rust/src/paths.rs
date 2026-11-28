//! Where Sayso keeps its data, its models, and the model store
//! (`sdk/protocol/paths.md`, "Data and models folders"). Local mode uses the same folders as
//! Sayso, so the two share the model files.

use std::ffi::OsString;
use std::path::PathBuf;

/// What the rules read from the system. A function, so tests do not touch the real system.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn system_env(key: &str) -> Option<OsString> {
    std::env::var_os(key).filter(|value| !value.is_empty())
}

/// The data folder of Sayso for this user.
pub fn data_dir() -> PathBuf {
    data_for(std::env::consts::OS, &system_env)
}

/// The models folder of Sayso: `<data>/models`. Engines get it as `SAYSO_MODELS_DIR`.
pub fn models_dir() -> PathBuf {
    models_for(std::env::consts::OS, &system_env)
}

/// The folder of the cleanup models: `<models>/cleanup`.
pub fn cleanup_models_dir() -> PathBuf {
    cleanup_for(std::env::consts::OS, &system_env)
}

/// The model store of this user: `<models folder of Sayso>/v1`. Local mode uses it also
/// when Sayso is not installed, so a later Sayso finds the models.
pub fn model_store() -> PathBuf {
    store_for(std::env::consts::OS, &system_env)
}

/// The folder of an app for an engine without the store: such an engine must not
/// use the models folder of Sayso. Each `name` has its own.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub(crate) fn app_models(name: &str) -> PathBuf {
    app_models_for(std::env::consts::OS, &system_env, name)
}

fn home(env: Env<'_>) -> PathBuf {
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The data folder of the system, without the name of an app.
fn system_data(os: &str, env: Env<'_>) -> PathBuf {
    match os {
        "macos" => home(env).join("Library").join("Application Support"),
        "windows" => env("LOCALAPPDATA")
            .map_or_else(|| home(env).join("AppData").join("Local"), PathBuf::from),
        _ => home(env).join(".local").join("share"),
    }
}

/// `os` is a value of `std::env::consts::OS`.
pub fn data_for(os: &str, env: Env<'_>) -> PathBuf {
    match env("XDG_DATA_HOME") {
        Some(xdg) => PathBuf::from(xdg).join("sayso"),
        None => system_data(os, env).join(if matches!(os, "macos" | "windows") {
            "Sayso"
        } else {
            "sayso"
        }),
    }
}

pub fn models_for(os: &str, env: Env<'_>) -> PathBuf {
    data_for(os, env).join("models")
}

pub fn cleanup_for(os: &str, env: Env<'_>) -> PathBuf {
    models_for(os, env).join("cleanup")
}

pub fn store_for(os: &str, env: Env<'_>) -> PathBuf {
    models_for(os, env).join("v1")
}

#[cfg_attr(not(feature = "local"), allow(dead_code))]
fn app_models_for(os: &str, env: Env<'_>, name: &str) -> PathBuf {
    let base = env("XDG_DATA_HOME").map_or_else(|| system_data(os, env), PathBuf::from);
    let folder: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let folder = if folder.trim_matches('.').is_empty() {
        "sayso-sdk-app".to_string()
    } else {
        folder
    };
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
        assert_eq!(
            store_for("macos", &env(&home)),
            PathBuf::from("/h/Library/Application Support/Sayso/models/v1")
        );
        assert_eq!(
            store_for("linux", &env(&home)),
            PathBuf::from("/h/.local/share/sayso/models/v1")
        );
        let windows = store_for(
            "windows",
            &env(&[("LOCALAPPDATA", "C"), ("USERPROFILE", "/u")]),
        );
        assert_eq!(
            windows,
            PathBuf::from("C").join("Sayso").join("models").join("v1")
        );
        assert_eq!(
            store_for("windows", &env(&[("USERPROFILE", "/u")])),
            PathBuf::from("/u/AppData/Local/Sayso/models/v1")
        );
    }

    #[test]
    fn xdg_data_home_counts_on_each_system_as_in_sayso() {
        for os in ["macos", "linux", "windows"] {
            assert_eq!(
                store_for(os, &env(&[("HOME", "/h"), ("XDG_DATA_HOME", "/d")])),
                PathBuf::from("/d/sayso/models/v1"),
                "{os}"
            );
        }
    }

    #[test]
    fn an_app_folder_has_a_safe_name_and_is_not_the_folder_of_sayso() {
        let vars = [("HOME", "/h")];
        assert_eq!(
            app_models_for("linux", &env(&vars), "My App/2"),
            PathBuf::from("/h/.local/share/My-App-2/sayso-models")
        );
        assert_eq!(
            app_models_for("macos", &env(&vars), ".."),
            PathBuf::from("/h/Library/Application Support/sayso-sdk-app/sayso-models")
        );
    }
}
