// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adding, editing and deleting feeds and folders: the feed dialog and the
//! questions before deleting. The changes go to the server on the sync
//! thread, before a sync that brings them in (`ren_sync::change`).

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use ren_store::{Feed, Folder};
use ren_sync::{Changed, FeedChange};

use super::{App, ConfirmWindow, FeedDialog, FeedDialogKind};
use crate::feed_edit::{self, URL_NOTE};
use crate::feed_tree::Node;

/// What the open feed dialog edits.
enum Target {
    AddFeed,
    EditFeed(Feed),
    AddFolder,
    RenameFolder(Folder),
}

/// The open feed dialog.
pub(super) struct Dialog {
    pub(super) window: FeedDialog,
    target: Target,
    /// The folders in its list, after "No folder".
    folders: Vec<Folder>,
    /// The changes sent, until the sync thread reports them.
    pending: Vec<FeedChange>,
    /// What the server refused of them.
    errors: Vec<String>,
}

impl App {
    /// Feed → Add Feed: in the folder selected, or the selected feed's.
    pub(super) fn add_feed(&self) {
        let folder = match self.reader.borrow().selected() {
            Node::Folder(id) => Some(id),
            Node::Feed(id) => self
                .with_reader(|reader| reader.feed(id))
                .flatten()
                .and_then(|feed| feed.folder_id),
            _ => None,
        };
        self.open_feed_dialog(Target::AddFeed, folder);
    }

    /// An entry of a feed's or folder's context menu that changes them.
    /// Returns `false` for other entries.
    pub(super) fn manage_feeds(&self, node: Node, action: &str) -> bool {
        match (action, node) {
            ("edit-feed", Node::Feed(id)) => {
                if let Some(Some(feed)) = self.with_reader(|reader| reader.feed(id)) {
                    let folder = feed.folder_id;
                    self.open_feed_dialog(Target::EditFeed(feed), folder);
                }
            }
            ("delete-feed", Node::Feed(id)) => self.ask_delete_feed(id),
            ("open-homepage", Node::Feed(id)) => {
                match self.with_reader(|reader| reader.feed(id)).flatten() {
                    Some(Feed {
                        link: Some(link), ..
                    }) => self.open_in_browser(&link),
                    Some(_) => self.status("This feed names no homepage"),
                    None => {}
                }
            }
            ("new-folder", _) => self.open_feed_dialog(Target::AddFolder, None),
            ("rename-folder", Node::Folder(id)) => {
                let folders = self
                    .with_reader(|reader| reader.folders())
                    .unwrap_or_default();
                if let Some(folder) = folders.into_iter().find(|folder| folder.id == id) {
                    self.open_feed_dialog(Target::RenameFolder(folder), None);
                }
            }
            ("delete-folder", Node::Folder(id)) => self.ask_delete_folder(id),
            _ => return false,
        }
        true
    }

