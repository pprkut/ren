// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings dialog: a window of its own, made when it opens and
//! dropped when it closes. Its rows come from `settings_dialog`; every
//! change is written to the settings file and applied at once.

use std::rc::Rc;
use std::time::Duration;

use ren_settings::Value;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::{App, SettingRow, SettingsWindow};
use crate::settings_dialog::{Control, Dialog, Edit, Row};

/// The time after the settings file was saved until it is read: editors
/// may write it in more than one step.
pub(super) const RELOAD_DELAY: Duration = Duration::from_millis(100);

/// What a new settings file starts with, when the dialog opens it.
const NEW_FILE: &str = "\
# Settings of ren. Only the settings changed from their defaults are kept
# here; Settings → Configure ren… lists them all. Changes apply when the
# file is saved.
";

/// The open dialog.
pub(super) struct SettingsDialog {
    window: SettingsWindow,
    model: Dialog,
    rows: Rc<VecModel<SettingRow>>,
    /// The settings the rows show, to update them in place.
    keys: Vec<&'static str>,
}

impl App {
    /// Opens the settings dialog, or brings it to the front.
    pub(super) fn open_settings(&self) {
        if let Some(dialog) = &*self.dialog.borrow() {
            let _ = dialog.window.show();
            return;
        }
        if self.settings.borrow().is_none() {
            self.status("Demo data has no settings");
            return;
        }
        let window = match SettingsWindow::new() {
            Ok(window) => window,
            Err(err) => {
                self.status(format!("Opening the settings failed: {err}"));
                return;
            }
        };
        if let Some(dark) = self.forced_dark {
            window.set_forced_color_scheme(if dark {
                slint::language::ColorScheme::Dark
            } else {
                slint::language::ColorScheme::Light
            });
        }
        let model = Dialog::new();
        let categories: Vec<SharedString> = model
            .categories()
            .iter()
            .map(|category| category.title().into())
            .collect();
        window.set_categories(ModelRc::new(VecModel::from(categories)));
        let rows = Rc::new(VecModel::default());
        window.set_rows(ModelRc::from(rows.clone()));
        self.connect_settings(&window);
        self.connect_account(&window);
        *self.dialog.borrow_mut() = Some(SettingsDialog {
            window,
            model,
            rows,
            keys: Vec::new(),
        });
        self.refresh_settings_dialog();
        if let Some(dialog) = &*self.dialog.borrow()
            && let Err(err) = dialog.window.show()
        {
            eprintln!("ren: showing the settings: {err}");
        }
    }

    fn connect_settings(&self, window: &SettingsWindow) {
        let this = self.this.clone();
        let with = move |f: &dyn Fn(&App)| {
            if let Some(app) = this.upgrade() {
                f(&app);
            }
        };
        let app = with.clone();
        window.on_search_edited(move |text| {
            app(&|app| app.with_dialog(|dialog| dialog.search(&text)));
        });
        let app = with.clone();
        window.on_category_selected(move |index| {
            app(&|app| {
                app.with_dialog(|dialog| {
                    if let Some(category) = usize::try_from(index)
                        .ok()
                        .and_then(|i| dialog.categories().get(i).copied())
                    {
                        dialog.select(category);
                    }
                });
                if let Some(dialog) = &*app.dialog.borrow() {
                    dialog.window.invoke_scroll_to_top();
                }
            });
        });
        let app = with.clone();
        window.on_toggled(move |key, on| {
            app(&|app| app.settings_edit(&key, |dialog| dialog.switch(&key, on)));
        });
        let app = with.clone();
        window.on_chosen(move |key, index| {
            let index = usize::try_from(index).unwrap_or(usize::MAX);
            app(&|app| app.settings_edit(&key, |dialog| dialog.choose(&key, index)));
        });
        let app = with.clone();
        window.on_input_accepted(move |key, text| {
            app(&|app| app.settings_edit(&key, |dialog| dialog.input(&key, &text)));
        });
        let app = with.clone();
        window.on_reset(move |key| {
            app(&|app| app.settings_edit(&key, |dialog| dialog.reset(&key)));
        });
        let app = with.clone();
        window.on_open_file(move || app(&App::open_settings_file));
        let app = with;
        window.window().on_close_requested(move || {
            app(&App::close_settings);
            slint::CloseRequestResponse::HideWindow
        });
    }

