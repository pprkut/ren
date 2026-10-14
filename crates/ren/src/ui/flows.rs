// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a reader does in the window, on Slint's testing backend: the main
//! window over a small database, clicked and typed at, without a display.
//! Articles are shown as plain text, so the flows are the same with and
//! without the `html-view` feature.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use i_slint_backend_testing::ElementHandle;
use i_slint_backend_testing::testing_backend::mock_elapsed_time;
use nextcloud_news::types;
use ren_settings::SettingsFile;
use ren_store::{MarkReadScope, Store};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalSize, Model, SharedString};

use super::{App, MainWindow, build_window};
use crate::cli::Options;
use crate::feed_tree::Node;
use crate::reader::Reader;
use crate::testing::TempDir;
use crate::window_state::Saved;

/// The settings of the window under test: an account, so the account page
/// doesn't open, no automatic syncs and no tray icon.
const SETTINGS: &str = "\
[account]
server = \"https://cloud.example.org\"
user = \"reader\"

[sync]
interval-minutes = 0
feed-icons = false

[window]
tray-icon = false
";

/// The testing backend, once per test thread (tests may also share one).
fn init_backend() {
    use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
    let backend = TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
    });
    let _ = slint::platform::set_platform(Box::new(backend));
}

fn item(
    id: u64,
    feed_id: u64,
    title: &str,
    pub_date: i64,
    unread: bool,
    starred: bool,
) -> types::Item {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "feedId": feed_id,
        "title": title,
        "author": "Ada",
        "url": format!("https://example.org/{id}"),
        "body": format!("<p>The body of {title}.</p>"),
        "pubDate": pub_date,
        "unread": unread,
        "starred": starred,
    }))
    .unwrap()
}

fn feed(id: u64, folder_id: Option<u64>, title: &str) -> types::Feed {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "url": format!("https://example.org/{id}.xml"),
        "title": title,
        "folderId": folder_id,
    }))
    .unwrap()
}

/// Folder "News" with feed Alpha (two unread items), and feed Beta
/// outside of it (an unread, starred item and a read one).
fn fill(store: &mut Store) {
    store
        .replace_folders(&[
            serde_json::from_value(serde_json::json!({"id": 1, "name": "News"})).unwrap(),
        ])
        .unwrap();
    store
        .replace_feeds(&[feed(10, Some(1), "Alpha"), feed(11, None, "Beta")])
        .unwrap();
    store
        .upsert_items(
            &[
                item(1, 10, "First", 100, true, false),
                item(2, 10, "Second", 200, true, false),
                item(3, 11, "Third", 300, true, true),
                item(4, 11, "Fourth", 400, false, false),
            ],
            false,
        )
        .unwrap();
}

/// The window over a new database, shown on the testing backend.
struct Ui {
    window: MainWindow,
    app: Rc<App>,
    settings: PathBuf,
    database: PathBuf,
    _dir: TempDir,
}

impl Ui {
    fn new() -> Self {
        Self::with_settings("")
    }

    /// With `more` settings after [`SETTINGS`].
    fn with_settings(more: &str) -> Self {
        Self::with_settings_file(&format!("{SETTINGS}{more}"))
    }

    /// With an account on `server`, whose password comes from a command.
    fn with_server(server: &str) -> Self {
        let settings = SETTINGS
            .replace("https://cloud.example.org", server)
            .replace(
                "user = \"reader\"",
                &format!(
                    "user = \"{}\"\npassword-command = \"echo {}\"",
                    crate::testing::USER,
                    crate::testing::PASSWORD
                ),
            );
        Self::with_settings_file(&settings)
    }

    fn with_settings_file(text: &str) -> Self {
        init_backend();
        let dir = TempDir::new("ui");
        let database = dir.join("ren.db");
        fill(&mut Store::open(&database).unwrap());
        let settings = dir.write("settings.toml", text);
        let reader = Reader::new(Store::open(&database).unwrap()).unwrap();
        let options = Options {
            plain_text: true,
            bundled_icons: true,
            ..Options::default()
        };
        let (window, app) = build_window(
            &options,
            Instant::now(),
            reader,
            Some(SettingsFile::open(settings.clone())),
            Some(database.clone()),
            (Saved::unsaved(), None),
        )
        .unwrap();
        window.window().set_size(LogicalSize::new(1200.0, 800.0));
        window.show().unwrap();
        // Lays out the window and makes the lists' rows.
        mock_elapsed_time(16);
        Self {
            window,
            app,
            settings,
            database,
            _dir: dir,
        }
    }

