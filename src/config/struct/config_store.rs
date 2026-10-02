use std::collections::HashMap;
use std::path::PathBuf;

use super::ConfigFile;

/// What the persistence layer believes is on disk, in the cache's plaintext
/// terms. A save diffs the live cache against this snapshot and writes only
/// what changed — so a command-history push rewrites one small table instead
/// of re-serialising every session and re-pushing every saved password into
/// the OS keyring. Everything is serialized form (not structural equality) so
/// the comparison is exactly "would the bytes we would write differ".
#[derive(Default, Clone)]
pub(crate) struct SavedState {
    /// The `meta` settings blob (ConfigFile minus sessions and history) as a
    /// compact JSON string.
    pub(crate) settings: String,
    /// The session ids in display order, joined — reordering changes nothing
    /// per session but must still rewrite every row's ordinal.
    pub(crate) order: String,
    /// Per-session plaintext JSON as last written, keyed by id.
    pub(crate) sessions: HashMap<String, String>,
    /// The command-history list as a compact JSON string.
    pub(crate) history: String,
}

impl SavedState {
    /// Snapshot a cache the store has just read back from disk (or that has
    /// just been written): disk and cache agree, so an unchanged save is a
    /// no-op from the very first call.
    pub(crate) fn of_cache(cache: &ConfigFile) -> Self {
        SavedState {
            settings: settings_blob(cache),
            order: session_order(cache),
            sessions: cache
                .sessions
                .iter()
                .map(|s| (s.id.clone(), serde_json::to_string(s).unwrap_or_default()))
                .collect(),
            history: serde_json::to_string(&cache.command_history).unwrap_or_default(),
        }
    }
}

/// The compact JSON stored in the `meta` table for everything except sessions
/// and history. Same shape as the old sessions.json, so a legacy file can be
/// dropped in almost verbatim during migration.
pub(crate) fn settings_blob(cache: &ConfigFile) -> String {
    let mut settings = cache.clone();
    settings.sessions = Vec::new();
    settings.command_history = Vec::new();
    serde_json::to_string(&settings).unwrap_or_default()
}

/// The ordered session id list, joined for cheap whole-order comparison.
pub(crate) fn session_order(cache: &ConfigFile) -> String {
    cache
        .sessions
        .iter()
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// Which rows a save has to touch, derived by diffing the cache against
/// [`SavedState`].
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct SavePlan {
    pub(crate) settings: bool,
    /// Rewrite every session row (the display order changed, or the
    /// destination had no state to diff against).
    pub(crate) all_sessions: bool,
    /// Individual session ids to write; an id missing from the cache means
    /// its row is deleted.
    pub(crate) sessions: Vec<String>,
    pub(crate) history: bool,
}

impl SavePlan {
    /// Rewrite everything — used when there is no trustworthy state to diff
    /// against (a fresh or externally replaced database, a legacy JSON
    /// migration).
    pub(crate) fn all() -> Self {
        SavePlan {
            settings: true,
            all_sessions: true,
            sessions: Vec::new(),
            history: true,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        !self.settings && !self.all_sessions && self.sessions.is_empty() && !self.history
    }
}

pub struct ConfigStore {
    /// `sessions.db` in the data directory.
    pub(crate) path: PathBuf,
    pub(crate) backup_dir: Option<PathBuf>,
    pub(crate) cache: ConfigFile,
    /// ChaCha20-Poly1305 master key. Installed configs keep it in the OS
    /// keychain (`xenterm` / `master-key`); portable and keychain-less
    /// installs load (or create) it from `secret.key` next to sessions.db.
    pub(crate) key: [u8; 32],
    /// Whether session passwords go to the OS keyring (Windows Credential
    /// Manager, macOS Keychain, Linux Secret Service). True for the real
    /// store; the test constructors turn it off so tests stay deterministic
    /// and never write into the user's keyring.
    pub(crate) keyring_enabled: bool,
    /// What the last successful load/save left on disk. Shared with the
    /// background saver only in the sense that its snapshot is taken here and
    /// never written back: a background save leaves the snapshot alone, so
    /// the next foreground save re-diffs against pre-background state and
    /// rewrites whatever the background already wrote — harmless, and it
    /// keeps this field confined to the UI thread's `&mut` world.
    pub(crate) saved_state: std::sync::Mutex<SavedState>,
}
