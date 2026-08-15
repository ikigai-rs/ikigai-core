//! Where configuration LIVES — the one spelling of the ikigai config home and of the
//! layered file paths under it.
//!
//! **Path algebra only. This module performs no filesystem I/O and never will**: reading,
//! parsing and merging are the consumer's business, and keeping them out is what lets the
//! rule live in a core that compiles to wasm. `std::env` is the single ambient dependency
//! (it compiles on wasm32 and simply answers `None` there); `std::fs` must not appear here.
//!
//! It is pushed down because it was written four times independently — in
//! `ikigai_embedded::config`, `ikigai_engine::config`, `ikigai-dev-server::config`, and
//! `ikigai-cms-web` — and the four had already drifted. They AGREE on a machine that
//! leaves `XDG_CONFIG_HOME` unset, which is exactly why the drift survived unnoticed:
//! only a machine that sets the variable can show it, and there `ikigai-cms-web` read
//! `~/.config/ikigai/cms.toml` while everything else read `$XDG_CONFIG_HOME/ikigai/`.
//! Two of the four also took a set-but-empty `XDG_CONFIG_HOME` literally, resolving a
//! relative `ikigai/…` against the working directory — a config home that moved with `cd`.
//!
//! `XDG_CONFIG_HOME` (and `HOME`, which it is defined in terms of) is the ONLY environment
//! variable this module reads, and no new one belongs here: config home plus flags is the
//! rule, and an environment variable is the banned third channel.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The directory name under the XDG config base — every ikigai config file is its child.
const APP_DIR: &str = "ikigai";

/// The ikigai config home: `$XDG_CONFIG_HOME/ikigai`, or `$HOME/.config/ikigai` when
/// `XDG_CONFIG_HOME` is unset. `None` when neither base directory is known.
///
/// `None` rather than a guess. A process with no `HOME` and no `XDG_CONFIG_HOME` has no
/// config home, and the plausible-looking fallbacks are worse than nothing: a relative
/// `./.config/ikigai` is a config home that moves with the working directory, and it reads
/// as success. A caller that genuinely wants such a fallback must write it itself, so the
/// choice is visible where the consequences land.
pub fn config_home() -> Option<PathBuf> {
    config_home_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// [`config_home`] with the environment passed in.
///
/// Public because it is the honest form of the rule: the resolution is a pure function of
/// two strings, and only the sugar above reaches for the process environment. It is also
/// the only way to TEST the rule — the environment is process-global, so `set_var` races
/// the test harness's own threads.
///
/// A set-but-EMPTY `xdg` counts as unset, per the XDG base directory specification.
pub fn config_home_from(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join(APP_DIR))
}

/// The candidate paths for one config `stem`, **lowest precedence first**:
/// `[<config home>/{stem}, <config home>/{app}.{stem}]`, or just the shared file when
/// `app` is `None`. Empty when there is no [`config_home`].
///
/// The layering is key-wise and later-wins: a consumer reads whichever of these exist, in
/// order, and merges — `a11y.toml` states the posture every ikigai front end shares, and
/// `cms-web.a11y.toml` overrides the handful of keys one front end differs on. Nothing here
/// is a11y-specific; `llm.json`, `calendar.json` and the rest express the same way.
///
/// The app-scoped file is a SIBLING of the shared one rather than a child of a per-app
/// subdirectory, so the whole config home stays a flat listing and a `cms-web.` prefix
/// reads as what it is: an override of the file it prefixes.
pub fn layered_paths(stem: &str, app: Option<&str>) -> Vec<PathBuf> {
    config_home().map_or_else(Vec::new, |home| layered_paths_in(&home, stem, app))
}