    /// Handles what the sync thread reports until the sync ended, as the
    /// event loop does when the thread wakes it.
    fn finish_sync(&self) {
        let started = Instant::now();
        while self.app.sync.borrow().running.is_some() {
            assert!(started.elapsed().as_secs() < 10, "the sync doesn't end");
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.app.sync_events();
        }
        mock_elapsed_time(16);
    }

    /// The elements of a component of the window, in order.
    fn elements(&self, type_name: &str) -> Vec<ElementHandle> {
        mock_elapsed_time(16);
        ElementHandle::find_by_element_type_name(&self.window, type_name).collect()
    }

    fn click(&self, type_name: &str, index: usize) {
        let elements = self.elements(type_name);
        let element = elements
            .get(index)
            .unwrap_or_else(|| panic!("{type_name} {index} of {}", elements.len()));
        element.mock_single_click(PointerEventButton::Left);
        mock_elapsed_time(16);
    }

    fn key(&self, text: impl Into<SharedString>) {
        let text = text.into();
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
        mock_elapsed_time(16);
    }

    /// The titles of the item list, in order.
    fn titles(&self) -> Vec<String> {
        let items = self.window.get_items();
        (0..items.row_count())
            .map(|row| items.row_data(row).unwrap().title.to_string())
            .collect()
    }

    /// The feed tree: title and count of each row.
    fn tree(&self) -> Vec<(String, i32)> {
        self.window
            .get_feeds()
            .iter()
            .map(|row| (row.title.to_string(), row.count))
            .collect()
    }

    fn article(&self) -> String {
        self.window.get_article_title().to_string()
    }

    fn store(&self) -> Store {
        Store::open(&self.database).unwrap()
    }
}

fn counts(ui: &Ui) -> Vec<i32> {
    ui.tree().into_iter().map(|(_, count)| count).collect()
}

#[test]
fn reading_articles() {
    let ui = Ui::new();
    assert_eq!(
        ui.tree(),
        [
            ("All items".to_owned(), 3),
            ("Starred".to_owned(), 1),
            ("News".to_owned(), 2),
            ("Alpha".to_owned(), 2),
            ("Beta".to_owned(), 1),
        ]
    );
    // Newest first.
    assert_eq!(ui.titles(), ["Fourth", "Third", "Second", "First"]);
    // The welcome page, until an article is shown.
    assert_eq!(ui.article(), "");
    assert!(ui.window.get_welcome());
    assert_eq!(
        ui.window.get_welcome_articles(),
        "3 unread articles in 2 feeds."
    );
    assert_eq!(ui.window.get_welcome_sync(), "Not synced yet.");

    // A click shows the article and marks it read.
    ui.click("ItemTableRow", 1);
    assert!(!ui.window.get_welcome());
    assert_eq!(ui.window.get_current_item(), 1);
    assert_eq!(ui.article(), "Third");
    assert!(ui.window.get_article_body().contains("The body of Third."));
    assert!(ui.window.get_current_starred());
    assert_eq!(counts(&ui), [2, 1, 2, 2, 0]);

    // j and k go down and up the list, s stars, m toggles read.
    ui.key("j");
    assert_eq!(ui.article(), "Second");
    assert!(!ui.window.get_current_starred());
    ui.key("s");
    assert!(ui.window.get_current_starred());
    assert_eq!(counts(&ui)[..2], [1, 2]);
    ui.key("m");
    assert_eq!(counts(&ui)[0], 2);
    ui.key("k");
    assert_eq!(ui.article(), "Third");
    ui.key(Key::DownArrow);
    ui.key(Key::DownArrow);
    assert_eq!(ui.article(), "First");

    // The changes are queued for the next sync.
    let store = ui.store();
    assert!(store.pending_count().unwrap() >= 3);
    let summaries = store.item_summaries(&[1, 2, 3]).unwrap();
    let state: Vec<(u64, bool)> = summaries.iter().map(|s| (s.id, s.starred)).collect();
    assert_eq!(state, [(1, false), (2, true), (3, true)]);
}

