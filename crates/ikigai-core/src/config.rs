//! Where ikigai's own directories LIVE — the one spelling of the config home, of the
//! layered file paths under it, and of the data home.
//!
//! Config and data are one rule wearing two hats ("where does ikigai keep its things"), so
//! they share a module: splitting them across two files is how a codebase ends up with two
//! answers again. The module is named for its first tenant, not for its boundary.
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
//! `XDG_CONFIG_HOME` and `HOME` are the ONLY environment variables this module reads, and no
//! new one belongs here: config home plus flags is the rule, and an environment variable is
//! the banned third channel. The overrides that predate that rule — `IKIGAI_FILES`,
//! `IKIGAI_SECRETS`, `IKIGAI_CALENDAR_CONFIG` — stay in the consumers that honour them, so
//! that retiring them stays one visible decision per consumer rather than a channel core
//! quietly reopens for everybody.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The directory name under the XDG config base — every ikigai config file is its child.
const APP_DIR: &str = "ikigai";

/// The directory name under `$HOME` that IS the data home — dotted, because it sits directly
/// in the home directory rather than under an XDG base.
const DATA_DIR: &str = ".ikigai";

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
/// A set-but-EMPTY `xdg` counts as unset, per the XDG base directory specification, and a
/// set-but-empty `home` counts as unset for the same reason it does there: joining onto it
/// yields a relative `.config/ikigai`, which is the working-directory-relative config home
/// this function exists to refuse.
pub fn config_home_from(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|base| base.join(APP_DIR))
}

/// The ikigai data home: `$HOME/.ikigai`. `None` when `HOME` is unknown.
///
/// **Deliberately not XDG** — the asymmetry with [`config_home`] is the decision, not an
/// oversight, and it rests on what the directory is FOR:
///
/// 1. **It is a rendezvous, not a private store.** `~/.ikigai/dev.sock` is how one process
///    finds another: the dev server binds it and every other host mounts it. Two processes
///    that disagree about the CONFIG home each fail to find a file — loud, and each still
///    runs. Two that disagree about the DATA home simply never meet. A launchd agent
///    inherits none of a login shell's environment, so an `XDG_DATA_HOME` exported from a
///    shell profile would split exactly that pair, on exactly this machine.
/// 2. **Sockets are not data.** XDG puts sockets in `$XDG_RUNTIME_DIR`, not
///    `$XDG_DATA_HOME`. Honouring `XDG_DATA_HOME` here would not make this directory
///    XDG-conformant; it would make it half-conformant and owe a second rule for the rest.
/// 3. **Live consumers name it literally.** `health-watch.sh` reads `~/.ikigai/health/`,
///    launchd plists name `~/.ikigai/host.sock` and `~/.ikigai/quic-drain`, and machines
///    carry hand-written `mount = "prefer urn:repo:=~/.ikigai/dev.sock"` lines. Moving the
///    default is a migration of things outside any Rust workspace. Giving the location it
///    already has ONE spelling is not.
///
/// `None` rather than a guess, exactly as [`config_home`] returns it — and here the harm is
/// not hypothetical: two of the spellings this replaces resolve a working-directory-relative
/// `.ikigai/…` when `HOME` is unset, so a health heartbeat and a link-status cache land
/// wherever the process happened to start and every reader looks in the right place and
/// finds nothing.
pub fn data_home() -> Option<PathBuf> {
    data_home_from(std::env::var_os("HOME"))
}

/// [`data_home`] with the environment passed in — the pure form, and the only testable one,
/// since the process environment is global and `set_var` races the harness's own threads.
///
/// One argument where [`config_home_from`] takes two: that IS the rule. There is no
/// `XDG_DATA_HOME` step to model, and adding the parameter to look symmetric would advertise
/// a variable this module refuses to read.
///
/// A set-but-EMPTY `home` counts as unset: `PathBuf::from("").join(".ikigai")` is a relative
/// `.ikigai`, which is the failure dressed as success.
pub fn data_home_from(home: Option<OsString>) -> Option<PathBuf> {
    home.filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(DATA_DIR))
}

