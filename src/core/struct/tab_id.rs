//! A tab's identity, as a type rather than a `String`.
//!
//! Tab ids travel farther than any other id in this crate: from the page that
//! mints them, through the session pumps' channels, into the shared status and
//! SFTP maps, and back out to the detached windows that follow the active tab.
//! A bare `String` carried them all, which meant any `String` could — the
//! compiler would happily hand a session id, a group name or a config path
//! where a tab id was wanted, and the map lookups would simply come back
//! empty.
//!
//! The newtype makes that a type error. It is deliberately not `Copy` — ids
//! are cloned where they are kept, and a move that silently empties the
//! original is the kind of accident the newtype exists to surface.
//!
//! The conversion is intentionally one-way cheap: `as_str` for reading, and
//! `TabId::new` at the two mint points (the terminal page's open/duplicate).
//! Everything else constructs from another `TabId` or parses an id it can
//! already trust.

/// One open tab's identity. Minted by the terminal page; equal to the session
/// id of the session running in it, which is the convention the shared maps
/// key on.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TabId(String);

impl TabId {
    /// Mint an id. Only the terminal page's open and duplicate paths call
    /// this; everything else derives ids from other ids.
    pub(crate) fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The plain string, for the map keys and the wire protocol that predate
    /// the newtype. Call sites are meant to shrink over time; `as_str` covers
    /// the reads.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::TabId;

    /// The id is its string, and two ids from one string are the same id —
    /// the map-keying convention the crate runs on.
    #[test]
    fn ids_from_the_same_string_are_equal() {
        let a = TabId::new("session-1");
        let b = TabId::new(String::from("session-1"));
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "session-1");
        assert_eq!(a.to_string(), "session-1");
    }

    /// The mint is the only `String` door, and it is consumed: a moved id
    /// cannot be read again.
    #[test]
    fn into_string_consumes_the_id() {
        let id = TabId::new("abc");
        let raw = id.into_string();
        assert_eq!(raw, "abc");
    }
}
