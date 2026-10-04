// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding response bodies while they are received.
//!
//! Item lists are decoded one item at a time and handed to a callback, so
//! memory stays flat however large a response is. This matters most for
//! the unpaged `GET /items/updated`: decoded at once, a response takes
//! about three times its JSON size in memory (S2).

use std::fmt;
use std::io::{BufReader, Read};

use serde::de::{self, DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess};
use serde::{Deserialize, Deserializer};

use crate::error::Error;
use crate::types::Item;

/// Decodes a JSON response body.
pub fn decode<T: DeserializeOwned>(reader: impl Read) -> Result<T, Error> {
    serde_json::from_reader(BufReader::new(reader)).map_err(Error::from_decode)
}

/// Decodes an item list (`{"items": [...]}`) and calls `f` with each item
/// as soon as it is complete. Returns the number of items. An error from
/// `f` stops decoding and is returned as it is.
pub fn decode_items<E: From<Error>>(
    reader: impl Read,
    mut f: impl FnMut(Item) -> Result<(), E>,
) -> Result<usize, E> {
    let mut failure = None;
    let mut sink = |item| match f(item) {
        Ok(()) => true,
        Err(err) => {
            failure = Some(err);
            false
        }
    };
    let mut de = serde_json::Deserializer::from_reader(BufReader::new(reader));
    let result = de
        .deserialize_map(ItemsVisitor { sink: &mut sink })
        .and_then(|count| de.end().map(|()| count));
    match (result, failure) {
        (_, Some(err)) => Err(err),
        (Ok(count), None) => Ok(count),
        (Err(err), None) => Err(Error::from_decode(err).into()),
    }
}

type Sink<'a> = &'a mut dyn FnMut(Item) -> bool;

/// The top level object: the `items` array, other keys ignored.
struct ItemsVisitor<'a> {
    sink: Sink<'a>,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum Key {
    Items,
    /// The News app answers some invalid requests with HTTP 200 and
    /// `{"message": "..."}` instead of items.
    Message,
    #[serde(other)]
    Other,
}

impl<'de> de::Visitor<'de> for ItemsVisitor<'_> {
    type Value = usize;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an object with an items array")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<usize, A::Error> {
        let mut count = None;
        let mut message = None;
        while let Some(key) = map.next_key()? {
            match key {
                Key::Items if count.is_some() => return Err(de::Error::duplicate_field("items")),
                Key::Items => {
                    count = Some(map.next_value_seed(ItemSeq {
                        sink: &mut *self.sink,
                    })?);
                }
                Key::Message => message = map.next_value::<Option<String>>()?,
                Key::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        match (count, message) {
            (Some(count), _) => Ok(count),
            (None, Some(message)) => Err(de::Error::custom(format_args!(
                "no items, the server says: {message}"
            ))),
            (None, None) => Err(de::Error::missing_field("items")),
        }
    }
}

/// The `items` array, handed over item by item.
struct ItemSeq<'a> {
    sink: Sink<'a>,
}