    fn open_feed_dialog(&self, target: Target, folder: Option<u64>) {
        self.close_feed_dialog();
        let window = match FeedDialog::new() {
            Ok(window) => window,
            Err(err) => {
                self.status(format!("Opening the dialog failed: {err}"));
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
        let folders = self
            .with_reader(|reader| reader.folders())
            .unwrap_or_default();
        let mut names: Vec<SharedString> = vec!["No folder".into()];
        names.extend(folders.iter().map(|folder| folder.name.as_str().into()));
        window.set_folders(ModelRc::new(VecModel::from(names)));
        window.set_folder_index(feed_edit::folder_index(&folders, folder));
        let (kind, title, heading, label) = match &target {
            Target::AddFeed => (
                FeedDialogKind::AddFeed,
                "Add Feed",
                "Add a feed".to_owned(),
                "Add",
            ),
            Target::EditFeed(feed) => {
                window.set_name(feed.title.as_str().into());
                window.set_url(feed.url.as_str().into());
                window.set_note(URL_NOTE.into());
                (
                    FeedDialogKind::EditFeed,
                    "Edit Feed",
                    format!("Edit “{}”", display_title(feed)),
                    "Save",
                )
            }
            Target::AddFolder => (
                FeedDialogKind::Folder,
                "New Folder",
                "Add a folder".to_owned(),
                "Add",
            ),
            Target::RenameFolder(folder) => {
                window.set_name(folder.name.as_str().into());
                (
                    FeedDialogKind::Folder,
                    "Rename Folder",
                    format!("Rename “{}”", folder.name),
                    "Rename",
                )
            }
        };
        window.set_kind(kind);
        window.set_window_title(format!("{title} – Ren").into());
        window.set_heading(heading.into());
        window.set_confirm_label(label.into());
        let this = self.this.clone();
        window.on_accepted(move || {
            if let Some(app) = this.upgrade() {
                app.feed_dialog_accepted();
            }
        });
        let this = self.this.clone();
        window.on_cancelled(move || {
            if let Some(app) = this.upgrade() {
                app.close_feed_dialog_later();
            }
        });
        let this = self.this.clone();
        window.window().on_close_requested(move || {
            if let Some(app) = this.upgrade() {
                app.close_feed_dialog_later();
            }
            slint::CloseRequestResponse::HideWindow
        });
        if let Err(err) = window.show() {
            eprintln!("ren: showing the feed dialog: {err}");
        }
        window.invoke_focus_field();
        *self.feed_dialog.borrow_mut() = Some(Dialog {
            window,
            target,
            folders,
            pending: Vec::new(),
            errors: Vec::new(),
        });
    }

    /// The dialog's changes go to the server; it stays open until they are
    /// made, and shows what the server refused.
    fn feed_dialog_accepted(&self) {
        let changes = {
            let mut dialog = self.feed_dialog.borrow_mut();
            let Some(dialog) = dialog.as_mut() else {
                return;
            };
            let window = &dialog.window;
            let name = window.get_name().trim().to_owned();
            let url = window.get_url().trim().to_owned();
            let folder_id = feed_edit::folder_at(&dialog.folders, window.get_folder_index());
            let changes = match &dialog.target {
                Target::AddFeed if url.is_empty() => {
                    window.set_error("Enter the address of a feed or of a site.".into());
                    return;
                }
                Target::AddFeed => vec![FeedChange::AddFeed { url, folder_id }],
                Target::EditFeed(feed) => feed_edit::feed_changes(feed, &name, &url, folder_id),
                Target::AddFolder | Target::RenameFolder(_) if name.is_empty() => {
                    window.set_error("Enter a name.".into());
                    return;
                }
                Target::AddFolder => vec![FeedChange::AddFolder { name }],
                Target::RenameFolder(folder) if folder.name == name => Vec::new(),
                Target::RenameFolder(folder) => vec![FeedChange::RenameFolder {
                    folder_id: folder.id,
                    name,
                }],
            };
            if !changes.is_empty() {
                window.set_error(SharedString::new());
                window.set_busy(true);
                dialog.pending = changes.clone();
                dialog.errors.clear();
            }
            changes
        };
        if changes.is_empty() {
            self.close_feed_dialog_later();
            return;
        }
        if let Err(err) = self.change_feeds(changes) {
            self.feed_dialog_done(Some(err));
        }
    }

    /// Sends changes with the next sync, now if none runs.
    fn change_feeds(&self, changes: Vec<FeedChange>) -> Result<(), String> {
        if self.database.is_none() {
            return Err("Demo data isn't synced, so its feeds can't change.".to_owned());
        }
        if self.current_settings().account().is_err() {
            return Err("Log in to your Nextcloud first.".to_owned());
        }
        self.feed_changes.borrow_mut().extend(changes);
        if self.sync.borrow().running.is_some() {
            self.status_until_next("The changes are sent after the running sync");
            return Ok(());
        }
        self.start_sync();
        if self.sync.borrow().running.is_none() {
            self.feed_changes.borrow_mut().clear();
            return Err(format!(
                "The sync didn't start: {}",
                self.window().get_status_text()
            ));
        }
        Ok(())
    }

    /// The sync thread sent a change, or the server refused it.
    pub(super) fn feed_changed(
        &self,
        change: FeedChange,
        result: Result<Changed, ren_sync::Error>,
    ) {
        let (text, failed) = feed_edit::outcome(&change, &result);
        if failed {
            self.refused_changes.borrow_mut().push(text.clone());
            self.status_until_next(text.clone());
        } else {
            self.status(text.clone());
        }
        if let Ok(changed) = &result {
            match &change {
                FeedChange::AddFeed { .. } => {
                    self.select_after_sync.set(changed.feed.map(Node::Feed));
                }
                FeedChange::ChangeUrl { feed_id, .. } => {
                    self.select_after_sync.set(changed.feed.map(Node::Feed));
                    // The feed's own setting goes to the new one.
                    if self.reader.borrow().is_full_page(*feed_id) {
                        self.full_page_after_sync.set(changed.feed);
                    }
                }
                _ => {}
            }
        }
        let done = {
            let mut dialog = self.feed_dialog.borrow_mut();
            let Some(dialog) = dialog.as_mut() else {
                return;
            };
            let Some(index) = dialog.pending.iter().position(|pending| *pending == change) else {
                return;
            };
            dialog.pending.remove(index);
            if failed {
                dialog.errors.push(text);
            }
            dialog.pending.is_empty()
        };
        if done {
            self.feed_dialog_done(None);
        }
    }

    /// The dialog's changes are made: it closes, unless the server
    /// refused one (or `failure` says why none was sent), which it shows.
    fn feed_dialog_done(&self, failure: Option<String>) {
        let errors = {
            let mut dialog = self.feed_dialog.borrow_mut();
            let Some(dialog) = dialog.as_mut() else {
                return;
            };
            dialog.pending.clear();
            dialog.errors.extend(failure);
            std::mem::take(&mut dialog.errors)
        };
        if errors.is_empty() {
            self.close_feed_dialog_later();
            return;
        }
        if let Some(dialog) = &*self.feed_dialog.borrow() {
            dialog.window.set_busy(false);
            dialog.window.set_error(errors.join("\n").into());
        }
    }

    /// After a sync: changes that came after it started, the feed added
    /// shown, and a dialog whose changes the sync didn't send told why.
    pub(super) fn feeds_after_sync(&self, failure: Option<String>) {
        if let Some(id) = self.full_page_after_sync.take() {
            self.with_reader(|reader| reader.set_full_page(id, true));
            self.reload_icons();
        }
        if let Some(node) = self.select_after_sync.take()
            && self
                .reader
                .borrow()
                .tree_rows()
                .iter()
                .any(|row| row.node == node)
        {
            self.select_node(node);
        }
        let unsent = self
            .feed_dialog
            .borrow()
            .as_ref()
            .is_some_and(|dialog| !dialog.pending.is_empty());
        if unsent && self.feed_changes.borrow().is_empty() {
            self.feed_dialog_done(Some(
                failure.unwrap_or_else(|| "The sync ended before the changes were sent.".into()),
            ));
        }
        if !self.feed_changes.borrow().is_empty() {
            self.start_sync();
        }
    }

    fn ask_delete_feed(&self, id: u64) {
        let Some(Some(feed)) = self.with_reader(|reader| reader.feed(id)) else {
            return;
        };
        self.ask(
            "Delete Feed",
            &format!("Delete “{}”?", display_title(&feed)),
            "The feed is deleted on the server, with all its articles, starred ones \
             included. This can't be undone.",
            "Delete",
            move |app| {
                if let Err(err) = app.change_feeds(vec![FeedChange::DeleteFeed { feed_id: id }]) {
                    app.status(err);
                }
            },
        );
    }

    fn ask_delete_folder(&self, id: u64) {
        let Some(folder) = self
            .with_reader(|reader| reader.folders())
            .and_then(|folders| folders.into_iter().find(|folder| folder.id == id))
        else {
            return;
        };
        let feeds: Vec<u64> = self
            .with_reader(|reader| reader.feeds_in(id))
            .unwrap_or_default();
        let text = match feeds.len() {
            0 => "The folder is deleted on the server.".to_owned(),
            1 => "The folder and its feed are deleted on the server, with all its articles, \
                  starred ones included. This can't be undone."
                .to_owned(),
            n => format!(
                "The folder and its {n} feeds are deleted on the server, with all their \
                 articles, starred ones included. This can't be undone."
            ),
        };
        self.ask(
            "Delete Folder",
            &format!("Delete “{}”?", folder.name),
            &text,
            "Delete",
            move |app| {
                let change = FeedChange::DeleteFolder {
                    folder_id: id,
                    feeds: feeds.clone(),
                };
                if let Err(err) = app.change_feeds(vec![change]) {
                    app.status(err);
                }
            },
        );
    }

    /// Asks before something that can't be undone, in a window of its own.
    fn ask(
        &self,
        title: &str,
        heading: &str,
        text: &str,
        label: &str,
        confirmed: impl Fn(&App) + 'static,
    ) {
        let confirm = match ConfirmWindow::new() {
            Ok(confirm) => confirm,
            Err(err) => {
                self.status(format!("{title} failed: {err}"));
                return;
            }
        };
        if let Some(dark) = self.forced_dark {
            confirm.set_forced_color_scheme(if dark {
                slint::language::ColorScheme::Dark
            } else {
                slint::language::ColorScheme::Light
            });
        }
        confirm.set_window_title(format!("{title} – Ren").into());
        confirm.set_heading(heading.into());
        confirm.set_text(text.into());
        confirm.set_confirm_label(label.into());
        let this = self.this.clone();
        confirm.on_confirmed(move || {
            if let Some(app) = this.upgrade() {
                app.close_confirm();
                confirmed(&app);
            }
        });
        let this = self.this.clone();
        confirm.on_cancelled(move || {
            if let Some(app) = this.upgrade() {
                app.close_confirm();
            }
        });
        let this = self.this.clone();
        confirm.window().on_close_requested(move || {
            if let Some(app) = this.upgrade() {
                app.close_confirm();
            }
            slint::CloseRequestResponse::HideWindow
        });
        if let Err(err) = confirm.show() {
            eprintln!("ren: showing the question: {err}");
        }
        *self.confirm.borrow_mut() = Some(confirm);
    }

    fn close_feed_dialog(&self) {
        if let Some(dialog) = self.feed_dialog.borrow_mut().take() {
            let _ = dialog.window.hide();
        }
    }

    /// Hides the dialog, and drops it: not from its own callback.
    fn close_feed_dialog_later(&self) {
        if let Some(dialog) = &*self.feed_dialog.borrow() {
            let _ = dialog.window.hide();
        }
        let this = self.this.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(app) = this.upgrade() {
                app.feed_dialog.borrow_mut().take();
            }
        });
    }
}

/// A feed's title, or its address if it has none.
fn display_title(feed: &Feed) -> &str {
    if feed.title.is_empty() {
        &feed.url
    } else {
        &feed.title
    }
}