/// [`layered_paths`] rooted at an explicit directory — the pure form, for tests and for a
/// caller that already knows its config home (or is serving someone else's).
///
/// An empty `stem` yields no paths: the alternative is handing back the config DIRECTORY
/// dressed as a config file. An empty `app` is treated as absent for the same reason —
/// `Some("")` would name a hidden `.{stem}` beside the file it means to override.
pub fn layered_paths_in(home: &Path, stem: &str, app: Option<&str>) -> Vec<PathBuf> {
    if stem.is_empty() {
        return Vec::new();
    }
    let mut paths = vec![home.join(stem)];
    if let Some(app) = app.filter(|app| !app.is_empty()) {
        paths.push(home.join(format!("{app}.{stem}")));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::{config_home, config_home_from, layered_paths, layered_paths_in, APP_DIR};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    /// `XDG_CONFIG_HOME` decides the config home when it is set — including with no `HOME`
    /// to fall back to. The spelling this replaces in `ikigai-cms-web` ignored the variable
    /// entirely, so on a machine that sets it the CMS read a different directory from the
    /// rest of the system.
    #[test]
    fn xdg_config_home_decides_the_config_home_when_set() {
        assert_eq!(
            config_home_from(Some("/xdg".into()), Some("/home/b".into())),
            Some(PathBuf::from("/xdg/ikigai"))
        );
        assert_eq!(
            config_home_from(Some("/xdg".into()), None),
            Some(PathBuf::from("/xdg/ikigai"))
        );
    }

    /// Unset — and set-but-empty, which the XDG spec says to read as unset — falls back to
    /// `$HOME/.config/ikigai`. Empty used to resolve a RELATIVE `ikigai/…`.
    #[test]
    fn an_unset_or_empty_xdg_falls_back_to_home_config() {
        assert_eq!(
            config_home_from(None, Some("/home/b".into())),
            Some(PathBuf::from("/home/b/.config/ikigai"))
        );
        assert_eq!(
            config_home_from(Some(OsString::new()), Some("/home/b".into())),
            Some(PathBuf::from("/home/b/.config/ikigai"))
        );
    }

    /// Neither base directory known: `None`, so each caller applies its OWN fallback rather
    /// than inheriting a working-directory-relative one it did not choose.
    #[test]
    fn no_base_directory_at_all_is_none() {
        assert_eq!(config_home_from(None, None), None);
    }

    /// The env-reading sugar agrees with the pure rule on whatever the ambient environment
    /// happens to be — asserted against the rule, not against a fixed path, because the
    /// test harness's environment is not ours to pin.
    #[test]
    fn the_ambient_config_home_matches_the_rule() {
        assert_eq!(
            config_home(),
            config_home_from(
                std::env::var_os("XDG_CONFIG_HOME"),
                std::env::var_os("HOME")
            )
        );
        if let Some(home) = config_home() {
            assert_eq!(home.file_name(), Some(APP_DIR.as_ref()));
        }
    }

    /// Shared file first, app override second: the ORDER is the precedence, and a consumer
    /// that merges in the order given gets later-wins for free.
    #[test]
    fn layering_orders_shared_before_the_app_override() {
        let home = Path::new("/cfg/ikigai");
        assert_eq!(
            layered_paths_in(home, "a11y.toml", Some("cms-web")),
            vec![
                PathBuf::from("/cfg/ikigai/a11y.toml"),
                PathBuf::from("/cfg/ikigai/cms-web.a11y.toml"),
            ]
        );
    }

    /// No app, no override entry — the shared file alone, not a one-element list with a
    /// hole in it.
    #[test]
    fn no_app_yields_the_shared_file_alone() {
        let home = Path::new("/cfg/ikigai");
        assert_eq!(
            layered_paths_in(home, "llm.json", None),
            vec![PathBuf::from("/cfg/ikigai/llm.json")]
        );
        // An empty app name is absent, not an override called `.llm.json`.
        assert_eq!(
            layered_paths_in(home, "llm.json", Some("")),
            layered_paths_in(home, "llm.json", None)
        );
    }

    /// An empty stem names no file. Returning the config home itself would hand a consumer
    /// a directory to open as a config file.
    #[test]
    fn an_empty_stem_names_no_file() {
        assert!(layered_paths_in(Path::new("/cfg/ikigai"), "", Some("cms-web")).is_empty());
        assert!(layered_paths("", None).is_empty());
    }

    /// The sugar is the pure form rooted at the ambient config home — and empty when there
    /// is no config home at all, which is the only case a consumer must handle specially.
    #[test]
    fn the_ambient_layering_is_the_pure_one_rooted_at_the_config_home() {
        match config_home() {
            Some(home) => assert_eq!(
                layered_paths("a11y.toml", Some("cms-web")),
                layered_paths_in(&home, "a11y.toml", Some("cms-web"))
            ),
            None => assert!(layered_paths("a11y.toml", Some("cms-web")).is_empty()),
        }
    }
}