impl<'de> DeserializeSeed<'de> for ItemSeq<'_> {
    type Value = usize;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<usize, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> de::Visitor<'de> for ItemSeq<'_> {
    type Value = usize;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array of items")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<usize, A::Error> {
        let mut count = 0;
        while let Some(item) = seq.next_element::<Item>()? {
            if !(self.sink)(item) {
                return Err(de::Error::custom("stopped by the caller"));
            }
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;

    use super::*;

    fn ids(json: &str) -> Result<Vec<u64>, Error> {
        let mut ids = Vec::new();
        decode_items(json.as_bytes(), |item| {
            ids.push(item.id);
            Ok::<_, Error>(())
        })?;
        Ok(ids)
    }

    #[test]
    fn items_in_order() {
        let json = r#"{"items": [{"id": 3, "feedId": 1}, {"id": 2, "feedId": 1}]}"#;
        assert_eq!(ids(json).unwrap(), [3, 2]);
        assert_eq!(ids(r#"{"items": []}"#).unwrap(), [] as [u64; 0]);
    }

    #[test]
    fn other_keys_are_ignored() {
        let json = r#"{"before": {"items": [1]}, "items": [{"id": 1, "feedId": 1}],
                       "after": [null, {"x": "y"}]}"#;
        assert_eq!(ids(json).unwrap(), [1]);
    }

    #[test]
    fn count() {
        let json = r#"{"items": [{"id": 3, "feedId": 1}, {"id": 2, "feedId": 1}]}"#;
        assert_eq!(
            decode_items(json.as_bytes(), |_| Ok::<_, Error>(())).unwrap(),
            2
        );
    }

    #[test]
    fn invalid_responses() {
        for json in [
            "",
            "[]",
            r#"{"items": {}}"#,
            r#"{"items": [{"id": 1}]}"#,
            r#"{"items": [{"id": 1, "feedId": 1}"#,
            r#"{"items": [{"id": 1, "feedId": 1}]} x"#,
            r#"{"items": [], "items": []}"#,
            r#"{"feeds": []}"#,
        ] {
            assert!(
                matches!(ids(json), Err(Error::Decode(_))),
                "accepted {json:?}"
            );
        }
    }

    #[test]
    fn message_instead_of_items() {
        let json = r#"{"message": "Setting getRead on an already filtered list is not allowed!"}"#;
        let err = ids(json).unwrap_err();
        assert!(matches!(err, Error::Decode(_)));
        assert!(err.to_string().contains("already filtered list"), "{err}");
    }

    #[derive(Debug)]
    enum CallerError {
        Full,
        Api(Error),
    }

    impl From<Error> for CallerError {
        fn from(err: Error) -> Self {
            CallerError::Api(err)
        }
    }

    #[test]
    fn caller_errors_stop_decoding() {
        let json = r#"{"items": [{"id": 3, "feedId": 1}, {"id": 2, "feedId": 1},
                                 {"id": 1, "feedId": 1}]}"#;
        let mut seen = 0;
        let result = decode_items(json.as_bytes(), |_| {
            seen += 1;
            if seen == 2 {
                Err(CallerError::Full)
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(CallerError::Full)));
        assert_eq!(seen, 2);

        let result = decode_items("{".as_bytes(), |_| Ok(()));
        assert!(matches!(result, Err(CallerError::Api(Error::Decode(_)))));
    }

    /// Hands out `head`, then `tail` only once the first item was handed to
    /// the callback.
    struct Gated<'a> {
        head: io::Cursor<&'a [u8]>,
        tail: io::Cursor<&'a [u8]>,
        open: &'a Cell<bool>,
    }

    impl Read for Gated<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.head.read(buf)? {
                0 if !self.open.get() => Err(io::Error::other("read past the first item")),
                0 => self.tail.read(buf),
                n => Ok(n),
            }
        }
    }

    #[test]
    fn items_are_handed_over_while_the_body_is_read() {
        let open = Cell::new(false);
        let reader = Gated {
            head: io::Cursor::new(br#"{"items": [{"id": 2, "feedId": 1},"#),
            tail: io::Cursor::new(br#"{"id": 1, "feedId": 1}]}"#),
            open: &open,
        };
        let mut ids = Vec::new();
        decode_items(reader, |item| {
            ids.push(item.id);
            open.set(true);
            Ok::<_, Error>(())
        })
        .unwrap();
        assert_eq!(ids, [2, 1]);
    }

    #[test]
    fn broken_connection_is_a_network_error() {
        let reader = Gated {
            head: io::Cursor::new(br#"{"items": [{"id": 2, "fe"#),
            tail: io::Cursor::new(b""),
            open: &Cell::new(false),
        };
        let result = decode_items(reader, |_| Ok::<_, Error>(()));
        assert!(matches!(result, Err(Error::Network(_))));
    }
}