/// The path of one `stem` in the data home — a file (`cms-tags-approved.ttl`) or a directory
/// (`health`, `workspace`, `secrets`). `None` when there is no [`data_home`].
///
/// The data home has no [`layered_paths`] twin and wants none: layering answers "which of
/// several files states this setting", and a tag overlay or a socket is not several files.
pub fn data_path(stem: &str) -> Option<PathBuf> {
    data_home().and_then(|home| data_path_in(&home, stem))
}

/// [`data_path`] rooted at an explicit directory — for tests, and for a caller handed a data
/// home rather than reading its own (a per-tenant root, a temp dir standing in for `$HOME`).
///
/// An empty `stem` names nothing, for the reason an empty stem yields no [`layered_paths`]:
/// the alternative hands back the data DIRECTORY dressed as an entry in it.
pub fn data_path_in(home: &Path, stem: &str) -> Option<PathBuf> {
    (!stem.is_empty()).then(|| home.join(stem))
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
    use super::{
        config_home, config_home_from, data_home, data_home_from, data_path, data_path_in,
        layered_paths, layered_paths_in, APP_DIR, DATA_DIR,
    };
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
    /// than inheriting a working-directory-relative one it did not choose. A set-but-empty
    /// `HOME` is the same case — joining onto it yields a relative `.config/ikigai`.
    #[test]
    fn no_base_directory_at_all_is_none() {
        assert_eq!(config_home_from(None, None), None);
        assert_eq!(config_home_from(None, Some(OsString::new())), None);
        assert_eq!(
            config_home_from(Some(OsString::new()), Some(OsString::new())),
            None
        );
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

    /// The data home is `$HOME/.ikigai` and nothing else consults it — no XDG step. Asserted
    /// with an `XDG_DATA_HOME`-shaped value nowhere in sight, because the signature is the
    /// guarantee: there is no parameter for a variable this rule refuses to read.
    #[test]
    fn the_data_home_is_dot_ikigai_under_home() {
        assert_eq!(
            data_home_from(Some("/home/b".into())),
            Some(PathBuf::from("/home/b/.ikigai"))
        );
    }

    /// No `HOME` — and a set-but-empty `HOME`, which joins into a relative `.ikigai` — is
    /// `None`. The spellings this replaces returned that relative path and read as success:
    /// a heartbeat written where the process started, and every watcher looking elsewhere.
    #[test]
    fn an_unknown_or_empty_home_has_no_data_home() {
        assert_eq!(data_home_from(None), None);
        assert_eq!(data_home_from(Some(OsString::new())), None);
    }

    /// The env-reading sugar agrees with the pure rule on whatever environment the harness
    /// happens to have — asserted against the rule, not a fixed path, which is not ours to
    /// pin.
    #[test]
    fn the_ambient_data_home_matches_the_rule() {
        assert_eq!(data_home(), data_home_from(std::env::var_os("HOME")));
        if let Some(home) = data_home() {
            assert_eq!(home.file_name(), Some(DATA_DIR.as_ref()));
        }
    }

    /// A stem names one entry in the data home, file or directory alike — the multi-segment
    /// form is a plain join, so `health/x.txt` needs no separate spelling.
    #[test]
    fn a_stem_names_one_entry_in_the_data_home() {
        let home = Path::new("/home/b/.ikigai");
        assert_eq!(
            data_path_in(home, "cms-tags-approved.ttl"),
            Some(PathBuf::from("/home/b/.ikigai/cms-tags-approved.ttl"))
        );
        assert_eq!(
            data_path_in(home, "health/plasma.txt"),
            Some(PathBuf::from("/home/b/.ikigai/health/plasma.txt"))
        );
    }

    /// An empty stem names nothing. Returning the data home itself would hand a consumer the
    /// directory to open — or truncate — as if it were a file in it.
    #[test]
    fn an_empty_stem_names_no_entry() {
        assert_eq!(data_path_in(Path::new("/home/b/.ikigai"), ""), None);
        assert_eq!(data_path(""), None);
    }

    /// The sugar is the pure form rooted at the ambient data home — and `None` when there is
    /// no data home at all, which is the only case a consumer must handle specially.
    #[test]
    fn the_ambient_data_path_is_the_pure_one_rooted_at_the_data_home() {
        assert_eq!(
            data_path("workspace"),
            data_home().and_then(|home| data_path_in(&home, "workspace"))
        );
    }
}
