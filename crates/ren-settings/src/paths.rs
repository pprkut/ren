// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the files go, after the XDG base directory specification. The
//! environment is passed in, so tests don't depend on the real one.

use std::path::{Path, PathBuf};

/// `$XDG_CONFIG_HOME/ren/settings.toml`, falling back to `~/.config`.
/// `var` looks up environment variables.
pub fn settings(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(base(&var, "XDG_CONFIG_HOME", ".config")?.join("ren/settings.toml"))
}

/// `$XDG_STATE_HOME/ren/state.toml`, falling back to `~/.local/state`.
pub fn state(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(base(&var, "XDG_STATE_HOME", ".local/state")?.join("ren/state.toml"))
}

/// The directory in `variable`, else `fallback` in the home directory.
/// Relative paths are invalid in the specification and ignored.
fn base(var: &impl Fn(&str) -> Option<String>, variable: &str, fallback: &str) -> Option<PathBuf> {
    var(variable)
        .filter(|dir| Path::new(dir).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            var("HOME")
                .filter(|home| Path::new(home).is_absolute())
                .map(|home| Path::new(&home).join(fallback))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn xdg_directories() {
        let vars = env(&[
            ("XDG_CONFIG_HOME", "/xdg/config"),
            ("XDG_STATE_HOME", "/xdg/state"),
            ("HOME", "/home/u"),
        ]);
        assert_eq!(
            settings(&vars),
            Some("/xdg/config/ren/settings.toml".into())
        );
        assert_eq!(state(&vars), Some("/xdg/state/ren/state.toml".into()));
    }

    #[test]
    fn home_fallback() {
        let vars = env(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/u")]);
        assert_eq!(
            settings(&vars),
            Some("/home/u/.config/ren/settings.toml".into())
        );
        assert_eq!(
            state(&vars),
            Some("/home/u/.local/state/ren/state.toml".into())
        );
        assert_eq!(settings(env(&[])), None);
        assert_eq!(state(env(&[("HOME", "")])), None);
    }
}