    /// Drops the dialog once it is hidden: not from its own callback.
    fn close_settings(&self) {
        let this = self.this.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(app) = this.upgrade() {
                app.dialog.borrow_mut().take();
            }
        });
    }

    /// Changes the dialog's state and shows the rows that result.
    pub(super) fn with_dialog(&self, f: impl FnOnce(&mut Dialog)) {
        if let Some(dialog) = &mut *self.dialog.borrow_mut() {
            f(&mut dialog.model);
        }
        self.refresh_settings_dialog();
    }

    /// An edit in a row: written to the file and applied, unless what
    /// was typed isn't a valid value.
    fn settings_edit(&self, key: &str, f: impl FnOnce(&mut Dialog) -> Edit) {
        let edit = match &mut *self.dialog.borrow_mut() {
            Some(dialog) => f(&mut dialog.model),
            None => return,
        };
        match edit {
            Edit::Set(value) => self.change_setting(key, Some(&value)),
            Edit::Reset => self.change_setting(key, None),
            Edit::Invalid => {}
        }
        self.refresh_settings_dialog();
    }

    /// Sets a setting in the file (`None` resets it) and applies the
    /// settings. A file with errors isn't changed.
    pub(super) fn change_setting(&self, key: &str, value: Option<&Value>) {
        let result = {
            let mut file = self.settings.borrow_mut();
            let Some(file) = file.as_mut() else {
                return;
            };
            match value {
                Some(value) => file.set(key, value),
                None => file.reset(key),
            }
            .map(|()| file.settings().clone())
        };
        match result {
            Ok(settings) => {
                self.settings_problem.borrow_mut().take();
                self.apply_settings(&settings);
            }
            Err(err) => {
                let message = format!("Changing the settings failed: {err}");
                if matches!(err, ren_settings::Error::Problems(_)) {
                    *self.settings_problem.borrow_mut() = Some(file_problem(&err.to_string()));
                } else {
                    self.status(message.clone());
                }
                eprintln!("ren: {message}");
            }
        }
        self.refresh_settings_dialog();
    }

    /// Shows the settings in effect in the dialog, if it is open.
    pub(super) fn refresh_settings_dialog(&self) {
        let settings = self.current_settings();
        let problem = self.settings_problem.borrow().clone();
        let mut dialog = self.dialog.borrow_mut();
        let Some(dialog) = &mut *dialog else {
            return;
        };
        let rows = dialog.model.rows(&settings);
        let keys: Vec<&'static str> = rows.iter().map(|row| row.setting.key).collect();
        if keys == dialog.keys {
            // In place, so that the fields keep the focus.
            for (i, row) in rows.iter().enumerate() {
                dialog.rows.set_row_data(i, setting_row(row));
            }
        } else {
            dialog
                .rows
                .set_vec(rows.iter().map(setting_row).collect::<Vec<_>>());
            dialog.keys = keys;
        }
        let window = &dialog.window;
        let current = if dialog.model.searching() {
            -1
        } else {
            dialog
                .model
                .categories()
                .iter()
                .position(|category| *category == dialog.model.category())
                .map_or(-1, |i| i as i32)
        };
        window.set_current_category(current);
        window.set_no_match(dialog.model.searching() && rows.is_empty());
        window.set_editable(problem.is_none());
        window.set_problem(problem.unwrap_or_default().into());
        window.set_account_page(dialog.model.account_page());
        self.show_account(window, &settings);
    }

    /// Opens the settings file in the desktop's editor for it; a missing
    /// one is made first, with a comment.
    fn open_settings_file(&self) {
        let Some(path) = self
            .settings
            .borrow()
            .as_ref()
            .map(|file| file.path().to_owned())
        else {
            return;
        };
        if !path.exists()
            && let Err(err) = ren_settings::write_atomically(&path, NEW_FILE)
        {
            self.status(format!("{}: {err}", path.display()));
            return;
        }
        self.xdg_open(path.as_os_str());
    }

    /// The settings file was saved (by ren or another program): it is read
    /// a moment later.
    pub(super) fn settings_saved(&self) {
        let this = self.this.clone();
        self.reload_timer
            .start(slint::TimerMode::SingleShot, RELOAD_DELAY, move || {
                if let Some(app) = this.upgrade() {
                    app.reload_settings();
                }
            });
    }
}

/// The dialog's banner for a settings file with errors.
pub(super) fn file_problem(problems: &str) -> String {
    format!(
        "The settings file has errors. Until they are fixed, the settings in effect stay \
         and changes here aren't saved.\n{problems}"
    )
}

fn setting_row(row: &Row) -> SettingRow {
    let setting = row.setting;
    let mut item = SettingRow {
        key: setting.key.into(),
        heading: row.heading.unwrap_or_default().into(),
        title: setting.title.into(),
        description: setting.description.into(),
        input: row.input.as_str().into(),
        modified: row.modified,
        error: row.error.clone().unwrap_or_default().into(),
        ..SettingRow::default()
    };
    match &row.control {
        Control::Switch(on) => {
            item.kind = 0;
            item.on = *on;
        }
        Control::Choice { options, index } => {
            item.kind = 1;
            let options: Vec<SharedString> = options.iter().map(|&o| o.into()).collect();
            item.options = ModelRc::new(VecModel::from(options));
            item.choice = i32::try_from(*index).unwrap_or(0);
        }
        Control::Number { unit } => {
            item.kind = 2;
            item.unit = (*unit).into();
        }
        Control::Text { placeholder } => {
            item.kind = 3;
            item.placeholder = (*placeholder).into();
        }
        Control::Colour(colour) => {
            item.kind = 4;
            item.placeholder = "The theme's colour".into();
            if let Some(colour) = colour {
                item.has_colour = true;
                item.colour = slint::Color::from_rgb_u8(colour.red, colour.green, colour.blue);
            }
        }
        Control::List => item.kind = 5,
    }
    item
}
