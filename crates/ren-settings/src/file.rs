// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings file: loading, reloading when it changed, and changing
//! single settings in it.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::edit;
use crate::error::Error;
use crate::schema::{SETTINGS, Setting, Value};
use crate::settings::{Loaded, Problem, Problems, Settings, Severity};

/// The settings file and the last good settings read from it.
///
/// [`SettingsFile::reload`] checks whether the file changed and reads it
/// again; the app calls it when its window gains focus, so edits in
/// another editor apply when the user comes back. A file that can't be
/// used leaves the last good settings in effect.
#[derive(Debug)]
pub struct SettingsFile {
    path: PathBuf,
    table: &'static [Setting],
    settings: Settings,
    /// The file as last read; `None` if it didn't exist.
    stamp: Option<Stamp>,
}

/// What [`SettingsFile::reload`] found.
#[derive(Debug, PartialEq)]
pub enum Reload {
    /// The file is as it was.
    Unchanged,
    /// The file changed and its settings are in effect, with these
    /// warnings.
    Loaded(Vec<Problem>),
    /// The file changed, but can't be used; the last good settings stay.
    Failed(Problems),
}

/// What identifies a version of the file without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    inode: (u64, u64),
}

impl Stamp {
    fn of(path: &Path) -> io::Result<Option<Stamp>> {
        use std::os::unix::fs::MetadataExt;
        match fs::metadata(path) {
            Ok(meta) => Ok(Some(Stamp {
                modified: meta.modified().ok(),
                len: meta.len(),
                inode: (meta.dev(), meta.ino()),
            })),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }
}

impl SettingsFile {
    /// Loads the settings with the settings of [`SETTINGS`]. A missing file
    /// has the defaults. If the file can't be used, the defaults are in
    /// effect and the problems are returned, along with the file.
    pub fn open(path: impl Into<PathBuf>) -> (SettingsFile, Reload) {
        SettingsFile::open_with(path, SETTINGS)
    }

    /// Loads the settings with the settings of `table`.
    pub fn open_with(
        path: impl Into<PathBuf>,
        table: &'static [Setting],
    ) -> (SettingsFile, Reload) {
        let mut file = SettingsFile {
            path: path.into(),
            table,
            settings: Settings::defaults(table),
            stamp: None,
        };
        let reload = file.load();
        (file, reload)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The settings in effect.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Reads the file again if it changed since it was last read.
    pub fn reload(&mut self) -> Reload {
        match Stamp::of(&self.path) {
            Ok(stamp) if stamp == self.stamp => Reload::Unchanged,
            _ => self.load(),
        }
    }

    fn load(&mut self) -> Reload {
        // The stamp is taken before reading, so a change while reading is
        // seen next time.
        let stamp = Stamp::of(&self.path);
        let text = match read(&self.path) {
            Ok(text) => text,
            Err(err) => {
                self.stamp = stamp.ok().flatten();
                return Reload::Failed(Problems(vec![Problem {
                    severity: Severity::Error,
                    position: None,
                    message: err.to_string(),
                }]));
            }
        };
        self.stamp = stamp.ok().flatten();
        match Settings::parse_with(&text, self.table) {
            Ok(Loaded { settings, warnings }) => {
                self.settings = settings;
                Reload::Loaded(warnings)
            }
            Err(problems) => Reload::Failed(problems),
        }
    }

    /// Sets `key` to `value` in the file and in effect. The file must be
    /// usable; one with errors is left alone until they're fixed.
    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), Error> {
        self.change(|text, table| edit::set(text, table, key, value))
    }

    /// Removes `key` from the file, so it has its default.
    pub fn reset(&mut self, key: &str) -> Result<(), Error> {
        self.change(|text, table| edit::reset(text, table, key))
    }

    fn change(
        &mut self,
        edit: impl FnOnce(&str, &[Setting]) -> Result<String, Error>,
    ) -> Result<(), Error> {
        let io_error = |err| Error::Io {
            path: self.path.clone(),
            err,
        };
        let text = read(&self.path).map_err(io_error)?;
        // What's in effect may be older than the file.
        Settings::parse_with(&text, self.table).map_err(Error::Problems)?;
        let changed = edit(&text, self.table)?;
        if changed != text {
            write_atomically(&self.path, &changed).map_err(io_error)?;
        }
        match self.load() {
            Reload::Failed(problems) => Err(Error::Problems(problems)),
            _ => Ok(()),
        }
    }
}

/// The text of the file; a missing file is empty.
fn read(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        result => result,
    }
}