#[test]
fn feeds_search_and_sort() {
    // With the list above the article, which has a header.
    let ui = Ui::with_settings("\n[list]\narrangement = \"above\"\n");
    // The tree's rows: All Feeds, Starred, News, Alpha, Beta.
    ui.click("FeedTreeRow", 4);
    assert_eq!(ui.titles(), ["Fourth", "Third"]);
    ui.click("FeedTreeRow", 2);
    assert_eq!(ui.titles(), ["Second", "First"]);
    ui.click("FeedTreeRow", 1);
    assert_eq!(ui.titles(), ["Third"]);

    ui.click("FeedTreeRow", 0);
    ui.window.invoke_search_edited("ir".into());
    assert_eq!(ui.titles(), ["Third", "First"]);
    ui.window.invoke_search_edited("".into());

    // Sorted by title by a click on its header, then the other way.
    ui.click("HeaderCell", 0);
    assert_eq!(ui.titles(), ["First", "Fourth", "Second", "Third"]);
    ui.click("HeaderCell", 0);
    assert_eq!(ui.titles(), ["Third", "Second", "Fourth", "First"]);
    assert_eq!(
        (ui.window.get_sort_column(), ui.window.get_sort_ascending()),
        (0, false)
    );
}

#[test]
fn marking_all_read() {
    let ui = Ui::new();
    ui.click("FeedTreeRow", 3);
    ui.window.invoke_action("mark-feed-read".into());
    assert_eq!(counts(&ui), [1, 1, 0, 0, 1]);
    assert_eq!(ui.window.get_status_text(), "Marked 2 articles as read");

    ui.window.invoke_action("mark-all-read".into());
    assert_eq!(counts(&ui), [0, 1, 0, 0, 0]);
    let pending = ui.store().pending_mark_read().unwrap();
    let scopes: Vec<MarkReadScope> = pending.iter().map(|p| p.scope).collect();
    assert_eq!(scopes, [MarkReadScope::Feed(10), MarkReadScope::All]);

    // The status message goes after a while.
    mock_elapsed_time(super::STATUS_TIMEOUT.as_millis() as u64 + 100);
    assert_eq!(ui.window.get_status_text(), "");
}

#[test]
fn settings_apply_at_once() {
    let ui = Ui::new();
    ui.window.invoke_action("configure".into());
    let dialog = match &*ui.app.dialog.borrow() {
        Some(dialog) => dialog.window.clone_strong(),
        None => panic!("the settings dialog isn't open"),
    };
    dialog.invoke_search_edited("arrangement".into());
    mock_elapsed_time(16);
    // The second picture: the list above the article.
    let pictures: Vec<ElementHandle> =
        ElementHandle::find_by_element_type_name(&dialog, "ArrangementPicture").collect();
    assert_eq!(pictures.len(), 2);
    pictures[1].mock_single_click(PointerEventButton::Left);
    mock_elapsed_time(16);
    let file = std::fs::read_to_string(&ui.settings).unwrap();
    assert!(file.contains("arrangement = \"above\""), "{file}");
    assert_eq!(ui.window.get_arrangement(), super::Arrangement::Above);
}

#[test]
fn typing_in_the_search_and_the_tree() {
    let ui = Ui::new();
    let search = ElementHandle::find_by_element_id(&ui.window, "MainWindow::search")
        .next()
        .expect("no search field");
    search.mock_single_click(PointerEventButton::Left);
    for key in ["t", "h"] {
        ui.key(key);
    }
    assert_eq!(ui.titles(), ["Fourth", "Third"]);
    ui.key("i");
    assert_eq!(ui.titles(), ["Third"]);
    for _ in 0..3 {
        ui.key(Key::Backspace);
    }
    assert_eq!(ui.titles().len(), 4);

    // From the tree with the keyboard: down to Starred, then to the folder,
    // which closes with the left arrow.
    ui.click("FeedTreeRow", 0);
    ui.key(Key::DownArrow);
    assert_eq!(ui.titles(), ["Third"]);
    ui.key(Key::DownArrow);
    assert_eq!(ui.titles(), ["Second", "First"]);
    ui.key(Key::LeftArrow);
    let titles: Vec<String> = ui.tree().into_iter().map(|(title, _)| title).collect();
    assert_eq!(titles, ["All items", "Starred", "News", "Beta"]);
    ui.key(Key::RightArrow);
    assert_eq!(ui.tree().len(), 5);
}

