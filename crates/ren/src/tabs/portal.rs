// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files for a page's file input (an upload), chosen in the desktop's own
//! file chooser through the XDG desktop portal
//! (`org.freedesktop.portal.FileChooser`, which Plasma and GNOME provide),
//! on a thread of its own: the chooser stays open as long as the reader
//! likes. zbus is in the build already (Slint's tray, the Secret Service).

use std::collections::HashMap;
use std::path::PathBuf;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

/// What the chooser gave: the files chosen, or `None` if it was
/// cancelled.
pub type Chosen = Result<Option<Vec<PathBuf>>, String>;

/// Opens the desktop's file chooser on a thread; `done` gets the files.
pub fn choose_files(
    title: String,
    multiple: bool,
    extensions: Vec<String>,
    done: impl FnOnce(Chosen) + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("ren-file-chooser".to_owned())
        .spawn(move || done(open_file(&title, multiple, &extensions)));
    if let Err(err) = spawned {
        eprintln!("ren: the file chooser's thread: {err}");
    }
}

fn open_file(title: &str, multiple: bool, extensions: &[String]) -> Chosen {
    let failed = |err: zbus::Error| format!("the desktop's file chooser (its portal): {err}");
    let connection = Connection::session().map_err(failed)?;
    // The portal answers on a request object whose path follows from the
    // connection's name and a token; listen there before asking, so the
    // answer can't come first.
    let sender = connection
        .unique_name()
        .ok_or("no name on the session bus")?
        .trim_start_matches(':')
        .replace('.', "_");
    let token = format!("ren{}", std::process::id());
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let request = Proxy::new(
        &connection,
        PORTAL,
        path.as_str(),
        "org.freedesktop.portal.Request",
    )
    .map_err(failed)?;
    let mut responses = request.receive_signal("Response").map_err(failed)?;
    let chooser = Proxy::new(
        &connection,
        PORTAL,
        PORTAL_PATH,
        "org.freedesktop.portal.FileChooser",
    )
    .map_err(failed)?;
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", token.as_str().into());
    options.insert("multiple", multiple.into());
    if let Some(filters) = filters(extensions) {
        options.insert("filters", filters.into());
    }
    let _: zbus::zvariant::OwnedObjectPath = chooser
        .call("OpenFile", &("", title, options))
        .map_err(failed)?;
    let answer = responses
        .next()
        .ok_or("the file chooser closed without an answer")?;
    let (response, results): (u32, HashMap<String, OwnedValue>) =
        answer.body().deserialize().map_err(failed)?;
    match response {
        0 => {
            let uris: Vec<String> = results
                .get("uris")
                .and_then(|uris| Vec::<String>::try_from(uris.try_clone().ok()?).ok())
                .unwrap_or_default();
            Ok(Some(uris.iter().filter_map(|uri| file_path(uri)).collect()))
        }
        1 => Ok(None),
        _ => Err("the file chooser failed".to_owned()),
    }
}

/// A filter of the file chooser: its name and its patterns, each a kind
/// (0: a glob) and the pattern.
type Filter = (String, Vec<(u32, String)>);

/// The chooser's filters for a page's extensions: one for them (in lower
/// and upper case: patterns are case-sensitive), and all files.
fn filters(extensions: &[String]) -> Option<Vec<Filter>> {
    if extensions.is_empty() {
        return None;
    }
    let patterns = extensions
        .iter()
        .flat_map(|extension| {
            [
                (0, format!("*.{}", extension.to_lowercase())),
                (0, format!("*.{}", extension.to_uppercase())),
            ]
        })
        .collect();
    Some(vec![
        ("Files the page accepts".to_owned(), patterns),
        ("All files".to_owned(), vec![(0, "*".to_owned())]),
    ])
}

/// The path of a local `file://` URI, percent-decoded; `None` for other
/// URIs.
fn file_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    // A host before the path (file://host/…) isn't local.
    if !path.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(path.len());
    let mut rest = path.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%'
            && let Some(hex) = tail.get(..2)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(decoded) = u8::from_str_radix(hex, 16)
        {
            bytes.push(decoded);
            rest = &tail[2..];
            continue;
        }
        bytes.push(byte);
        rest = tail;
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris() {
        assert_eq!(
            file_path("file:///home/u/a%20b%C3%A4.pdf"),
            Some(PathBuf::from("/home/u/a bä.pdf"))
        );
        assert_eq!(
            file_path("file:///tmp/100%"),
            Some(PathBuf::from("/tmp/100%"))
        );
        assert_eq!(file_path("file://host/share/x"), None);
        assert_eq!(file_path("https://example.org/x"), None);
    }

    #[test]
    fn chooser_filters() {
        assert_eq!(filters(&[]), None);
        let filters = filters(&["pdf".to_owned()]).unwrap();
        assert_eq!(
            filters[0].1,
            [(0, "*.pdf".to_owned()), (0, "*.PDF".to_owned())]
        );
        assert_eq!(filters[1].1, [(0, "*".to_owned())]);
    }
}