/// Replaces the file at `path` with `contents` in one step, so that a
/// crash or a full disk never leaves half a file: the new contents go to
/// a temporary file next to it, which is then renamed. A symbolic link is
/// followed (dotfiles kept elsewhere), and an existing file keeps its
/// permissions. Missing directories are created.
pub fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let target = match fs::canonicalize(path) {
        Ok(target) => target,
        Err(err) if err.kind() == io::ErrorKind::NotFound => path.to_owned(),
        Err(err) => return Err(err),
    };
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a file name",
        ));
    };
    fs::create_dir_all(dir)?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(format!(".{}.tmp", std::process::id()));
    let temporary = dir.join(temporary_name);
    let result = (|| {
        let mut file = File::create(&temporary)?;
        if let Ok(meta) = fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::TABLE;

    /// A new, empty directory for one test.
    pub(crate) fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ren-settings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `text` so that the file looks changed even within the
    /// resolution of the modification time: through a new inode.
    fn rewrite(path: &Path, text: &str) {
        write_atomically(path, text).unwrap();
    }

    #[test]
    fn missing_file_has_the_defaults() {
        let dir = test_dir("missing");
        let (file, reload) = SettingsFile::open_with(dir.join("settings.toml"), TABLE);
        assert_eq!(reload, Reload::Loaded(vec![]));
        assert_eq!(*file.settings(), Settings::defaults(TABLE));
    }

    #[test]
    fn reload_when_changed() {
        let dir = test_dir("reload");
        let path = dir.join("settings.toml");
        rewrite(&path, "[b]\nnumber = 2\n");
        let (mut file, reload) = SettingsFile::open_with(&path, TABLE);
        assert_eq!(reload, Reload::Loaded(vec![]));
        assert_eq!(file.settings().number("b.number"), 2);
        assert_eq!(file.reload(), Reload::Unchanged);

        rewrite(&path, "[b]\nnumber = 3\nx = 1\n");
        let Reload::Loaded(warnings) = file.reload() else {
            panic!("not reloaded");
        };
        assert_eq!(warnings.len(), 1);
        assert_eq!(file.settings().number("b.number"), 3);

        // The last good settings stay.
        rewrite(&path, "[b]\nnumber = 30\n");
        let Reload::Failed(problems) = file.reload() else {
            panic!("not failed");
        };
        assert_eq!(
            problems.to_string(),
            "2:10: b.number must be between 1 and 10"
        );
        assert_eq!(file.settings().number("b.number"), 3);
        assert_eq!(file.reload(), Reload::Unchanged);

        // Removed: the defaults.
        fs::remove_file(&path).unwrap();
        assert_eq!(file.reload(), Reload::Loaded(vec![]));
        assert_eq!(file.settings().number("b.number"), 5);
        assert_eq!(file.reload(), Reload::Unchanged);
    }

    #[test]
    fn unreadable_file() {
        let dir = test_dir("unreadable");
        // A directory where the file should be.
        let path = dir.join("settings.toml");
        fs::create_dir(&path).unwrap();
        let (file, reload) = SettingsFile::open_with(&path, TABLE);
        assert!(matches!(reload, Reload::Failed(_)), "{reload:?}");
        assert_eq!(*file.settings(), Settings::defaults(TABLE));
    }

    #[test]
    fn set_and_reset() {
        let dir = test_dir("set");
        let path = dir.join("ren").join("settings.toml");
        let (mut file, _) = SettingsFile::open_with(&path, TABLE);
        file.set("b.number", &Value::Number(8)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "[b]\nnumber = 8\n");
        assert_eq!(file.settings().number("b.number"), 8);
        assert_eq!(file.reload(), Reload::Unchanged);

        file.reset("b.number").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert_eq!(file.settings().number("b.number"), 5);

        assert!(matches!(
            file.set("b.number", &Value::Number(11)),
            Err(Error::Invalid(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn set_refuses_a_file_with_errors() {
        let dir = test_dir("refuse");
        let path = dir.join("settings.toml");
        rewrite(&path, "[b]\nnumber = 2\n");
        let (mut file, _) = SettingsFile::open_with(&path, TABLE);
        rewrite(&path, "[b]\nnumber = 20\n");
        let err = file.set("a.switch", &Value::Bool(false)).unwrap_err();
        assert!(matches!(err, Error::Problems(_)), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "[b]\nnumber = 20\n");
    }

    #[test]
    fn writing_follows_links_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir("links");
        let real = dir.join("dotfiles").join("settings.toml");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "[b]\nnumber = 2\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("settings.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let (mut file, _) = SettingsFile::open_with(&link, TABLE);
        file.set("b.number", &Value::Number(4)).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "[b]\nnumber = 4\n");
        let mode = fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // No temporary file left behind.
        assert_eq!(fs::read_dir(real.parent().unwrap()).unwrap().count(), 1);
    }
}