#[test]
fn invalid_settings_and_reset() {
    let ui = Ui::new();
    ui.window.invoke_action("configure".into());
    let dialog = match &*ui.app.dialog.borrow() {
        Some(dialog) => dialog.window.clone_strong(),
        None => panic!("the settings dialog isn't open"),
    };
    let row = |key: &str| {
        dialog
            .get_rows()
            .iter()
            .find(|row| row.key == key)
            .unwrap_or_else(|| panic!("no row {key}"))
    };
    dialog.invoke_search_edited("interval".into());
    let key = ren_settings::schema::SYNC_INTERVAL;
    // A value out of range stays in the field with its error; the file
    // is left alone.
    dialog.invoke_input_accepted(key.into(), "100000".into());
    assert!(!row(key).error.is_empty());
    assert!(
        std::fs::read_to_string(&ui.settings)
            .unwrap()
            .contains("interval-minutes = 0")
    );
    dialog.invoke_input_accepted(key.into(), "30".into());
    assert_eq!(row(key).error, "");
    assert!(row(key).modified);
    assert!(
        std::fs::read_to_string(&ui.settings)
            .unwrap()
            .contains("interval-minutes = 30")
    );
    // Reset to Default removes it from the file.
    dialog.invoke_reset(key.into());
    assert!(
        !std::fs::read_to_string(&ui.settings)
            .unwrap()
            .contains("interval-minutes")
    );
    assert!(!row(key).modified);
    assert_eq!(row(key).input, "15");

    // Closing the dialog drops it.
    dialog.window().dispatch_event(WindowEvent::CloseRequested);
    mock_elapsed_time(16);
    assert!(ui.app.dialog.borrow().is_none());
}

#[test]
fn about_ren() {
    let ui = Ui::new();
    ui.window.invoke_action("about".into());
    let about = ui
        .app
        .about
        .borrow()
        .as_ref()
        .map(ComponentHandle::clone_strong);
    let about = about.expect("About Ren isn't open");
    assert!(about.get_version().contains(env!("CARGO_PKG_VERSION")));
    ui.window.invoke_action("about-close".into());
    mock_elapsed_time(16);
    assert!(ui.app.about.borrow().is_none());
}

#[test]
fn adding_and_deleting_feeds() {
    let server = crate::testing::recorded_server();
    let ui = Ui::with_server(&server.url());
    // Add Feed in the folder selected.
    ui.click("FeedTreeRow", 2);
    ui.window.invoke_action("add-feed".into());
    let dialog = match &*ui.app.feed_dialog.borrow() {
        Some(dialog) => dialog.window.clone_strong(),
        None => panic!("the feed dialog isn't open"),
    };
    assert_eq!(dialog.get_folder_index(), 1);
    // Without an address, nothing is sent.
    dialog.invoke_accepted();
    assert!(!dialog.get_error().is_empty());
    assert!(ui.app.sync.borrow().running.is_none());
    dialog.set_url("https://example.org/new.xml".into());
    dialog.invoke_accepted();
    assert!(dialog.get_busy());
    ui.finish_sync();
    // The dialog closed, and the sync brought the server's feeds; the one
    // the server answered with is shown.
    assert!(ui.app.feed_dialog.borrow().is_none());
    let requests: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.api_path().unwrap_or_default()))
        .collect();
    assert_eq!(requests[0], "POST feeds");
    let added: serde_json::Value =
        serde_json::from_slice(&crate::testing::recorded("feeds")).unwrap();
    let added = added["feeds"][0]["id"].as_u64().unwrap();
    assert_eq!(ui.app.reader.borrow().selected(), Node::Feed(added));
    let request = &server.requests()[0];
    assert_eq!(
        String::from_utf8_lossy(&request.body),
        r#"{"url":"https://example.org/new.xml","folderId":1}"#
    );

    // Deleting it asks first.
    let row = ui
        .app
        .reader
        .borrow()
        .tree_rows()
        .iter()
        .position(|row| row.node == Node::Feed(added))
        .unwrap();
    ui.app.feed_menu(row, "delete-feed");
    let confirm = match &*ui.app.confirm.borrow() {
        Some(confirm) => confirm.clone_strong(),
        None => panic!("no question before deleting"),
    };
    confirm.invoke_confirmed();
    ui.finish_sync();
    let deleted = format!("feeds/{added}");
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.method == "DELETE" && r.api_path() == Some(deleted.as_str()))
    );
}
