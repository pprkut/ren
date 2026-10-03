// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Response types of the News API v1-3.
//!
//! Unknown fields are ignored and most fields are optional, since they were
//! added in different versions of the News app. Strings marked as not
//! sanitised by the API (titles, authors, URLs, enclosure and media fields)
//! must not be rendered as HTML.

use serde::{Deserialize, Deserializer};

/// `GET /version`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Version {
    pub version: String,
}

/// `GET /status`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub version: String,
    #[serde(default)]
    pub warnings: Warnings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Warnings {
    /// Feeds are only updated while the web interface is open.
    #[serde(default)]
    pub improperly_configured_cron: bool,
    #[serde(default)]
    pub incorrect_db_charset: bool,
}

/// `GET /folders`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Folders {
    pub folders: Vec<Folder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Folder {
    pub id: u64,
    pub name: String,
}

/// `GET /feeds`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feeds {
    pub feeds: Vec<Feed>,
    #[serde(default)]
    pub starred_count: u64,
    /// Only sent if there are items.
    pub newest_item_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub id: u64,
    /// The feed URL.
    pub url: String,
    /// Not sanitised.
    pub title: Option<String>,
    pub favicon_link: Option<String>,
    /// Unix seconds.
    pub added: Option<i64>,
    /// `None` (or 0 in older versions) for feeds outside of folders.
    pub folder_id: Option<u64>,
    #[serde(default)]
    pub unread_count: u64,
    /// 0: default, 1: oldest first, 2: newest first.
    #[serde(default)]
    pub ordering: u8,
    /// The web site. Not sanitised.
    pub link: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub update_error_count: u64,
    pub last_update_error: Option<String>,
}

/// `GET /items`, `GET /items/updated`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Items {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: u64,
    pub guid: Option<String>,
    pub guid_hash: Option<String>,
    /// Not sanitised.
    pub url: Option<String>,
    /// Not sanitised.
    pub title: Option<String>,
    /// Not sanitised.
    pub author: Option<String>,
    /// Unix seconds.
    pub pub_date: Option<i64>,
    /// Unix seconds.
    #[serde(default, deserialize_with = "seconds")]
    pub updated_date: Option<i64>,
    /// Sanitised HTML.
    pub body: Option<String>,
    pub enclosure_mime: Option<String>,
    pub enclosure_link: Option<String>,
    pub media_thumbnail: Option<String>,
    pub media_description: Option<String>,
    pub feed_id: u64,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub rtl: bool,
    /// Unix seconds. Documented as a string, sent as a number.
    #[serde(default, deserialize_with = "seconds")]
    pub last_modified: Option<i64>,
    /// Hash over title, enclosures, body and URL; the same fingerprint
    /// means the same article in several feeds.
    pub fingerprint: Option<String>,
    pub content_hash: Option<String>,
}

/// A timestamp sent as a number or as a string of digits.
fn seconds<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Str(String),
    }
    match Option::<Raw>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Raw::Int(n)) => Ok(Some(n)),
        Some(Raw::Str(s)) if s.is_empty() => Ok(None),
        Some(Raw::Str(s)) => s
            .parse()
            .map(Some)
            .map_err(|_| serde::de::Error::custom(format!("invalid timestamp: {s:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_item() {
        let item: Item = serde_json::from_str(r#"{"id": 3, "feedId": 7}"#).unwrap();
        assert_eq!(item.id, 3);
        assert_eq!(item.feed_id, 7);
        assert_eq!(item.title, None);
        assert_eq!(item.last_modified, None);
        assert!(!item.unread);
    }

    #[test]
    fn full_item_with_unknown_fields() {
        let json = r#"{
            "id": 3443, "guid": "http://example.org/?p=76",
            "guidHash": "3059047a572cd9cd5d0bf645faffd077",
            "url": "http://example.org/2013/04/29/post/", "title": "A post",
            "author": "Someone", "pubDate": 1367270544, "updatedDate": null,
            "body": "<p>Text</p>", "enclosureMime": null, "enclosureLink": null,
            "mediaThumbnail": null, "mediaDescription": null, "feedId": 67,
            "unread": true, "starred": false, "filtered": false, "rtl": false,
            "lastModified": 1367273003, "fingerprint": "aeaae2123",
            "contentHash": "abc", "somethingNew": [1, 2]
        }"#;
        let item: Item = serde_json::from_str(json).unwrap();
        assert_eq!(item.pub_date, Some(1_367_270_544));
        assert_eq!(item.last_modified, Some(1_367_273_003));
        assert_eq!(item.body.as_deref(), Some("<p>Text</p>"));
        assert!(item.unread);
    }

    #[test]
    fn timestamps_as_strings() {
        let item: Item = serde_json::from_str(
            r#"{"id": 1, "feedId": 1, "lastModified": "1367273003", "updatedDate": ""}"#,
        )
        .unwrap();
        assert_eq!(item.last_modified, Some(1_367_273_003));
        assert_eq!(item.updated_date, None);
        assert!(
            serde_json::from_str::<Item>(r#"{"id": 1, "feedId": 1, "lastModified": "x"}"#).is_err()
        );
    }

    #[test]
    fn feeds() {
        let json = r#"{
            "feeds": [
                {"id": 39, "url": "http://example.org/feed", "title": "Example",
                 "faviconLink": null, "added": 1367063790, "nextUpdateTime": 2071387335,
                 "folderId": 4, "unreadCount": 9, "ordering": 0,
                 "link": "http://example.org/", "pinned": true,
                 "updateErrorCount": 0, "lastUpdateError": null},
                {"id": 40, "url": "http://example.com/feed", "folderId": null}
            ],
            "starredCount": 2
        }"#;
        let feeds: Feeds = serde_json::from_str(json).unwrap();
        assert_eq!(feeds.feeds.len(), 2);
        assert_eq!(feeds.feeds[0].folder_id, Some(4));
        assert_eq!(feeds.feeds[0].unread_count, 9);
        assert_eq!(feeds.feeds[1].folder_id, None);
        assert_eq!(feeds.starred_count, 2);
        assert_eq!(feeds.newest_item_id, None);
    }

    #[test]
    fn status() {
        let status: Status = serde_json::from_str(
            r#"{"version": "25.0.0", "warnings": {"improperlyConfiguredCron": true}}"#,
        )
        .unwrap();
        assert!(status.warnings.improperly_configured_cron);
        assert!(!status.warnings.incorrect_db_charset);
    }
}
