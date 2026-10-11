//! Session / application configuration.
//!
//! Persists a single SQLite database (`sessions.db`) in the app's data
//! directory. Resolution is **portable-first** (#141): a `config/` folder next
//! to the executable is preferred so the whole app can ride along on a USB
//! stick and never litters the user profile. When the executable lives
//! somewhere read-only (a system-wide install under Program Files / `/usr`),
//! it falls back to the per-user OS config dir (e.g. `%APPDATA%/xenterm`,
//! `~/.config/xenterm`), which is also where every pre-0.4.15 version stored
//! its data — so existing installs keep working untouched. See [`data_dir`].
//!
//! ## Layout
//!
//! Three tables, all mirrored in RAM ([`ConfigStore::cache`]) so readers never
//! touch the database:
//!
//! * `meta` — schema version plus one `settings` row: the whole `ConfigFile`
//!   as JSON minus sessions and history (same shape as the old
//!   sessions.json).
//! * `sessions` — one row per saved session in display order, its JSON in
//!   "disk form" (secrets transformed, see below).
//! * `command_history` — append-mostly rows; the hot path (every command sent
//!   from the command box) writes one row instead of re-serialising the whole
//!   config.
//!
//! A save diffs the cache against a snapshot of what was last written and
//! touches only the rows that changed; a save with no diff is a no-op. A
//! legacy `sessions.json` found without a database is imported once and kept
//! beside the database as `sessions.json.migrated`.
//!
//! ## Password encryption
//!
//! Passwords are **not** stored in plaintext.  Every non-empty secret is
//! encrypted with **ChaCha20-Poly1305** (a random 96-bit nonce per value) and
//! stored as
//!
//! ```text
//! enc:v1:<base64url(nonce_12_bytes || ciphertext)>
//! ```
//!
//! Session passwords additionally move to the OS keyring whenever one is
//! available; the disk copy then carries only the `keyring:v1` marker.
//!
//! The master key for the `enc:v1:` blobs lives in the **OS keychain**
//! (Windows Credential Manager / macOS Keychain / Linux Secret Service) when
//! the config sits in the installed, per-user data dir — so a leaked or synced
//! copy of the config directory alone can no longer decrypt anything. The
//! first launch after this scheme adopts the old on-disk `secret.key` and,
//! once the keychain write round-trips, deletes the file. Where the platform
//! has no usable keychain (e.g. headless Linux), `secret.key` remains the
//! fallback so the app keeps working. **Portable** installs (`config/` beside
//! the exe) always use the key file: the directory must stay self-contained on
//! a USB stick, and the keychain of whichever machine it is plugged into is
//! not.
//!
//! Legacy plaintext passwords (from older installs) are left untouched in
//! memory and silently re-encrypted the next time the config is saved.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit},
    ChaCha20Poly1305,
};
use directories::ProjectDirs;
use rand::rngs::OsRng;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::structs::*;

/// The database save failed and the original keyring credential could not be
/// restored and verified. Callers must not describe this as a complete rollback.
#[derive(Debug, thiserror::Error)]
#[error("could not confirm the save outcome or restore the original keyring credential")]
pub(crate) struct SessionCredentialRollbackFailed;

/// A stale cache must never become an implicit last-writer-wins overwrite.
#[derive(Debug, thiserror::Error)]
#[error("configuration changed since it was loaded; keep pending edits and reload before saving")]
pub(crate) struct ConfigurationChanged;

// ── Data directory resolution (portable-first, #141) ──────────────────────────
//
// All user data — sessions.db, secret.key, known_hosts, error.log — lives in
// ONE directory resolved here, and `errlog` / `known_hosts` route through it too.

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static PINNED_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

#[path = "profile.rs"]
mod profile;
#[path = "profile_preflight.rs"]
mod profile_preflight;
pub use profile::{configure_profile, has_explicit_data_dir};

/// The single directory holding all user data (sessions, encryption key,
/// known_hosts, error.log). Resolved once and cached; any one-time migration
/// from the legacy per-user dir runs exactly once.
///
/// Portable-first: prefers a `config/` folder beside the executable, falling
/// back to the per-user OS config dir when the exe dir is read-only (#141).
pub fn data_dir() -> PathBuf {
    DATA_DIR.get_or_init(resolve_data_dir).clone()
}

/// Directory for diagnostic logs (`error.log`). Kept *separate* from the config
/// dir so logs don't clutter user data: portable-first → a `log/` folder beside
/// the executable (a sibling of `config/`). On Windows, the fallback is
/// `%APPDATA%/xenterm/xenterm/log/log`, outside the config directory
/// (#log-dir).
pub fn log_dir() -> PathBuf {
    if let Some(dir) = PINNED_DATA_DIR.get() {
        let log = dir.join("log");
        let _ = fs::create_dir_all(&log);
        return log;
    }
    // Portable: <exe_dir>/log, sibling of the portable config/ folder.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let log = parent.join("log");
            if fs::create_dir_all(&log).is_ok() && dir_is_writable(&log) {
                return log;
            }
        }
    }
    // Resolve independently from portable configuration storage.
    let dir = user_log_dir();
    let _ = fs::create_dir_all(&dir);
    dir
}

fn user_log_dir() -> PathBuf {
    let config = data_dir();
    user_log_dir_from_config(&config, cfg!(target_os = "windows"))
}

fn user_log_dir_from_config(config: &Path, windows: bool) -> PathBuf {
    if windows {
        if let Some(base) = config.parent() {
            return base.join("log").join("log");
        }
    }
    config.join("log")
}

/// The per-user OS config dir (`%APPDATA%/xenterm/XenTerm/config`,
/// `~/.config/xenterm`, …).
fn user_data_dir() -> Option<PathBuf> {
    ProjectDirs::from("dev", "xenterm", "XenTerm").map(|d| d.config_dir().to_path_buf())
}

/// The location the app used before the XenTerm rename
/// (`%APPDATA%/xenterm/xenterm`, `~/.config/xenterm`, …). Kept as the
/// migration source: an upgrade must carry the user's saved sessions over,
/// never lose them.
fn legacy_data_dir() -> Option<PathBuf> {
    ProjectDirs::from("dev", "meatshell", "meatshell").map(|d| d.config_dir().to_path_buf())
}

/// Portable location: a `config/` folder beside the executable.
fn portable_data_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("config"))
}

/// True only if we can actually create and write a file in `dir` — Program Files
/// and other system locations can reject writes even when the dir appears to
/// exist, so a real write probe is the reliable test.
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".write_probe_{}", std::process::id()));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn resolve_data_dir() -> PathBuf {
    if let Some(dir) = PINNED_DATA_DIR.get() {
        return dir.clone();
    }
    let legacy = legacy_data_dir();

    if let Some(portable) = portable_data_dir() {
        // Already a portable install → keep using it (nothing to migrate).
        if portable.exists() && dir_is_writable(&portable) {
            return portable;
        }
        // Otherwise try to claim the portable dir. This succeeds only where the
        // exe directory is writable (i.e. not a Program Files / system install),
        // which is exactly when portable mode makes sense.
        if fs::create_dir_all(&portable).is_ok() && dir_is_writable(&portable) {
            if let Some(ref legacy) = legacy {
                migrate_legacy(legacy, &portable);
            }
            return portable;
        }
    }

    // The per-user dir under the XenTerm name, carrying the pre-rename
    // location's files over on first use. `ConfigStore::load` additionally
    // restores missing profiles from dedicated backups or the legacy source.
    if let Some(user) = user_data_dir() {
        if fs::create_dir_all(&user).is_ok() && dir_is_writable(&user) {
            if let Some(ref legacy) = legacy {
                migrate_legacy(legacy, &user);
            }
            return user;
        }
    }

    // The new dir could not be created or written — fall back to the
    // pre-rename per-user dir rather than losing an existing user's data.
    // Last resort: a temp dir, so the app still launches if neither is
    // available.
    let dir = legacy.unwrap_or_else(|| std::env::temp_dir().join("xenterm"));
    let _ = fs::create_dir_all(&dir);
    dir
}

/// On the first launch that lands on the portable dir, copy user data over from
/// the legacy per-user dir so upgrading users keep their saved sessions. The
/// originals are left in place (copy, not move) as a safety net, and existing
/// destination files are never overwritten (#141).
fn migrate_legacy(legacy: &Path, portable: &Path) {
    if let Err(error) = recovery::migrate_legacy(legacy, portable) {
        tracing::warn!("profile recovery deferred: {error:#}");
    }
}

#[cfg(test)]
fn sessions_file_has_connections(path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<ConfigFile>(&raw)
        .map(|cfg| !cfg.sessions.is_empty())
        .unwrap_or(false)
}

/// Whether the directory holds any saved session, in either storage format.
/// Guards the one-time backup restore so it never clobbers live data.
#[cfg(test)]
fn config_dir_has_sessions(dir: &Path) -> bool {
    db_has_sessions(&dir.join("sessions.db")).unwrap_or(false)
        || sessions_file_has_connections(&dir.join("sessions.json"))
}

/// Read-only probe: does this database contain at least one session? Errors
/// (missing file, no table yet, a WAL file we can't open read-only) mean "no".
#[cfg(test)]
fn db_has_sessions(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| {
        row.get::<_, i64>(0)
    })? > 0)
}

/// One-time disaster recovery: when the primary config dir holds no sessions
/// but the legacy per-user dir does, copy them over. A backup in the current
/// format is a sessions.db snapshot; older installs left a sessions.json
/// behind, which [`ConfigStore::load`] then imports and renames. `secret.key`
/// only matters for portable/keychain-less installs, where it still exists.
fn restore_user_backup_if_needed(primary_dir: &Path, backup_dir: &Path) -> Result<()> {
    recovery::restore_user_backup_if_needed(primary_dir, backup_dir)
}

fn normalize_hex_color(value: &str) -> Option<String> {
    hex_to_rgb(value).map(|(r, g, b)| format!("#{r:02X}{g:02X}{b:02X}"))
}

/// What reading a store database hands back: the settings blob (secrets
/// still encrypted), sessions in disk form, history oldest-first.
type DiskStore = (String, Vec<Session>, Vec<String>);

/// A stored hex colour as its three channels.
///
/// Shared with the frontend, which has to draw the colour a setting names: the terminal
/// view parses this string, and a second parser would be a second answer to what counts
/// as a valid colour.
pub fn hex_to_rgb(value: &str) -> Option<(u8, u8, u8)> {
    let digits = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some((channel(0..2)?, channel(2..4)?, channel(4..6)?))
}

/// A brand-new config (no file yet, or the old one was corrupt). Seeds the
/// new-user default layout (#new-user-defaults): ms wallpaper, welcome page as
/// a left sidebar, resource panel docked right, 15% wallpaper transparency, and
/// marks the migration done so it isn't re-applied.
fn fresh_config() -> ConfigFile {
    ConfigFile {
        wallpaper: "builtin:ms".to_string(),
        welcome_as_sidebar: true,
        sidebar_dock: "right".to_string(),
        wallpaper_overlay: DEFAULT_WALLPAPER_OVERLAY,
        defaults_rev: DEFAULTS_REV,
        ..ConfigFile::default()
    }
}

/// One-time push of the new default layout to *existing* users — but only for
/// each item they're still leaving at the old default, so deliberate choices are
/// never clobbered. Runs once (gated by `defaults_rev`); returns whether anything
/// changed so the caller can persist it. (#new-user-defaults)
fn migrate_defaults(cfg: &mut ConfigFile) -> bool {
    if cfg.defaults_rev >= DEFAULTS_REV {
        return false;
    }
    // rev 1: miku / welcome-as-sidebar / right-docked resources / wallpaper overlay.
    if cfg.defaults_rev < 1 {
        // Old default wallpaper → miku. A custom path, "none" (""), or any other
        // built-in means the user chose it, so leave it.
        if cfg.wallpaper == "builtin:tech" {
            cfg.wallpaper = "builtin:miku".to_string();
        }
        // Overlay still unset -> current default.
        if cfg.wallpaper_overlay <= 0.0 {
            cfg.wallpaper_overlay = DEFAULT_WALLPAPER_OVERLAY;
        }
        // Never enabled the welcome sidebar → enable it.
        if !cfg.welcome_as_sidebar {
            cfg.welcome_as_sidebar = true;
        }
        // Never moved the resource panel (empty = the old left default) → right.
        if cfg.sidebar_dock.trim().is_empty() {
            cfg.sidebar_dock = "right".to_string();
        }
    }
    // rev 2: settings show wallpaper transparency, while rev 1 accidentally
    // stored the default as panel alpha 0.38, so it displayed as ~62%.
    if cfg.defaults_rev < 2
        && (cfg.wallpaper_overlay - PREVIOUS_DEFAULT_WALLPAPER_TRANSPARENCY).abs() < 0.005
    {
        cfg.wallpaper_overlay = DEFAULT_WALLPAPER_OVERLAY;
    }
    // rev 3: reduce the default transparency from 38% to 15%. Only advance
    // users still on the previous default; preserve every custom slider value.
    if cfg.defaults_rev < 3
        && (cfg.wallpaper_overlay - PREVIOUS_DEFAULT_WALLPAPER_OVERLAY).abs() < 0.005
    {
        cfg.wallpaper_overlay = DEFAULT_WALLPAPER_OVERLAY;
    }
    // rev 4: the MCP preview used to default all four capability switches to
    // true and persisted that, so every config saved by a preview build carries
    // `true` values that no user ever chose. A `true` that arrived as a default
    // is indistinguishable from one deliberately set, so reset them all once:
    // anyone who actually wants MCP re-enables it in Settings > Interface > MCP.
    // Nothing else in the config is touched (H-04/H-05).
    if cfg.defaults_rev < 4 {
        cfg.mcp_enabled = false;
        cfg.mcp_use_saved_credentials = false;
        cfg.mcp_allow_commands = false;
        cfg.mcp_allow_file_transfers = false;
    }
    // rev 5: the embedded terminal font changed. "Meatshell Mono" no longer
    // ships — the binary now embeds MiSans (default) and HarmonyOS Sans SC — so
    // a config still naming the retired face is reset to empty, which means
    // "the new default". Any other value is a face the user chose and stands.
    if cfg.defaults_rev < 5 && cfg.font_family == RETIRED_DEFAULT_FONT {
        cfg.font_family = String::new();
    }
    // rev 6: the terminal went back to monospace-only. The proportional faces
    // rev 5 made the default (and the picker's head entries) are not terminal
    // candidates — a proportional face in a cell grid drifts against the
    // cursor — so a config naming either is reset to empty, which now means
    // the bundled monospace. Same covenant as the MCP reset: a value that
    // arrived as a default is indistinguishable from one deliberately set, so
    // both go; a proportional face has no place in the terminal anyway, by the
    // same rule the picker enforces.
    if cfg.defaults_rev < 6
        && RETIRED_PROPORTIONAL_DEFAULTS
            .iter()
            .any(|face| *face == cfg.font_family)
    {
        cfg.font_family = String::new();
    }
    cfg.defaults_rev = DEFAULTS_REV;
    true
}

fn normalize_highlight_color(color: &str) -> &'static str {
    match color {
        "yellow" => "yellow",
        "green" => "green",
        "cyan" => "cyan",
        "magenta" => "magenta",
        "gray" => "gray",
        _ => "red",
    }
}

/// Remove duplicate entries in place, keeping the *last* (most recent)
/// occurrence of each and preserving relative order (#113). The list is capped
/// at 200, so the quadratic scan is trivial.
fn dedup_keep_last(items: &mut Vec<String>) {
    let mut i = 0;
    while i < items.len() {
        if items[i + 1..].contains(&items[i]) {
            items.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Display-only session groups that must never be persisted as user folders.
/// `default` maps to an empty group; `system` is owned by built-in local shells.
pub(crate) fn is_reserved_session_group(name: &str) -> bool {
    name.eq_ignore_ascii_case("default") || name.eq_ignore_ascii_case("system")
}

/// Named display groups: explicit folders ∪ the groups sessions are filed
/// under, with reserved names and ungrouped excluded, de-duplicated and
/// sorted case-insensitively. The single source of truth for group display
/// order — shared by the welcome list, drag-reorder target finding and the
/// group dropdown; if these ever drift, drop targets and rendered rows
/// disagree (#41).
pub(crate) fn named_display_groups(explicit: &[String], sessions: &[Session]) -> Vec<String> {
    let mut named: Vec<String> = explicit
        .iter()
        .filter(|group| !is_reserved_session_group(group.trim()))
        .cloned()
        .chain(
            sessions
                .iter()
                .filter(|session| {
                    !session.group.is_empty() && !is_reserved_session_group(session.group.trim())
                })
                .map(|session| session.group.clone()),
        )
        .collect();
    named.sort_by_key(|group| group.to_lowercase());
    named.dedup();
    named
}

/// Repair configurations created before #316/#324, when the Move-to menu exposed
/// the built-in `system` group as a destination for saved server sessions.
fn normalize_reserved_session_groups(cfg: &mut ConfigFile) -> bool {
    let old_group_count = cfg.groups.len();
    cfg.groups
        .retain(|group| !is_reserved_session_group(group.trim()));
    let mut changed = cfg.groups.len() != old_group_count;
    for session in &mut cfg.sessions {
        if is_reserved_session_group(session.group.trim()) {
            session.group.clear();
            changed = true;
        }
    }
    changed
}

#[cfg(any(target_os = "macos", test))]
fn normalize_macos_renderer_mode(mode: &str) -> &'static str {
    match mode {
        "femtovg" => "femtovg",
        "skia" => "skia",
        _ => "software",
    }
}

impl ConfigStore {
    /// The prefix that marks an encrypted password blob at rest.
    const ENC_PREFIX: &'static str = "enc:v1:";

    /// Marks a password encrypted with the **portable export key** (issue #46).
    const EXPORT_PREFIX: &'static str = "enc:exp:v1:";

    /// Fixed 32-byte key for portable exports. Baked into the binary so an
    /// exported file decrypts on any machine. Obfuscation only — see `ExportFile`.
    // The value predates the XenTerm rename and must not change: it is
    // the fixed key portable export files were encrypted with, and old
    // exports must keep decrypting after the rename.
    const EXPORT_KEY: [u8; 32] = *b"meatshell.export.portable.key.01";

    // ── Encryption helpers ────────────────────────────────────────────────

    /// Encrypt `plaintext` with ChaCha20-Poly1305 and return
    /// `"enc:v1:<base64url(nonce_12_bytes || ciphertext)>"`.
    fn encrypt(key: &[u8; 32], plaintext: &str) -> Result<String> {
        let cipher = ChaCha20Poly1305::new(key.into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng); // 12 random bytes
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("password encrypt error: {e}"))?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        Ok(format!(
            "{}{}",
            Self::ENC_PREFIX,
            URL_SAFE_NO_PAD.encode(&blob)
        ))
    }

    /// Try to decrypt a value produced by [`Self::encrypt`].
    /// Returns `None` if the string is not an encrypted blob (e.g. a legacy
    /// plaintext value, an empty string, or a tampered/corrupt blob).
    fn try_decrypt(key: &[u8; 32], s: &str) -> Option<String> {
        let b64 = s.strip_prefix(Self::ENC_PREFIX)?;
        let blob = URL_SAFE_NO_PAD.decode(b64).ok()?;
        if blob.len() < 12 {
            return None;
        }
        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let cipher = ChaCha20Poly1305::new(key.into());
        let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);
        let plain = cipher.decrypt(nonce, ciphertext).ok()?;
        String::from_utf8(plain).ok()
    }

    /// Rewrite a proxy password, preserving the original optional scheme and
    /// literal userinfo encoding. Connection parsing uses the same splitter,
    /// including the implicit SOCKS5 form `user:pass@host:port`.
    fn map_proxy_password(url: &str, map: impl FnOnce(&str) -> Option<String>) -> Option<String> {
        let parts = super::validation::split_proxy_url(url);
        let (user, pass) = parts.auth?;
        if pass.is_empty() {
            return None;
        }
        let mapped = map(pass)?;
        let prefix = parts
            .scheme
            .map(|scheme| format!("{scheme}://"))
            .unwrap_or_default();
        Some(format!("{prefix}{user}:{mapped}@{}", parts.hostport))
    }

    // ── Key file management ───────────────────────────────────────────────

    /// The keyring account holding the master encryption key when the config
    /// lives in the installed (per-user) data dir. Sister of the per-session
    /// password entries (`KEYRING_SERVICE`, session id) — same store, same
    /// lifetime as the OS user account.
    const MASTER_KEY_ACCOUNT: &'static str = "master-key";

    fn master_key_entry() -> Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new(Self::KEYRING_SERVICE, Self::MASTER_KEY_ACCOUNT)
    }

    /// Decode the keychain-stored form of the master key (base64url of the 32
    /// raw bytes). `None` for anything that isn't exactly that — a keyring
    /// entry that can't be parsed can't decrypt anything either.
    fn decode_master_key(stored: &str) -> Option<[u8; 32]> {
        let bytes = URL_SAFE_NO_PAD.decode(stored).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Some(key)
    }

    /// Read `secret.key` from `config_dir`; `None` when missing or malformed.
    fn read_key_file(config_dir: &Path) -> Option<[u8; 32]> {
        let bytes = fs::read(config_dir.join("secret.key")).ok()?;
        if bytes.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Some(key)
        } else {
            tracing::warn!("secret.key has wrong length — regenerating");
            None
        }
    }

    /// Resolve the master key for the `enc:v1:` blobs.
    ///
    /// Installed (per-user) dir: the key lives in the OS keychain, so it no
    /// longer sits next to the ciphertext it protects — under the old
    /// `secret.key`-beside-`sessions.json` layout, anyone who exfiltrated the
    /// config directory (backup, cloud sync, copied disk) could decrypt every
    /// non-keyring secret in it. The first launch after the move adopts
    /// `secret.key` and, only once the keychain write has been verified by
    /// reading it back, deletes the file. When the platform has no usable
    /// keychain (e.g. headless Linux), the key file remains the fallback so
    /// the app keeps working.
    ///
    /// Portable dir: the key file travels beside `sessions.db` on purpose —
    /// see the module docs.
    fn resolve_master_key(config_dir: &Path, portable: bool) -> Result<[u8; 32]> {
        if cfg!(feature = "desktop") && !portable {
            match Self::master_key_entry().and_then(|entry| entry.get_password()) {
                Ok(stored) => {
                    if let Some(key) = Self::decode_master_key(&stored) {
                        return Ok(key);
                    }
                    // An entry we can't parse can't decrypt anything either;
                    // drop it and re-derive from the key file (or fresh).
                    tracing::warn!("keyring master-key entry is malformed; re-deriving");
                    let _ = Self::master_key_entry().and_then(|entry| entry.delete_credential());
                }
                Err(keyring::Error::NoEntry) => {
                    if let Some(key) = Self::seed_master_key_in_keyring(config_dir) {
                        return Ok(key);
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        "OS keyring unavailable ({error}); using the secret.key file fallback"
                    );
                }
            }
        }
        Self::load_or_create_key(config_dir)
    }

    /// First launch with the key-in-keychain scheme: adopt `secret.key` when
    /// one exists (otherwise generate a fresh key), park it in the OS keychain,
    /// and only after the write round-trips remove the file copy. Any failure
    /// keeps the file — destroying the only other copy of the key would make
    /// every existing `enc:v1:` blob permanently undecryptable.
    fn seed_master_key_in_keyring(config_dir: &Path) -> Option<[u8; 32]> {
        use rand::RngCore as _;
        let from_file = Self::read_key_file(config_dir);
        let key = from_file.unwrap_or_else(|| {
            let mut key = [0u8; 32];
            OsRng.fill_bytes(&mut key);
            key
        });
        let stored = URL_SAFE_NO_PAD.encode(key);
        if let Err(error) = Self::master_key_entry().and_then(|entry| entry.set_password(&stored)) {
            tracing::warn!("storing master key in OS keyring failed ({error}); keeping secret.key");
            return None;
        }
        // Verify before removing the fallback: a keychain that accepted the
        // write but wouldn't hand it back would brick the config next launch.
        let read_back = Self::master_key_entry().and_then(|entry| entry.get_password());
        if !matches!(read_back.as_deref(), Ok(s) if Self::decode_master_key(s) == Some(key)) {
            tracing::warn!("keyring master-key write could not be verified; keeping secret.key");
            return None;
        }
        if from_file.is_some() {
            if let Err(error) = fs::remove_file(config_dir.join("secret.key")) {
                // A harmless leftover: the keychain entry always wins from now
                // on, the file is simply never consulted again.
                tracing::warn!("removing the migrated secret.key failed: {error}");
            }
        }
        Some(key)
    }

    /// Load the 32-byte key from `<config_dir>/secret.key`, or generate and
    /// persist a fresh one.  On Unix the key file is created with mode `0600`
    /// so other local accounts cannot read it.  On Windows files in `%APPDATA%`
    /// are already restricted to the owning user by default ACLs.
    ///
    /// Creation is claimed with `create_new`, so two instances first-booting at
    /// the same time cannot each mint a key and clobber the other's — the loser
    /// re-reads the winner's key, and ciphertext written by either stays
    /// decryptable (audit N-低3).
    fn load_or_create_key(config_dir: &Path) -> Result<[u8; 32]> {
        use rand::RngCore as _;
        let key_path = config_dir.join("secret.key");

        if key_path.exists() {
            let bytes = fs::read(&key_path)
                .with_context(|| format!("failed to read {}", key_path.display()))?;
            if bytes.len() == 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                return Ok(key);
            }
            tracing::warn!("secret.key has wrong length — regenerating");
        }

        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        #[cfg(unix)]
        let create = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&key_path)
        };
        #[cfg(not(unix))]
        let create = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&key_path);
        match create {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(&key)
                    .and_then(|_| file.flush())
                    .with_context(|| format!("failed to write {}", key_path.display()))?;
                #[cfg(unix)]
                {
                    // Best effort: creation already used mode 0600, so this only
                    // repairs a file someone chmod'd wide in between.
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600));
                }
                tracing::info!("generated new encryption key at {}", key_path.display());
                Ok(key)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another instance (or thread) created the key between our
                // check and this create: adopt theirs rather than overwrite —
                // overwriting would orphan every secret the winner already
                // encrypted with its key.
                let bytes = fs::read(&key_path)
                    .with_context(|| format!("failed to read {}", key_path.display()))?;
                if bytes.len() != 32 {
                    bail!(
                        "{} is {} bytes, expected 32; refusing to overwrite it",
                        key_path.display(),
                        bytes.len()
                    );
                }
                key.copy_from_slice(&bytes);
                Ok(key)
            }
            Err(error) => Err(error).with_context(|| format!("create {}", key_path.display())),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// Load (or initialise) the config store. Read/locking failures preserve
    /// the original database instead of treating an active profile as corrupt.
    /// Legacy JSON is archived only after its optimistic migration commits.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let config_dir = path
            .parent()
            .context("config path has no parent directory")?
            .to_path_buf();

        fs::create_dir_all(&config_dir)
            .with_context(|| format!("failed to create config dir {}", config_dir.display()))?;

        if has_explicit_data_dir() {
            Self::preflight_explicit_profile(&config_dir)?;
        }
        let legacy_backup =
            legacy_data_dir().filter(|dir| !has_explicit_data_dir() && dir != &config_dir);
        if let Some(ref legacy) = legacy_backup {
            restore_user_backup_if_needed(&config_dir, legacy)?;
        }
        let backup_dir = legacy_backup
            .as_deref()
            .map(|legacy| recovery::backup_directory(&config_dir, legacy))
            .transpose()?;

        // Short cross-process protection spans master-key initialization,
        // legacy decryption, pending credential recovery, and a consistent load.
        // It is released before migration saving (which acquires its own guard).
        let ordered = Self::save_lock().lock().unwrap_or_else(|p| p.into_inner());
        let profile_io = profile_io::ProfileIoGuard::acquire(&path)?;
        // The master key: OS keychain when installed, key file when portable
        // or when the platform has no usable keyring (see resolve_master_key).
        let portable =
            has_explicit_data_dir() || portable_data_dir().is_some_and(|dir| dir == config_dir);
        let key = Self::resolve_master_key(&config_dir, portable)?;

        // Legacy file left by an upgrade; imported below, then renamed.
        let legacy_json = path.with_file_name("sessions.json");
        let mut disk_fingerprint = None;
        let mut force_full_write = false;
        let mut imported_legacy_json = false;
        let mut cache = if path.exists() {
            match Self::read_cache_snapshot_locked(&path, &key, &profile_io) {
                Ok((cfg, fingerprint)) => {
                    disk_fingerprint = fingerprint;
                    if fingerprint.is_none() && legacy_json.exists() {
                        // A failed/aborted first migration may have created an
                        // empty schema. The retained source must still import.
                        imported_legacy_json = true;
                        force_full_write = true;
                        Self::read_json_store(&legacy_json, &key).context(
                            "legacy configuration could not be read; original file preserved",
                        )?
                    } else {
                        cfg
                    }
                }
                Err(err) => {
                    return Err(err
                        .context("configuration could not be loaded; original database preserved"))
                }
            }
        } else if profile_io::pending_journal(&path)? {
            return Err(SessionCredentialRollbackFailed.into());
        } else if legacy_json.exists() {
            match Self::read_json_store(&legacy_json, &key) {
                Ok(cfg) => {
                    imported_legacy_json = true;
                    force_full_write = true;
                    cfg
                }
                Err(err) => {
                    return Err(err.context(
                        "legacy configuration could not be read; original file preserved",
                    ))
                }
            }
        } else {
            fresh_config()
        };

        drop(profile_io);
        drop(ordered);
        // Keep the actually loaded cache as the diff baseline even if a
        // migration fails; a later retry must still have work to persist.
        let mut saved_state = SavedState::of_cache(&cache);
        saved_state.disk_fingerprint = disk_fingerprint;
        // Clean up any duplicate history accumulated before #113, keeping the
        // last (most recent) occurrence of each command.
        dedup_keep_last(&mut cache.command_history);
        // `system` and `default` are display-only group names. Older builds
        // allowed moving saved servers into `system`, creating a duplicate
        // empty-menu folder (#316, #324).
        let mut migrated = normalize_reserved_session_groups(&mut cache);
        // One-time push of the new default layout to existing users (only for
        // items they never changed). (#new-user-defaults)
        migrated |= migrate_defaults(&mut cache);
        force_full_write |= migrated;

        let store = Self {
            path,
            backup_dir,
            cache,
            key,
            keyring_enabled: cfg!(feature = "desktop") && !has_explicit_data_dir(),
            saved_state: std::sync::Mutex::new(saved_state).into(),
        };
        // Persist the migrations so they run exactly once (and so a later
        // opt-out — e.g. turning the welcome sidebar back off — isn't reverted
        // next launch).
        if force_full_write {
            if let Err(e) = store.save_all() {
                if has_explicit_data_dir() {
                    bail!("failed to persist explicit profile migration; source JSON preserved");
                }
                tracing::warn!("failed to persist config migration: {e:#}");
            } else if imported_legacy_json {
                if let Err(error) = recovery::finish_legacy_migration(&legacy_json) {
                    tracing::warn!("could not archive legacy configuration: {error:#}");
                }
            }
        }
        Ok(store)
    }

    fn config_path() -> Result<PathBuf> {
        Ok(data_dir().join("sessions.db"))
    }

    pub fn sessions(&self) -> &[Session] {
        &self.cache.sessions
    }

    /// Drag-to-reorder a saved session (`dir < 0` = up). The stored Vec order
    /// is the display order within a group (same convention as quick commands).
    /// Same-group hops swap neighbours; when there is no same-group neighbour
    /// the hop crosses the group boundary instead: the session moves into the
    /// nearest visible display group along `dir`, landing at that group's
    /// boundary (first member moving down, last moving up). Returns whether
    /// anything changed.
    pub fn reorder_session(&mut self, id: &str, dir: isize) -> bool {
        // Display group of a session: ungrouped (and reserved names) render
        // under "default"; everything else under its own group. Mirrors
        // build_session_rows in src/app/session_models.rs.
        fn display_group(session: &Session) -> String {
            if session.group.is_empty() || is_reserved_session_group(session.group.trim()) {
                "default".to_string()
            } else {
                session.group.clone()
            }
        }

        let Some(idx) = self.cache.sessions.iter().position(|s| s.id == id) else {
            return false;
        };
        let group = display_group(&self.cache.sessions[idx]);

        // Same-group neighbour in stored (= display) order → plain swap.
        let same_group_target = {
            let sessions = &self.cache.sessions;
            if dir < 0 {
                (0..idx)
                    .rev()
                    .find(|&i| display_group(&sessions[i]) == group)
            } else {
                (idx + 1..sessions.len()).find(|&i| display_group(&sessions[i]) == group)
            }
        };
        if let Some(target) = same_group_target {
            self.cache.sessions.swap(idx, target);
            return true;
        }

        // Cross-group hop. Display order: "default" first (only when ungrouped
        // sessions exist), then named groups — explicit folders ∪ sessions'
        // groups — alphabetically. Collapsed groups are skipped: their rows
        // are hidden, so a card must never land inside one.
        let Some(collapsed_groups) = self.cache.collapsed_session_groups.clone() else {
            // Mirrors build_session_rows: no collapse list = everything
            // collapsed, so no visible target group can exist.
            return false;
        };
        let is_collapsed = |name: &str| collapsed_groups.iter().any(|g| g == name);

        let mut display: Vec<String> = Vec::new();
        if self
            .cache
            .sessions
            .iter()
            .any(|s| display_group(s) == "default")
        {
            display.push("default".to_string());
        }
        display.extend(named_display_groups(
            &self.cache.groups,
            &self.cache.sessions,
        ));

        let Some(pos) = display.iter().position(|g| g == &group) else {
            return false;
        };
        let target_group = if dir < 0 {
            (0..pos).rev().find(|&i| !is_collapsed(&display[i]))
        } else {
            (pos + 1..display.len()).find(|&i| !is_collapsed(&display[i]))
        }
        .map(|i| display[i].clone());
        let Some(target_group) = target_group else {
            return false;
        };

        let source_group = self.cache.sessions[idx].group.clone();
        let mut moved = self.cache.sessions.remove(idx);
        moved.group = if target_group == "default" {
            String::new()
        } else {
            target_group.clone()
        };
        let insert_at = {
            let members: Vec<usize> = self
                .cache
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| display_group(s) == target_group)
                .map(|(i, _)| i)
                .collect();
            if members.is_empty() {
                idx.min(self.cache.sessions.len())
            } else if dir < 0 {
                members[members.len() - 1] + 1
            } else {
                members[0]
            }
        };
        self.cache.sessions.insert(insert_at, moved);

        // A named source group that just lost its last member would vanish
        // from the list (changing the row count mid-drag, which drops the
        // dragging row's pointer grab); keep it as an empty folder. Register
        // it in `groups` directly rather than via add_group: the folder was
        // visibly expanded during the drag, and add_group would collapse it.
        if group != "default"
            && !self.cache.sessions.iter().any(|s| s.group == source_group)
            && !self.cache.groups.iter().any(|g| g == &source_group)
        {
            self.cache.groups.push(source_group);
        }
        true
    }

    pub fn upsert(&mut self, mut session: Session) {
        if is_reserved_session_group(session.group.trim()) {
            session.group.clear();
        }
        if let Some(existing) = self.cache.sessions.iter_mut().find(|s| s.id == session.id) {
            *existing = session;
        } else {
            self.cache.sessions.push(session);
        }
    }

    /// Save an editor's change, restoring the previous in-memory session if the
    /// database write fails. A failed save must not become visible to another
    /// view, or be persisted later by an unrelated settings change.
    pub fn upsert_and_save(&mut self, mut session: Session) -> Result<()> {
        // Include credential snapshot and compensation in the same ordering
        // boundary as the database write; no background save may slip between.
        let _ordered = Self::save_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if is_reserved_session_group(session.group.trim()) {
            session.group.clear();
        }
        let id = session.id.clone();
        let index = self.cache.sessions.iter().position(|s| s.id == id);
        let previous = if let Some(index) = index {
            Some(std::mem::replace(&mut self.cache.sessions[index], session))
        } else {
            self.cache.sessions.push(session);
            None
        };
        if let Err(error) = self.save_impl_locked(None, self.keyring_enabled) {
            if let (Some(index), Some(previous)) = (index, previous) {
                self.cache.sessions[index] = previous;
            } else {
                self.cache.sessions.pop();
            }
            return Err(error);
        }
        Ok(())
    }

    fn read_keyring_password(id: &str) -> Result<Option<Secret>> {
        match Self::keyring_entry(id)?.get_password() {
            Ok(password) => Ok(Some(Secret::new(password))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn restore_keyring_password(id: &str, previous: Option<&Secret>) -> Result<()> {
        let expected = previous.map(Secret::as_str);
        let matches = |current: &Option<Secret>| current.as_ref().map(Secret::as_str) == expected;
        if Self::read_keyring_password(id).as_ref().is_ok_and(matches) {
            return Ok(());
        }
        match expected {
            Some(password) => Self::keyring_entry(id)?.set_password(password)?,
            None => Self::keyring_set_password(id, "")?,
        }
        if !matches(&Self::read_keyring_password(id)?) {
            bail!("could not verify the original keyring credential after a failed save");
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &str) {
        self.cache.sessions.retain(|s| s.id != id);
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.cache.sessions.iter().find(|s| s.id == id)
    }

    pub fn download_dir(&self) -> &str {
        &self.cache.download_dir
    }

    pub fn set_download_dir(&mut self, dir: String) {
        self.cache.download_dir = dir;
    }

    /// UI language code ("zh" default / "en").
    pub fn language(&self) -> &str {
        if self.cache.language.is_empty() {
            "zh"
        } else {
            &self.cache.language
        }
    }

    pub fn set_language(&mut self, lang: String) {
        self.cache.language = lang;
    }

    /// Theme preference: "system" (default) | "dark" | "light".
    pub fn theme_pref(&self) -> &str {
        if self.cache.theme_pref.is_empty() {
            "system"
        } else {
            &self.cache.theme_pref
        }
    }

    pub fn set_theme_pref(&mut self, pref: String) {
        self.cache.theme_pref = pref;
    }

    /// Renderer preference for the current platform.
    #[cfg(target_os = "macos")]
    pub fn renderer_mode(&self) -> &str {
        normalize_macos_renderer_mode(&self.cache.renderer_mode)
    }

    /// Missing and invalid Windows values deliberately use software so upgrades
    /// preserve the high-DPI/VM compatibility from #224.
    #[cfg(target_os = "windows")]
    pub fn renderer_mode(&self) -> &str {
        match self.cache.renderer_mode.as_str() {
            "auto" => "auto",
            "gpu" => "gpu",
            _ => "software",
        }
    }

    #[cfg(target_os = "macos")]
    pub fn set_renderer_mode(&mut self, mode: String) {
        self.cache.renderer_mode = normalize_macos_renderer_mode(&mode).into();
    }

    /// Linux has no renderer settings entry, so the choice stays with the
    /// platform's automatic renderer selection. Keep that behaviour for
    /// existing configurations.
    #[cfg(target_os = "linux")]
    pub fn renderer_mode(&self) -> &str {
        match self.cache.renderer_mode.as_str() {
            "gpu" => "gpu",
            "software" => "software",
            _ => "auto",
        }
    }

    #[cfg(target_os = "windows")]
    pub fn set_renderer_mode(&mut self, mode: String) {
        self.cache.renderer_mode = match mode.as_str() {
            "auto" => "auto".into(),
            "gpu" => "gpu".into(),
            _ => "software".into(),
        };
    }

    #[cfg(target_os = "linux")]
    pub fn set_renderer_mode(&mut self, mode: String) {
        self.cache.renderer_mode = match mode.as_str() {
            "gpu" => "gpu".into(),
            "software" => "software".into(),
            _ => "auto".into(),
        };
    }

    /// Terminal font family ("" = built-in default).
    pub fn font_family(&self) -> &str {
        &self.cache.font_family
    }

    pub fn set_font_family(&mut self, family: String) {
        self.cache.font_family = family;
    }

    /// Terminal font size in px (falls back to 13 when unset).
    pub fn font_size(&self) -> u32 {
        if self.cache.font_size == 0 {
            13
        } else {
            self.cache.font_size
        }
    }

    pub fn set_font_size(&mut self, size: u32) {
        self.cache.font_size = size.clamp(8, 32);
    }

    pub fn terminal_line_spacing(&self) -> f32 {
        let value = self.cache.terminal_line_spacing;
        if value <= 0.0 {
            1.0
        } else {
            value.clamp(0.8, 1.5)
        }
    }

    pub fn set_terminal_line_spacing(&mut self, value: f32) {
        self.cache.terminal_line_spacing = value.clamp(0.8, 1.5);
    }

    /// Whether the terminal grid is inset from its pane's edge. Missing or
    /// legacy config means on: a flush grid is the old look, not the default.
    pub fn terminal_padding(&self) -> bool {
        self.cache.terminal_padding
    }

    pub fn set_terminal_padding(&mut self, value: bool) {
        self.cache.terminal_padding = value;
    }

    pub fn paste_confirm_enabled(&self) -> bool {
        !self.cache.paste_confirm_disabled
    }

    pub fn set_paste_confirm_enabled(&mut self, enabled: bool) {
        self.cache.paste_confirm_disabled = !enabled;
    }

    pub fn extra_paste_shortcuts_enabled(&self) -> bool {
        !self.cache.extra_paste_shortcuts_disabled
    }

    pub fn set_extra_paste_shortcuts_enabled(&mut self, enabled: bool) {
        self.cache.extra_paste_shortcuts_disabled = !enabled;
    }

    pub fn zen_mode(&self) -> bool {
        self.cache.zen_mode
    }

    pub fn set_zen_mode(&mut self, enabled: bool) {
        self.cache.zen_mode = enabled;
    }

    /// Force regular terminal text to render with a bold face (#262).
    pub fn terminal_bold(&self) -> bool {
        self.cache.terminal_bold
    }

    pub fn set_terminal_bold(&mut self, bold: bool) {
        self.cache.terminal_bold = bold;
    }

    /// Selected terminal insertion cursor shape. Legacy and invalid values use
    /// the existing block cursor so upgrades preserve the current appearance.
    pub fn terminal_cursor_style(&self) -> &str {
        match self.cache.terminal_cursor_style.as_str() {
            "bar" => "bar",
            "underline" => "underline",
            _ => "block",
        }
    }

    pub fn set_terminal_cursor_style(&mut self, style: String) {
        self.cache.terminal_cursor_style = match style.as_str() {
            "bar" => "bar".into(),
            "underline" => "underline".into(),
            _ => "block".into(),
        };
    }

    pub fn terminal_cursor_color(&self) -> &str {
        if normalize_hex_color(&self.cache.terminal_cursor_color).is_some() {
            &self.cache.terminal_cursor_color
        } else {
            ""
        }
    }

    pub fn set_terminal_cursor_color(&mut self, color: &str) -> bool {
        let Some(normalized) = normalize_hex_color(color) else {
            return false;
        };
        self.cache.terminal_cursor_color = normalized;
        true
    }

    /// Whether client-side highlighting of otherwise unstyled output is active.
    pub fn output_highlight_enabled(&self) -> bool {
        !self.cache.output_highlight_disabled
    }

    pub fn set_output_highlight_enabled(&mut self, enabled: bool) {
        self.cache.output_highlight_disabled = !enabled;
    }

    pub fn json_format_output(&self) -> bool {
        !self.cache.json_format_disabled
    }

    pub fn set_json_format_output(&mut self, enabled: bool) {
        self.cache.json_format_disabled = !enabled;
    }

    /// Selected built-in rule set. Unknown values safely fall back to the
    /// conservative log-level preset for forward/backward compatibility.
    pub fn output_highlight_preset(&self) -> &str {
        match self.cache.output_highlight_preset.as_str() {
            "devops" => "devops",
            _ => "log",
        }
    }

    pub fn set_output_highlight_preset(&mut self, preset: String) {
        self.cache.output_highlight_preset = match preset.as_str() {
            "devops" => "devops".to_string(),
            _ => "log".to_string(),
        };
    }

    pub fn output_highlight_rules(&self) -> &[OutputHighlightRule] {
        &self.cache.output_highlight_rules
    }

    pub fn add_output_highlight_rule(&mut self, mut rule: OutputHighlightRule) {
        rule.pattern = rule.pattern.trim().to_string();
        rule.color = normalize_highlight_color(&rule.color).to_string();
        self.cache.output_highlight_rules.push(rule);
    }

    pub fn remove_output_highlight_rule(&mut self, index: usize) {
        if index < self.cache.output_highlight_rules.len() {
            self.cache.output_highlight_rules.remove(index);
        }
    }

    pub fn set_output_highlight_rule_enabled(&mut self, index: usize, enabled: bool) {
        if let Some(rule) = self.cache.output_highlight_rules.get_mut(index) {
            rule.enabled = enabled;
        }
    }

    /// Global UI scale in percent (#100). Defaults to 100.
    pub fn ui_scale(&self) -> u32 {
        if self.cache.ui_scale == 0 {
            100
        } else {
            self.cache.ui_scale
        }
    }

    pub fn set_ui_scale(&mut self, percent: u32) {
        self.cache.ui_scale = percent.clamp(80, 200);
    }

    /// Immersive wallpaper id ("" = none).
    pub fn wallpaper(&self) -> &str {
        &self.cache.wallpaper
    }

    pub fn set_wallpaper(&mut self, id: impl Into<String>) {
        self.cache.wallpaper = id.into();
    }

    /// Whether the SFTP panel follows the terminal's cd (default true).
    pub fn sftp_follow_cd(&self) -> bool {
        !self.cache.sftp_no_follow_cd
    }

    pub fn set_sftp_follow_cd(&mut self, follow: bool) {
        self.cache.sftp_no_follow_cd = !follow;
    }

    /// Whether the quick-command bar under the terminal is hidden.
    pub fn cmd_bar_hidden(&self) -> bool {
        self.cache.hide_cmd_bar
    }

    pub fn set_cmd_bar_hidden(&mut self, hidden: bool) {
        self.cache.hide_cmd_bar = hidden;
    }

    /// Saved quick commands (#55).
    pub fn quick_commands(&self) -> &[QuickCommand] {
        &self.cache.quick_commands
    }

    pub fn set_quick_commands(&mut self, cmds: Vec<QuickCommand>) {
        self.cache.quick_commands = cmds;
    }

    pub fn wsl_profiles(&self) -> &[WslProfile] {
        &self.cache.wsl_profiles
    }

    pub fn add_wsl_profile(&mut self, name: String, distribution: String, directory: String) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.cache.wsl_profiles.push(WslProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            distribution: distribution.trim().to_string(),
            directory: match directory.trim() {
                "" => "~".to_string(),
                value => value.to_string(),
            },
        });
    }

    pub fn remove_wsl_profile(&mut self, id: &str) {
        self.cache.wsl_profiles.retain(|profile| profile.id != id);
    }

    pub fn quick_panel_open(&self) -> bool {
        self.cache.quick_panel_open
    }

    pub fn quick_commands_as_sidebar(&self) -> bool {
        self.cache.quick_commands_as_sidebar
    }

    pub fn set_quick_commands_as_sidebar(&mut self, enabled: bool) {
        self.cache.quick_commands_as_sidebar = enabled;
        if !enabled {
            self.cache.quick_panel_open = false;
        }
    }

    pub fn set_quick_panel_open(&mut self, open: bool) {
        self.cache.quick_panel_open = open;
    }

    pub fn quick_panel_collapsed(&self) -> bool {
        self.cache.quick_panel_collapsed
    }

    pub fn set_quick_panel_collapsed(&mut self, collapsed: bool) {
        self.cache.quick_panel_collapsed = collapsed;
    }

    pub fn quick_panel_width(&self) -> f32 {
        let width = self.cache.quick_panel_width;
        if width <= 0.0 {
            default_quick_panel_width()
        } else {
            width
        }
    }

    pub fn set_quick_panel_width(&mut self, width: f32) {
        self.cache.quick_panel_width = width;
    }

    pub fn quick_panel_height(&self) -> f32 {
        let height = self.cache.quick_panel_height;
        if height <= 0.0 {
            default_quick_panel_height()
        } else {
            // Legacy configs may hold an unclamped value from before the
            // setter had bounds.
            height.clamp(120.0, 600.0)
        }
    }

    pub fn set_quick_panel_height(&mut self, height: f32) {
        // The same bounds the drag gesture clamps to. The settings field had
        // no floor: a 0 saved here mounted a dock with no rows at all, and a
        // 9999 ate the terminal.
        self.cache.quick_panel_height = height.clamp(120.0, 600.0);
    }

    pub fn quick_panel_dock(&self) -> String {
        match self.cache.quick_panel_dock.trim() {
            "left" | "right" | "top" | "bottom" => self.cache.quick_panel_dock.clone(),
            _ => "right".into(),
        }
    }

    pub fn set_quick_panel_dock(&mut self, dock: String) {
        self.cache.quick_panel_dock = dock;
    }

    /// True when the file panel is docked to the right edge rather than the bottom one.
    ///
    /// A boolean rather than the string, because that is the only question the layout asks,
    /// and both callers — the strip's shape and its border — ask the same one.
    pub fn sftp_panel_on_the_right(&self) -> bool {
        self.cache.sftp_panel_dock.trim() == "right"
    }

    pub fn set_sftp_panel_dock(&mut self, dock: String) {
        self.cache.sftp_panel_dock = dock;
    }

    /// Explicit quick-command groups (#55) — parallels [`groups`](Self::groups).
    pub fn quick_groups(&self) -> &[String] {
        &self.cache.quick_groups
    }

    /// Create an empty quick-command group. Ignores blank, "default", duplicates.
    pub fn add_quick_group(&mut self, name: String) {
        let n = name.trim().to_string();
        if n.is_empty() || n.eq_ignore_ascii_case("default") {
            return;
        }
        if !self.cache.quick_groups.iter().any(|g| g == &n) {
            self.cache.quick_groups.push(n);
        }
    }

    /// Delete a quick-command group; any command still in it falls back to
    /// ungrouped (the UI only offers delete on empty groups, but clear defensively).
    pub fn remove_quick_group(&mut self, name: &str) {
        self.cache.quick_groups.retain(|g| g != name);
        for c in &mut self.cache.quick_commands {
            if c.group == name {
                c.group.clear();
            }
        }
    }

    /// Rename a quick-command group, moving its commands along. No-op for
    /// blank / "default".
    pub fn rename_quick_group(&mut self, old: &str, new: String) {
        let n = new.trim().to_string();
        if n.is_empty() || n.eq_ignore_ascii_case("default") || n == old {
            return;
        }
        for g in &mut self.cache.quick_groups {
            if g == old {
                *g = n.clone();
            }
        }
        for c in &mut self.cache.quick_commands {
            if c.group == old {
                c.group = n.clone();
            }
        }
        self.cache.quick_groups.sort();
        self.cache.quick_groups.dedup();
    }

    /// Update one quick command in place by index (#55).
    pub fn update_quick_command(&mut self, index: usize, cmd: QuickCommand) {
        if let Some(slot) = self.cache.quick_commands.get_mut(index) {
            *slot = cmd;
        }
    }

    /// Recent command-box history, oldest first (#55).
    pub fn command_history(&self) -> &[String] {
        &self.cache.command_history
    }

    /// Append a command to the history: skips blanks, de-duplicates globally so
    /// each command appears once, and re-appends at the end so the most-recently
    /// used command is always last. Capped so it can't grow without bound (#113).
    pub fn push_command_history(&mut self, cmd: String) {
        if cmd.trim().is_empty() {
            return;
        }
        // Drop any earlier occurrence, then push → no duplicates and "last used"
        // moves to the end (bash `HISTCONTROL=erasedups` semantics).
        self.cache.command_history.retain(|c| c != &cmd);
        const CAP: usize = 200;
        self.cache.command_history.push(cmd);
        let len = self.cache.command_history.len();
        if len > CAP {
            self.cache.command_history.drain(0..len - CAP);
        }
    }

    /// Remove a single command-history entry by storage index (#96).
    pub fn remove_command_history(&mut self, index: usize) {
        if index < self.cache.command_history.len() {
            self.cache.command_history.remove(index);
        }
    }

    /// Forget every command the box has run.
    ///
    /// A method rather than `remove_command_history` in a loop, because the loop is wrong
    /// the moment the caller iterates forwards — each removal shifts the indices after it,
    /// which is how half a history survives being cleared.
    pub fn clear_command_history(&mut self) {
        self.cache.command_history.clear();
    }

    /// Collapse the resource sidebar on startup (default false) (#78).
    pub fn collapse_sidebar_default(&self) -> bool {
        self.cache.collapse_sidebar_default
    }

    pub fn set_collapse_sidebar_default(&mut self, v: bool) {
        self.cache.collapse_sidebar_default = v;
    }

    /// Persisted sidebar width in logical px. Falls back to the default when the
    /// stored value is unset/zero (e.g. a config created via `Default`).
    pub fn sidebar_width(&self) -> f32 {
        let w = self.cache.sidebar_width;
        if w <= 0.0 {
            default_sidebar_width()
        } else {
            w
        }
    }

    pub fn set_sidebar_width(&mut self, v: f32) {
        self.cache.sidebar_width = v;
    }

    /// Resource / SFTP panel docking geometry, persisted across restarts (#dock).
    /// Sizes fall back to their defaults when unset/zero; docks fall back to a
    /// sensible edge when the stored string is empty.
    pub fn sidebar_height(&self) -> f32 {
        let h = self.cache.sidebar_height;
        if h <= 0.0 {
            default_sidebar_height()
        } else {
            h
        }
    }
    pub fn set_sidebar_height(&mut self, v: f32) {
        self.cache.sidebar_height = v;
    }
    pub fn sidebar_dock(&self) -> String {
        let d = self.cache.sidebar_dock.trim();
        if d.is_empty() {
            "left".into()
        } else {
            d.to_string()
        }
    }
    pub fn set_sidebar_dock(&mut self, v: String) {
        self.cache.sidebar_dock = v;
    }
    pub fn sidebar_collapsed(&self) -> Option<bool> {
        self.cache.sidebar_collapsed
    }
    pub fn set_sidebar_collapsed(&mut self, v: bool) {
        self.cache.sidebar_collapsed = Some(v);
    }
    pub fn welcome_as_sidebar(&self) -> bool {
        self.cache.welcome_as_sidebar
    }
    pub fn set_welcome_as_sidebar(&mut self, v: bool) {
        self.cache.welcome_as_sidebar = v;
    }
    pub fn welcome_sidebar_width(&self) -> f32 {
        let w = self.cache.welcome_sidebar_width;
        if w <= 0.0 {
            240.0
        } else {
            w
        }
    }
    pub fn set_welcome_sidebar_width(&mut self, v: f32) {
        self.cache.welcome_sidebar_width = v;
    }
    pub fn welcome_sidebar_dock(&self) -> String {
        let d = self.cache.welcome_sidebar_dock.trim();
        if d.is_empty() {
            "left".into()
        } else {
            d.to_string()
        }
    }
    pub fn set_welcome_sidebar_dock(&mut self, v: String) {
        self.cache.welcome_sidebar_dock = v;
    }
    pub fn welcome_collapsed(&self) -> Option<bool> {
        self.cache.welcome_collapsed
    }
    pub fn set_welcome_collapsed(&mut self, v: bool) {
        self.cache.welcome_collapsed = Some(v);
    }
    /// Whether the startup new-version check is enabled (#184).
    pub fn update_check_enabled(&self) -> bool {
        !self.cache.update_check_disabled
    }
    pub fn set_update_check_enabled(&mut self, enabled: bool) {
        self.cache.update_check_disabled = !enabled;
    }
    pub fn mcp_enabled(&self) -> bool {
        self.cache.mcp_enabled
    }
    pub fn set_mcp_enabled(&mut self, enabled: bool) {
        self.cache.mcp_enabled = enabled;
    }
    pub fn mcp_use_saved_credentials(&self) -> bool {
        self.cache.mcp_use_saved_credentials
    }
    pub fn set_mcp_use_saved_credentials(&mut self, enabled: bool) {
        self.cache.mcp_use_saved_credentials = enabled;
    }
    pub fn mcp_allow_commands(&self) -> bool {
        self.cache.mcp_allow_commands
    }
    pub fn set_mcp_allow_commands(&mut self, enabled: bool) {
        self.cache.mcp_allow_commands = enabled;
    }
    pub fn mcp_allow_file_transfers(&self) -> bool {
        self.cache.mcp_allow_file_transfers
    }
    pub fn set_mcp_allow_file_transfers(&mut self, enabled: bool) {
        self.cache.mcp_allow_file_transfers = enabled;
    }
    /// Whether risky MCP commands need a human's approval at the main window
    /// before they run. On by default; the setter exists for the settings page.
    pub fn mcp_approval_enabled(&self) -> bool {
        self.cache.mcp_approval_enabled
    }
    pub fn set_mcp_approval_enabled(&mut self, enabled: bool) {
        self.cache.mcp_approval_enabled = enabled;
    }
    /// Seconds an approval request waits before the command is refused.
    pub fn mcp_approval_timeout_secs(&self) -> u64 {
        let v = self.cache.mcp_approval_timeout_secs;
        // 0 would mean "never approvable"; clamp to something a human can win.
        v.clamp(10, 600)
    }
    pub fn set_mcp_approval_timeout_secs(&mut self, secs: u64) {
        self.cache.mcp_approval_timeout_secs = secs.clamp(10, 600);
    }
    /// The command patterns the approval gate matches, case-insensitively.
    pub fn mcp_risky_patterns(&self) -> Vec<String> {
        self.cache.mcp_risky_patterns.clone()
    }
    pub fn set_mcp_risky_patterns(&mut self, patterns: Vec<String>) {
        self.cache.mcp_risky_patterns = patterns;
    }
    /// The path prefixes the approval gate watches.
    pub fn mcp_risky_dirs(&self) -> Vec<String> {
        self.cache.mcp_risky_dirs.clone()
    }
    pub fn set_mcp_risky_dirs(&mut self, dirs: Vec<String>) {
        self.cache.mcp_risky_dirs = dirs;
    }
    /// Audit-journal retention, in whole days (7-365; 30 by default).
    pub fn mcp_audit_retention_days(&self) -> u64 {
        self.cache.mcp_audit_retention_days.clamp(7, 365)
    }
    pub fn set_mcp_audit_retention_days(&mut self, days: u64) {
        self.cache.mcp_audit_retention_days = days.clamp(7, 365);
    }
    pub fn wallpaper_overlay(&self) -> f32 {
        let a = self.cache.wallpaper_overlay;
        // Floor lowered 0.40 -> 0.30 so more see-through panels are reachable.
        if a <= 0.0 {
            DEFAULT_WALLPAPER_OVERLAY
        } else {
            a.clamp(0.30, 1.0)
        }
    }
    pub fn set_wallpaper_overlay(&mut self, v: f32) {
        self.cache.wallpaper_overlay = v.clamp(0.30, 1.0);
    }
    pub fn panel_font(&self) -> u32 {
        if self.cache.panel_font == 0 {
            100
        } else {
            self.cache.panel_font
        }
    }
    pub fn set_panel_font(&mut self, percent: u32) {
        self.cache.panel_font = percent.clamp(80, 160);
    }
    pub fn sftp_panel_width(&self) -> f32 {
        let w = self.cache.sftp_panel_width;
        if w <= 0.0 {
            default_sftp_width()
        } else {
            w
        }
    }
    pub fn set_sftp_panel_width(&mut self, v: f32) {
        self.cache.sftp_panel_width = v;
    }
    pub fn sftp_panel_height(&self) -> f32 {
        let h = self.cache.sftp_panel_height;
        if h <= 0.0 {
            default_sftp_height()
        } else {
            h
        }
    }
    pub fn set_sftp_panel_height(&mut self, v: f32) {
        self.cache.sftp_panel_height = v;
    }
    pub fn sftp_tree_width(&self) -> f32 {
        let width = self.cache.sftp_tree_width;
        if width <= 0.0 {
            default_sftp_tree_width()
        } else {
            width.clamp(120.0, 420.0)
        }
    }
    pub fn set_sftp_tree_width(&mut self, width: f32) {
        self.cache.sftp_tree_width = width.clamp(120.0, 420.0);
    }
    pub fn sftp_dock(&self) -> String {
        let d = self.cache.sftp_dock.trim();
        if d.is_empty() {
            "bottom".into()
        } else {
            d.to_string()
        }
    }
    pub fn set_sftp_dock(&mut self, v: String) {
        self.cache.sftp_dock = v;
    }
    /// Last window size in logical px; `(0,0)` means unset (use the default).
    pub fn window_size(&self) -> (f32, f32) {
        (self.cache.window_width, self.cache.window_height)
    }
    pub fn set_window_size(&mut self, w: f32, h: f32) {
        self.cache.window_width = w;
        self.cache.window_height = h;
    }

    /// Collapse the SFTP panel on startup (default false) (#78).
    pub fn collapse_sftp_default(&self) -> bool {
        self.cache.collapse_sftp_default
    }

    pub fn set_collapse_sftp_default(&mut self, v: bool) {
        self.cache.collapse_sftp_default = v;
    }

    /// Mirror SFTP uploads to other sessions while session-sync is on (default
    /// false). Only has effect when the session-sync toggle is on.
    pub fn sync_upload(&self) -> bool {
        self.cache.sync_upload
    }

    pub fn set_sync_upload(&mut self, v: bool) {
        self.cache.sync_upload = v;
    }

    pub fn webdav_enabled(&self) -> bool {
        self.cache.webdav_enabled
    }

    pub fn webdav_url(&self) -> &str {
        &self.cache.webdav_url
    }

    pub fn webdav_username(&self) -> &str {
        &self.cache.webdav_username
    }

    pub fn webdav_password(&self) -> &str {
        self.cache.webdav_password.as_str()
    }

    pub fn webdav_remote_path(&self) -> &str {
        if self.cache.webdav_remote_path.trim().is_empty() {
            "xenterm-connections.json"
        } else {
            &self.cache.webdav_remote_path
        }
    }

    pub fn webdav_accept_invalid_certs(&self) -> bool {
        self.cache.webdav_accept_invalid_certs
    }

    pub fn set_webdav_settings(
        &mut self,
        enabled: bool,
        url: String,
        username: String,
        password: String,
        remote_path: String,
        accept_invalid_certs: bool,
    ) {
        self.cache.webdav_enabled = enabled;
        self.cache.webdav_url = url.trim().trim_end_matches('/').to_string();
        self.cache.webdav_username = username.trim().to_string();
        self.cache.webdav_password = Secret::new(password);
        self.cache.webdav_remote_path = if remote_path.trim().is_empty() {
            "xenterm-connections.json".to_string()
        } else {
            remote_path.trim().trim_start_matches('/').to_string()
        };
        self.cache.webdav_accept_invalid_certs = accept_invalid_certs;
    }

    /// Whether each download prompts for a save location (default false) (#87).
    pub fn download_always_ask(&self) -> bool {
        self.cache.download_always_ask
    }

    pub fn set_download_always_ask(&mut self, ask: bool) {
        self.cache.download_always_ask = ask;
    }

    // ── Session groups / folders (#41) ────────────────────────────────────

    /// Explicit groups (empty folders included). "default" is implicit.
    pub fn groups(&self) -> &[String] {
        &self.cache.groups
    }

    pub fn collapsed_session_groups(&self) -> Option<&[String]> {
        self.cache.collapsed_session_groups.as_deref()
    }

    /// Remember a Quick Connect folder's open/closed state. On the first
    /// interaction, materialise the default-collapsed state for every existing
    /// folder so expanding one folder does not accidentally expand the rest.
    pub fn set_session_group_collapsed(&mut self, name: &str, collapsed: bool) {
        if self.cache.collapsed_session_groups.is_none() {
            let mut groups = vec!["system".to_string()];
            if self
                .cache
                .sessions
                .iter()
                .any(|session| session.group.is_empty())
            {
                groups.push("default".to_string());
            }
            groups.extend(self.cache.groups.iter().cloned());
            groups.extend(
                self.cache
                    .sessions
                    .iter()
                    .filter(|session| !session.group.is_empty())
                    .map(|session| session.group.clone()),
            );
            groups.sort();
            groups.dedup();
            self.cache.collapsed_session_groups = Some(groups);
        }

        let groups = self.cache.collapsed_session_groups.as_mut().unwrap();
        groups.retain(|group| group != name);
        if collapsed {
            groups.push(name.to_string());
            groups.sort();
            groups.dedup();
        }
    }

    /// Whether a user group already exists, including groups inferred from
    /// sessions that were created before explicit group records were added.
    pub fn session_group_exists(&self, name: &str) -> bool {
        let target = name.trim();
        if target.is_empty() {
            return false;
        }
        self.cache
            .groups
            .iter()
            .any(|group| group.trim().eq_ignore_ascii_case(target))
            || self.cache.sessions.iter().any(|session| {
                !session.group.trim().is_empty()
                    && session.group.trim().eq_ignore_ascii_case(target)
            })
    }

    /// Create an empty group. Ignores blank/reserved names and duplicates.
    pub fn add_group(&mut self, name: String) {
        let n = name.trim().to_string();
        if n.is_empty() || is_reserved_session_group(&n) || self.session_group_exists(&n) {
            return;
        }
        self.cache.groups.push(n.clone());
        if let Some(groups) = &mut self.cache.collapsed_session_groups {
            groups.push(n);
            groups.sort();
            groups.dedup();
        }
    }

    /// Delete a group. Any session still in it falls back to ungrouped — the UI
    /// only offers delete on empty groups, but we clear sessions defensively.
    pub fn remove_group(&mut self, name: &str) {
        if is_reserved_session_group(name.trim()) {
            return;
        }
        self.cache.groups.retain(|g| g != name);
        if let Some(groups) = &mut self.cache.collapsed_session_groups {
            groups.retain(|group| group != name);
        }
        for s in &mut self.cache.sessions {
            if s.group == name {
                s.group.clear();
            }
        }
    }

    /// Rename a group, moving its sessions along. No-op for reserved names.
    pub fn rename_group(&mut self, old: &str, new: String) {
        let n = new.trim().to_string();
        if n.is_empty()
            || is_reserved_session_group(old.trim())
            || is_reserved_session_group(&n)
            || n == old
            || (!n.eq_ignore_ascii_case(old) && self.session_group_exists(&n))
        {
            return;
        }
        for g in &mut self.cache.groups {
            if g == old {
                *g = n.clone();
            }
        }
        for s in &mut self.cache.sessions {
            if s.group == old {
                s.group = n.clone();
            }
        }
        if let Some(groups) = &mut self.cache.collapsed_session_groups {
            for group in groups.iter_mut() {
                if group == old {
                    *group = n.clone();
                }
            }
            groups.sort();
            groups.dedup();
        }
        self.cache.groups.sort();
        self.cache.groups.dedup();
    }

    /// The OS keyring entry for a session's password: Windows Credential
    /// Manager, macOS Keychain, or the Linux Secret Service, whichever the
    /// platform offers. The account is the session id, so a rename never
    /// orphans the secret and a duplicate never collides.
    fn keyring_entry(session_id: &str) -> Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new(Self::KEYRING_SERVICE, session_id)
    }

    /// Marker written into sessions.json in place of a password whose value
    /// lives in the OS keyring.
    pub(crate) const KEYRING_MARKER: &'static str = "keyring:v1";

    const KEYRING_SERVICE: &'static str = "xenterm";

    /// The service name before the XenTerm rename. Saved passwords live under
    /// it until first read, which copies them to the new service — a rename
    /// must not orphan stored credentials.
    const LEGACY_KEYRING_SERVICE: &'static str = "meatshell";

    /// A session password held in the keyring comes back as plaintext; a
    /// missing or unreachable entry answers None, and the caller falls back
    /// to the interactive prompt as though no password were saved.
    pub fn keyring_password(&self, session_id: &str) -> Option<String> {
        if !self.keyring_enabled {
            return None;
        }
        match Self::keyring_entry(session_id).and_then(|entry| entry.get_password()) {
            Ok(plain) => Some(plain),
            Err(keyring::Error::NoEntry) => {
                // First read since the rename: the secret is still stored
                // under the pre-rename service. Copy it forward once and
                // answer with it, so the rename is invisible to the user.
                let legacy = keyring::Entry::new(Self::LEGACY_KEYRING_SERVICE, session_id)
                    .and_then(|entry| entry.get_password());
                match legacy {
                    Ok(plain) => {
                        if let Err(error) = Self::keyring_set_password(session_id, &plain) {
                            tracing::warn!("keyring migration for {session_id} failed: {error}");
                        }
                        Some(plain)
                    }
                    Err(_) => None,
                }
            }
            Err(error) => {
                tracing::warn!("keyring read for {session_id} failed: {error}");
                None
            }
        }
    }

    /// Put a session password in the keyring. Best effort: a platform without
    /// a usable keyring answers Err and the caller falls back to the file's
    /// own encryption.
    fn keyring_set_password(session_id: &str, plain: &str) -> Result<(), keyring::Error> {
        let entry = Self::keyring_entry(session_id)?;
        if plain.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error),
            }
        } else {
            entry.set_password(plain)
        }
    }

    // ── SQLite persistence (sessions.db) ──────────────────────────────────

    /// Layout version of sessions.db, stored in `meta`.
    const SCHEMA_VERSION: i64 = 1;

    /// Schema of sessions.db. `sessions.data` holds one session's JSON in
    /// disk form; `meta.settings` holds the whole ConfigFile minus sessions
    /// and history — the same shape as the old sessions.json.
    const SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (ordinal INTEGER NOT NULL, id TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS command_history (seq INTEGER PRIMARY KEY AUTOINCREMENT, command TEXT NOT NULL);";

    /// Open the store database with the pragmas the config cares about and
    /// make sure the schema exists. WAL keeps MCP/automation readers
    /// unblocked while the UI writes; `synchronous=FULL` costs a few ms on a
    /// rare write and keeps the config intact through a power cut.
    fn open_db(path: &Path) -> Result<rusqlite::Connection> {
        let conn = rusqlite::Connection::open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("failed to set the sqlite busy timeout")?;
        // journal_mode answers a row ("wal"), so it goes through query_row.
        conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .context("failed to switch the config database to WAL mode")?;
        conn.pragma_update(None, "synchronous", "FULL")
            .context("failed to make the config database fully synchronous")?;
        conn.execute_batch(Self::SCHEMA_SQL)
            .context("failed to initialise the config database schema")?;
        Ok(conn)
    }

    /// Existing snapshots never initialize or replace their destination.
    /// Schema/row validation happens before any persistence work.
    fn open_existing_db(path: &Path) -> Result<rusqlite::Connection> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .context("failed to open existing configuration")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        Ok(conn)
    }

    /// Read the disk-form store out of the database: the settings blob (its
    /// secrets still encrypted), sessions in disk form, and the history.
    /// `None` means the database was never written — created and abandoned,
    /// or brand new.
    fn read_disk_store(conn: &rusqlite::Connection) -> Result<Option<DiskStore>> {
        let settings: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(settings) = settings else {
            return Ok(None);
        };
        let mut stmt = conn.prepare("SELECT data FROM sessions ORDER BY ordinal, id")?;
        let sessions = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?
            .into_iter()
            .map(|raw| serde_json::from_str::<Session>(&raw))
            .collect::<std::result::Result<Vec<Session>, _>>()?;
        let mut stmt = conn.prepare(
            "SELECT command FROM \
             (SELECT command, seq FROM command_history ORDER BY seq DESC LIMIT 200) \
             ORDER BY seq",
        )?;
        let history = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(Some((settings, sessions, history)))
    }

    /// Write the planned slice of `cache`. One transaction: a half-written
    /// config is worse than an unwritten one.
    fn write_store(
        tx: &rusqlite::Transaction<'_>,
        cache: &ConfigFile,
        plan: &SavePlan,
        key: [u8; 32],
        credentials: &std::collections::HashMap<String, Option<Secret>>,
    ) -> Result<()> {
        if plan.settings {
            // The settings blob keeps the secret treatment the JSON file had:
            // everything plaintext except the WebDAV password.
            let mut settings = cache.clone();
            settings.sessions = Vec::new();
            settings.command_history = Vec::new();
            if !settings.webdav_password.is_empty()
                && !settings.webdav_password.is_local_ciphertext()
            {
                let enc = Self::encrypt(&key, settings.webdav_password.as_str())?;
                settings.webdav_password = Secret::new(enc);
            }
            let blob = serde_json::to_string(&settings)?;
            for (k, v) in [
                ("schema_version", Self::SCHEMA_VERSION.to_string()),
                ("settings", blob),
            ] {
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES(?1, ?2) \
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![k, v],
                )?;
            }
        }
        if plan.all_sessions {
            tx.execute("DELETE FROM sessions", [])?;
            for (ordinal, session) in cache.sessions.iter().enumerate() {
                Self::upsert_session_row(
                    tx,
                    ordinal,
                    session,
                    key,
                    credentials.contains_key(&session.id),
                )?;
            }
        } else {
            for id in &plan.sessions {
                match cache.sessions.iter().position(|s| &s.id == id) {
                    Some(ordinal) => Self::upsert_session_row(
                        &tx,
                        ordinal,
                        &cache.sessions[ordinal],
                        key,
                        credentials.contains_key(id),
                    )?,
                    // A planned id that is no longer in the cache was removed.
                    None => {
                        tx.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
                    }
                }
            }
        }
        if plan.history {
            tx.execute("DELETE FROM command_history", [])?;
            for command in &cache.command_history {
                tx.execute("INSERT INTO command_history(command) VALUES(?1)", [command])?;
            }
        }
        Ok(())
    }

    /// Insert or update one session row. `ordinal` is the position in the
    /// display order, so a reordered store only ever rewrites ordinals.
    fn upsert_session_row(
        tx: &rusqlite::Transaction<'_>,
        ordinal: usize,
        session: &Session,
        key: [u8; 32],
        keyring_enabled: bool,
    ) -> Result<()> {
        let mut disk = session.clone();
        Self::session_to_disk_form(&mut disk, &key, keyring_enabled)?;
        tx.execute(
            "INSERT INTO sessions(ordinal, id, data) VALUES(?1, ?2, ?3) \
             ON CONFLICT(id) DO UPDATE SET ordinal = excluded.ordinal, data = excluded.data",
            rusqlite::params![ordinal as i64, disk.id, serde_json::to_string(&disk)?],
        )?;
        Ok(())
    }

    /// Transform one session into its on-disk form: the password moves to the
    /// OS keyring when possible (the row then carries only the `keyring:v1`
    /// marker), and every other secret is encrypted with the master key. The
    /// in-memory cache keeps plaintext either way, so a connect never reads
    /// the marker by accident.
    fn session_to_disk_form(
        session: &mut Session,
        key: &[u8; 32],
        keyring_enabled: bool,
    ) -> Result<()> {
        if !session.password.is_empty() && !session.password.is_local_ciphertext() {
            let mut stored_in_keyring = false;
            if keyring_enabled {
                match Self::keyring_set_password(&session.id, session.password.as_str()) {
                    Ok(()) => {
                        session.password = Secret::new(Self::KEYRING_MARKER.to_string());
                        stored_in_keyring = true;
                    }
                    Err(error) => tracing::warn!(
                        "keyring write for {} failed; falling back to encryption: {error}",
                        session.id
                    ),
                }
            }
            if !stored_in_keyring {
                let enc = Self::encrypt(key, session.password.as_str())?;
                session.password = Secret::new(enc);
            }
        }
        if !session.private_key_inline.is_empty()
            && !session.private_key_inline.is_local_ciphertext()
        {
            let enc = Self::encrypt(key, session.private_key_inline.as_str())?;
            session.private_key_inline = Secret::new(enc);
        }
        for trigger in &mut session.triggers {
            if !trigger.response.is_empty() && !trigger.response.is_local_ciphertext() {
                let enc = Self::encrypt(key, trigger.response.as_str())?;
                trigger.response = Secret::new(enc);
            }
        }
        // The proxy URL's embedded password is a credential like the others.
        // Cache values are plaintext, including literal encryption-like prefixes.
        if !session.proxy.is_empty() {
            if let Some(enc) =
                Self::map_proxy_password(&session.proxy, |pass| Self::encrypt(key, pass).ok())
            {
                session.proxy = enc;
            }
        }
        Ok(())
    }

    /// The reverse of [`Self::session_to_disk_form`]: resolve keyring markers
    /// and decrypt every secret back to what the connect path needs. Values
    /// that aren't blobs (legacy plaintext) pass through untouched; a vanished
    /// keyring entry degrades to "no saved password" — the interactive prompt
    /// takes over, which is what an unsaved password means.
    fn session_from_disk_form(session: &mut Session, key: &[u8; 32]) {
        if session.password.as_str() == Self::KEYRING_MARKER {
            let credential = if !cfg!(feature = "desktop") || has_explicit_data_dir() {
                Err(keyring::Error::NoEntry)
            } else {
                Self::keyring_entry(&session.id).and_then(|entry| entry.get_password())
            };
            match credential {
                Ok(plain) => session.password = Secret::new(plain),
                Err(error) => {
                    tracing::warn!(
                        "keyring read for {} failed; the session will prompt: {error}",
                        session.id
                    );
                    session.password = Secret::default();
                }
            }
        } else if let Some(plain) = Self::try_decrypt(key, session.password.as_str()) {
            session.password = Secret::new(plain);
        }
        if let Some(plain) = Self::try_decrypt(key, session.private_key_inline.as_str()) {
            session.private_key_inline = Secret::new(plain);
        }
        for trigger in &mut session.triggers {
            if let Some(plain) = Self::try_decrypt(key, trigger.response.as_str()) {
                trigger.response = Secret::new(plain);
            }
        }
        if !session.proxy.is_empty() {
            if let Some(url) =
                Self::map_proxy_password(&session.proxy, |pass| Self::try_decrypt(key, pass))
            {
                session.proxy = url;
            }
        }
    }

    /// Read the legacy sessions.json (disk form) and resolve every secret
    /// back to plaintext. Used by the one-time JSON→SQLite migration and by
    /// the legacy backup-restore path.
    fn read_json_store(path: &Path, key: &[u8; 32]) -> Result<ConfigFile> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut cfg: ConfigFile =
            serde_json::from_str(&raw).with_context(|| "not a valid config document")?;
        for session in &mut cfg.sessions {
            Self::session_from_disk_form(session, key);
        }
        if let Some(plain) = Self::try_decrypt(key, cfg.webdav_password.as_str()) {
            cfg.webdav_password = Secret::new(plain);
        }
        Ok(cfg)
    }

    // ── Saving ────────────────────────────────────────────────────────────

    pub fn save(&self) -> Result<()> {
        self.save_impl(None, self.keyring_enabled)
    }

    /// Save with the diff plan forced to "everything" — for one-time
    /// migrations whose cache changed semantically without necessarily
    /// changing per-row content.
    fn save_all(&self) -> Result<()> {
        self.save_impl(Some(SavePlan::all()), self.keyring_enabled)
    }

    fn save_impl(&self, forced: Option<SavePlan>, keyring_enabled: bool) -> Result<()> {
        let _ordered = Self::save_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.save_impl_locked(forced, keyring_enabled)
    }

    /// The caller holds `save_lock`, including during saved-state bookkeeping.
    fn save_impl_locked(&self, forced: Option<SavePlan>, keyring_enabled: bool) -> Result<()> {
        let mut saved = {
            let mut shared = self.saved_state.lock().unwrap_or_else(|p| p.into_inner());
            shared.submitted += 1;
            let mut saved = shared.clone();
            saved.attempted = shared.submitted;
            saved
        };
        let result = Self::persist_snapshot(
            &self.cache,
            &mut saved,
            forced,
            self.key,
            &self.path,
            self.backup_dir.as_deref(),
            keyring_enabled,
        );
        if let Err(ref error) = result {
            Self::record_save_error(&mut saved, error);
        }
        Self::publish_snapshot(&self.saved_state, saved);
        result
    }

    /// Diff the cache against what was last written: which rows would differ
    /// on disk. Pure, and the reason saves stay cheap — see [`SavedState`].
    fn plan_save(cache: &ConfigFile, saved: &SavedState) -> SavePlan {
        let mut plan = SavePlan {
            settings: settings_blob(cache) != saved.settings,
            all_sessions: false,
            sessions: Vec::new(),
            history: serde_json::to_string(&cache.command_history).unwrap_or_default()
                != saved.history,
        };
        if session_order(cache) != saved.order {
            // The display order changed, so every row's ordinal is stale:
            // rewrite them all (and reconcile deletions) in one pass.
            plan.all_sessions = true;
        } else {
            for session in &cache.sessions {
                let raw = serde_json::to_string(session).unwrap_or_default();
                if saved.sessions.get(&session.id).map(String::as_str) != Some(raw.as_str()) {
                    plan.sessions.push(session.id.clone());
                }
            }
        }
        // Sessions that were deleted from the cache still have rows on disk.
        for id in saved.sessions.keys() {
            if !cache.sessions.iter().any(|s| &s.id == id) {
                plan.sessions.push(id.clone());
            }
        }
        plan
    }

    /// Queue a snapshot without blocking the UI. The shared commit state and
    /// submission counter reject out-of-order tasks and surface failures.
    pub fn save_in_background(&self) {
        let sequence = {
            let mut saved = self.saved_state.lock().unwrap_or_else(|p| p.into_inner());
            saved.submitted += 1;
            saved.submitted
        };
        let cache = self.cache.clone();
        let shared = self.saved_state.clone();
        let key = self.key;
        let path = self.path.clone();
        let backup_dir = self.backup_dir.clone();
        let keyring_enabled = self.keyring_enabled;
        std::thread::spawn(move || {
            Self::run_background_save(
                cache,
                sequence,
                shared,
                key,
                path,
                backup_dir,
                keyring_enabled,
            )
        });
    }

    /// Orders process-local keyring and SQLite work. Background submission
    /// order is separately enforced by the shared persistence generations.
    fn save_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Mirror the just-written database into the user config backup dir, when
    /// one is configured, along with the key and known-hosts beside it. The
    /// mirror is a consistent `VACUUM INTO` snapshot — safe while the WAL is
    /// live — written to a temp name and renamed over the previous one.
    fn sync_backup_to(backup_dir: Option<&Path>, db_path: &Path, config_dir: Option<&Path>) {
        if let Err(error) = recovery::sync_backup_to(backup_dir, db_path, config_dir) {
            tracing::warn!("could not refresh profile backup: {error:#}");
        }
    }

    // ── Portable export / import (issue #46) ──────────────────────────────

    /// Encrypt a password with the portable export key → `"enc:exp:v1:<b64>"`.
    fn encrypt_export(plaintext: &str) -> Result<String> {
        let cipher = ChaCha20Poly1305::new((&Self::EXPORT_KEY).into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("export encrypt error: {e}"))?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        Ok(format!(
            "{}{}",
            Self::EXPORT_PREFIX,
            URL_SAFE_NO_PAD.encode(&blob)
        ))
    }

    /// Decrypt a value produced by [`Self::encrypt_export`]; `None` if it isn't one.
    fn decrypt_export(s: &str) -> Option<String> {
        let b64 = s.strip_prefix(Self::EXPORT_PREFIX)?;
        let blob = URL_SAFE_NO_PAD.decode(b64).ok()?;
        if blob.len() < 12 {
            return None;
        }
        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let cipher = ChaCha20Poly1305::new((&Self::EXPORT_KEY).into());
        let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);
        let plain = cipher.decrypt(nonce, ciphertext).ok()?;
        String::from_utf8(plain).ok()
    }

    /// Export all sessions to a portable JSON file. Passwords are re-encrypted
    /// with the built-in export key; everything else stays plaintext so the
    /// file is human-readable and editable. Returns the number of sessions.
    pub fn export_json(&self) -> Result<(String, usize)> {
        let mut out = ExportFile {
            meatshell_export: 1,
            sessions: self.cache.sessions.clone(),
        };
        for s in &mut out.sessions {
            // `cache` holds plaintext passwords; obfuscate with the export key.
            if !s.password.is_empty() {
                let enc = Self::encrypt_export(s.password.as_str())?;
                s.password = Secret::new(enc);
            }
            if !s.private_key_inline.is_empty() {
                let enc = Self::encrypt_export(s.private_key_inline.as_str())?;
                s.private_key_inline = Secret::new(enc);
            }
            for trigger in &mut s.triggers {
                if !trigger.response.is_empty() {
                    let enc = Self::encrypt_export(trigger.response.as_str())?;
                    trigger.response = Secret::new(enc);
                }
            }
            // Same for a password embedded in the proxy URL — an export file
            // is meant to be moved around; it must not carry the one secret
            // that stayed plaintext in sessions.json before the fix.
            if !s.proxy.is_empty() {
                if let Some(enc) =
                    Self::map_proxy_password(&s.proxy, |pass| Self::encrypt_export(pass).ok())
                {
                    s.proxy = enc;
                }
            }
            // `last_used` is machine-local noise — don't carry it across.
            s.last_used = None;
        }
        Ok((serde_json::to_string_pretty(&out)?, out.sessions.len()))
    }

    /// Export all sessions to a portable JSON file. Passwords are re-encrypted
    /// with the built-in export key; everything else stays plaintext so the
    /// file is human-readable and editable. Returns the number of sessions.
    pub fn export_to(&self, path: &Path) -> Result<usize> {
        let (raw, count) = self.export_json()?;
        fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(count)
    }

    /// Publish a complete, private portable export without replacing anything.
    /// Unlike the GUI save dialog, an unattended CLI has no overwrite prompt.
    #[cfg(unix)]
    pub fn export_to_new(&self, path: &Path) -> Result<usize> {
        use std::io::Write;

        if path.as_os_str().is_empty()
            || path.to_string_lossy().chars().any(char::is_control)
            || path.file_name().is_none()
        {
            anyhow::bail!("export destination must be a nonempty valid file path");
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .context("cannot resolve export destination directory")?;
        let profile = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .context("cannot resolve source profile directory")?;
        if parent.starts_with(&profile) {
            anyhow::bail!("export destination must be outside the source profile directory");
        }
        // Resolve existing ancestors once and use that resolved destination for
        // both staging and publication. A symlink alias cannot bypass the
        // profile boundary; existing files/hardlinks still fail no-clobber.
        let destination = parent.join(path.file_name().expect("validated file name"));
        let (raw, count) = self.export_json()?;
        if raw.len() > import::MAX_IMPORT_BYTES {
            anyhow::bail!("portable export exceeds the 16 MiB import limit; no file was written");
        }
        // NamedTempFile uses mode 0600 on Unix. Stage in the same directory so
        // native no-replace publication is atomic and also rejects symlinks.
        let mut staged = tempfile::Builder::new()
            .prefix(".xenterm-export-")
            .tempfile_in(&parent)
            .context("cannot create private export file in destination directory")?;
        Self::verify_private_export_file(staged.as_file())?;
        staged
            .write_all(raw.as_bytes())
            .context("cannot write portable export")?;
        staged
            .as_file()
            .sync_all()
            .context("cannot flush portable export")?;
        staged
            .persist_noclobber(&destination)
            .map_err(|error| error.error)
            .context("cannot publish export; destination must not already exist")?;
        Ok(count)
    }

    #[cfg(unix)]
    fn verify_private_export_file(file: &fs::File) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        // Some mounted filesystems ignore creation mode. Check the opened file
        // before placing any recoverable credential bytes in it.
        let metadata = file
            .metadata()
            .context("cannot verify export file permissions")?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("destination does not enforce private export file permissions; no credentials were written");
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn export_to_new(&self, path: &Path) -> Result<usize> {
        windows_export::export(self, path)
    }

    #[cfg(not(any(unix, windows)))]
    pub fn export_to_new(&self, _path: &Path) -> Result<usize> {
        // Unknown platforms cannot promise private creation permissions.
        // Refusing before serialization keeps secrets off shared disks.
        anyhow::bail!("private CLI export is unsupported on this platform; no file was written")
    }
}

#[path = "import.rs"]
mod import;
#[path = "persistence.rs"]
mod persistence;
#[path = "profile_io.rs"]
mod profile_io;
#[path = "recovery.rs"]
mod recovery;
#[cfg(windows)]
#[path = "windows_export.rs"]
mod windows_export;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn temp_store() -> ConfigStore {
        let path = std::env::temp_dir().join(format!("ms-test-{}.db", Uuid::new_v4()));
        ConfigStore {
            path,
            backup_dir: None,
            cache: ConfigFile::default(),
            key: [7u8; 32],
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(SavedState::of_cache(&ConfigFile::default())).into(),
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn private_cli_export_rejects_oversized_output_before_creating_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let path = directory.path().join("oversized.json");
        let mut store = temp_store();
        store.path = profile.path().join("sessions.db");
        let mut session = Session::new_empty();
        session.note = "x".repeat(import::MAX_IMPORT_BYTES);
        store.cache.sessions.push(session);
        assert!(store
            .export_to_new(&path)
            .unwrap_err()
            .to_string()
            .contains("16 MiB"));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn private_cli_export_checks_effective_permissions_before_writing() {
        use std::os::unix::fs::PermissionsExt;

        let staged = tempfile::NamedTempFile::new().unwrap();
        let file = staged.as_file();
        ConfigStore::verify_private_export_file(file).unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o640))
            .unwrap();
        assert!(ConfigStore::verify_private_export_file(file).is_err());
        assert_eq!(file.metadata().unwrap().len(), 0);
    }

    #[cfg(not(any(unix, windows)))]
    #[test]
    fn private_cli_export_fails_closed_without_private_file_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unsupported.json");
        assert!(temp_store().export_to_new(&path).is_err());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    /// Read one session's raw at-rest JSON straight out of the store
    /// database — the form the encryption tests assert on.
    fn disk_row(store: &ConfigStore, id: &str) -> String {
        let conn = rusqlite::Connection::open(&store.path).unwrap();
        conn.query_row("SELECT data FROM sessions WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn renaming_a_quick_group_renames_the_entries_filed_in_it() {
        let mut store = temp_store();
        store.add_quick_group("ops".into());
        store.add_quick_group("net".into());
        store.set_quick_commands(vec![
            QuickCommand {
                name: "deploy".into(),
                command: "./deploy.sh".into(),
                group: "ops".into(),
                send_enter: true,
            },
            QuickCommand {
                name: "ping".into(),
                command: "ping -c3 1.1.1.1".into(),
                group: "net".into(),
                send_enter: true,
            },
        ]);

        store.rename_quick_group("ops", "release".into());

        // The group list and every entry that carried the old name, together: a header
        // that disagreed with its entries would split one group into two.
        assert_eq!(store.quick_groups(), ["net", "release"]);
        assert_eq!(store.quick_commands()[0].group, "release");
        assert_eq!(store.quick_commands()[1].group, "net");

        // Renaming onto the reserved name, or onto itself, is refused rather than
        // producing a second "default".
        store.rename_quick_group("release", "default".into());
        store.rename_quick_group("release", "release".into());
        assert_eq!(store.quick_groups(), ["net", "release"]);

        // Deleting a group keeps its entries, ungrouped.
        store.remove_quick_group("release");
        assert_eq!(store.quick_groups(), ["net"]);
        assert!(store.quick_commands()[0].group.is_empty());
    }

    #[test]
    fn terminal_cursor_style_defaults_and_validates() {
        let mut store = temp_store();
        assert_eq!(store.terminal_cursor_style(), "block");

        store.set_terminal_cursor_style("bar".into());
        assert_eq!(store.terminal_cursor_style(), "bar");
        store.set_terminal_cursor_style("underline".into());
        assert_eq!(store.terminal_cursor_style(), "underline");
        store.set_terminal_cursor_style("unexpected".into());
        assert_eq!(store.terminal_cursor_style(), "block");

        store.cache = serde_json::from_str("{}").expect("legacy config must deserialize");
        assert_eq!(store.terminal_cursor_style(), "block");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn renderer_mode_preserves_compatibility_default_and_validates() {
        let mut store = temp_store();
        assert_eq!(store.renderer_mode(), "software");

        store.set_renderer_mode("auto".into());
        assert_eq!(store.renderer_mode(), "auto");
        store.set_renderer_mode("gpu".into());
        assert_eq!(store.renderer_mode(), "gpu");
        store.set_renderer_mode("unexpected".into());
        assert_eq!(store.renderer_mode(), "software");

        store.cache = serde_json::from_str("{}").expect("legacy config must deserialize");
        assert_eq!(store.renderer_mode(), "software");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn renderer_mode_preserves_linux_automatic_default_and_validates() {
        let mut store = temp_store();
        assert_eq!(store.renderer_mode(), "auto");

        store.set_renderer_mode("gpu".into());
        assert_eq!(store.renderer_mode(), "gpu");
        store.set_renderer_mode("software".into());
        assert_eq!(store.renderer_mode(), "software");
        store.set_renderer_mode("unexpected".into());
        assert_eq!(store.renderer_mode(), "auto");

        store.cache = serde_json::from_str("{}").expect("legacy config must deserialize");
        assert_eq!(store.renderer_mode(), "auto");
    }

    #[test]
    fn quick_connect_groups_default_collapsed_and_remember_expansion() {
        let mut store = temp_store();
        store.cache.groups = vec!["production".into(), "staging".into()];
        store.cache.sessions.push(Session {
            group: "production".into(),
            ..sample_session("server")
        });

        assert!(store.collapsed_session_groups().is_none());
        store.set_session_group_collapsed("production", false);

        let collapsed = store.collapsed_session_groups().unwrap();
        assert!(!collapsed.iter().any(|group| group == "production"));
        assert!(collapsed.iter().any(|group| group == "staging"));
        assert!(collapsed.iter().any(|group| group == "system"));

        store.set_session_group_collapsed("production", true);
        assert!(store
            .collapsed_session_groups()
            .unwrap()
            .iter()
            .any(|group| group == "production"));
    }

    /// Display order for these tests: default(d1, d2), alpha(a1), beta(b1, b2)
    /// — "default" first, named groups alphabetically, stored Vec order
    /// matching display order.
    fn reorder_store() -> ConfigStore {
        let mut store = temp_store();
        store.cache.sessions = vec![
            sample_session("d1"),
            sample_session("d2"),
            Session {
                group: "alpha".into(),
                ..sample_session("a1")
            },
            Session {
                group: "beta".into(),
                ..sample_session("b1")
            },
            Session {
                group: "beta".into(),
                ..sample_session("b2")
            },
        ];
        // Empty collapse list = every group expanded.
        store.cache.collapsed_session_groups = Some(Vec::new());
        store
    }

    fn id_of(store: &ConfigStore, name: &str) -> String {
        store
            .sessions()
            .iter()
            .find(|s| s.name == name)
            .expect("session present")
            .id
            .clone()
    }

    fn order_of(store: &ConfigStore) -> Vec<(String, String)> {
        store
            .sessions()
            .iter()
            .map(|s| (s.name.clone(), s.group.clone()))
            .collect()
    }

    #[test]
    fn reorder_session_swaps_same_group_neighbours() {
        let mut store = reorder_store();
        assert!(store.reorder_session(&id_of(&store, "d1"), 1));
        assert_eq!(
            order_of(&store)[..2],
            [("d2".into(), "".into()), ("d1".into(), "".into())]
        );
    }

    #[test]
    fn reorder_session_hops_down_into_next_group_at_its_top() {
        let mut store = reorder_store();
        assert!(store.reorder_session(&id_of(&store, "d2"), 1));
        assert_eq!(
            order_of(&store)[1..4],
            [
                ("d2".into(), "alpha".into()),
                ("a1".into(), "alpha".into()),
                ("b1".into(), "beta".into()),
            ]
        );
    }

    #[test]
    fn reorder_session_hops_up_into_previous_group_at_its_bottom() {
        let mut store = reorder_store();
        assert!(store.reorder_session(&id_of(&store, "a1"), -1));
        assert_eq!(
            order_of(&store)[..3],
            [
                ("d1".into(), "".into()),
                ("d2".into(), "".into()),
                ("a1".into(), "".into()),
            ]
        );
    }

    #[test]
    fn reorder_session_skips_collapsed_groups() {
        let mut store = reorder_store();
        store.set_session_group_collapsed("alpha", true);
        assert!(store.reorder_session(&id_of(&store, "d2"), 1));
        assert_eq!(
            order_of(&store)[1..4],
            [
                ("a1".into(), "alpha".into()),
                ("d2".into(), "beta".into()),
                ("b1".into(), "beta".into()),
            ]
        );
    }

    #[test]
    fn reorder_session_lands_in_empty_explicit_folder() {
        let mut store = reorder_store();
        store.cache.groups.push("gamma".into());
        assert!(store.reorder_session(&id_of(&store, "b2"), 1));
        let last = store.sessions().iter().find(|s| s.name == "b2").unwrap();
        assert_eq!(last.group, "gamma");
    }

    #[test]
    fn reorder_session_keeps_emptied_implicit_group_as_folder() {
        let mut store = reorder_store();
        assert!(store.reorder_session(&id_of(&store, "a1"), 1));
        assert_eq!(
            store
                .sessions()
                .iter()
                .find(|s| s.name == "a1")
                .unwrap()
                .group,
            "beta"
        );
        assert!(store.groups().iter().any(|g| g == "alpha"));
    }

    #[test]
    fn reorder_session_no_op_at_list_edges() {
        let mut store = reorder_store();
        assert!(!store.reorder_session(&id_of(&store, "d1"), -1));
        assert!(!store.reorder_session(&id_of(&store, "b2"), 1));
        assert_eq!(order_of(&store).len(), 5);
    }

    #[test]
    fn reorder_session_cross_group_needs_a_collapse_list() {
        let mut store = reorder_store();
        store.cache.collapsed_session_groups = None;
        // Same-group hops still work ...
        assert!(store.reorder_session(&id_of(&store, "d1"), 1));
        // ... but crossing a boundary does not: no list means every group
        // renders collapsed (mirrors build_session_rows).
        assert!(!store.reorder_session(&id_of(&store, "b2"), 1));
    }

    #[test]
    fn issue_316_reserved_system_groups_are_repaired_and_rejected() {
        let mut system_session = sample_session("misfiled");
        system_session.group = "system".into();
        system_session.password = Secret::default();
        let mut default_session = sample_session("legacy-default");
        default_session.group = "Default".into();
        let mut cfg = ConfigFile {
            sessions: vec![system_session, default_session],
            groups: vec![
                "system".into(),
                "System".into(),
                "default".into(),
                "prod".into(),
            ],
            collapsed_session_groups: Some(vec!["system".into(), "prod".into()]),
            ..ConfigFile::default()
        };

        assert!(normalize_reserved_session_groups(&mut cfg));
        assert_eq!(cfg.groups, ["prod"]);
        assert!(cfg.sessions.iter().all(|session| session.group.is_empty()));
        assert!(cfg.sessions[0].password.is_empty());
        // The built-in system folder's collapse preference is display state,
        // not a user-created group, so normalization must preserve it.
        assert_eq!(
            cfg.collapsed_session_groups.as_deref(),
            Some(["system".to_string(), "prod".to_string()].as_slice())
        );

        let mut store = temp_store();
        store.add_group("system".into());
        store.add_group("DEFAULT".into());
        store.add_group("prod".into());
        store.rename_group("prod", "System".into());
        assert_eq!(store.groups(), ["prod"]);

        let mut session = sample_session("server");
        session.group = "SYSTEM".into();
        let id = session.id.clone();
        store.upsert(session);
        assert_eq!(store.get(&id).unwrap().group, "");
    }

    #[test]
    fn session_group_names_are_unique_case_insensitively() {
        let mut store = temp_store();
        store.add_group("Production".into());
        store.add_group("production".into());
        assert_eq!(store.groups(), ["Production"]);
        assert!(store.session_group_exists(" PRODUCTION "));

        let mut session = sample_session("staging-server");
        session.group = "Staging".into();
        store.upsert(session);
        assert!(store.session_group_exists("staging"));

        store.rename_group("Production", "STAGING".into());
        assert_eq!(store.groups(), ["Production"]);

        // Changing only the spelling/case of the same group remains valid.
        store.rename_group("Production", "production".into());
        assert_eq!(store.groups(), ["production"]);
    }

    #[test]
    fn macos_renderer_mode_defaults_to_cpu_and_preserves_gpu_choices() {
        assert_eq!(normalize_macos_renderer_mode(""), "software");
        assert_eq!(normalize_macos_renderer_mode("software"), "software");
        assert_eq!(normalize_macos_renderer_mode("femtovg"), "femtovg");
        assert_eq!(normalize_macos_renderer_mode("skia"), "skia");
        assert_eq!(normalize_macos_renderer_mode("unexpected"), "software");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn renderer_mode_uses_macos_backends_and_validates() {
        let mut store = temp_store();
        assert_eq!(store.renderer_mode(), "software");

        store.set_renderer_mode("skia".into());
        assert_eq!(store.renderer_mode(), "skia");
        store.set_renderer_mode("femtovg".into());
        assert_eq!(store.renderer_mode(), "femtovg");
        store.set_renderer_mode("software".into());
        assert_eq!(store.renderer_mode(), "software");
        store.set_renderer_mode("unexpected".into());
        assert_eq!(store.renderer_mode(), "software");

        store.cache = serde_json::from_str("{}").expect("legacy config must deserialize");
        assert_eq!(store.renderer_mode(), "software");
    }

    #[test]
    fn terminal_cursor_color_normalizes_and_rejects_invalid_values() {
        let mut store = temp_store();
        assert_eq!(store.terminal_cursor_color(), "");

        assert!(store.set_terminal_cursor_color("#1a2B3c"));
        assert_eq!(store.terminal_cursor_color(), "#1A2B3C");
        assert!(store.set_terminal_cursor_color("abcdef"));
        assert_eq!(store.terminal_cursor_color(), "#ABCDEF");

        assert!(!store.set_terminal_cursor_color("#12345"));
        assert_eq!(store.terminal_cursor_color(), "#ABCDEF");
        assert!(!store.set_terminal_cursor_color("#GG0000"));
        assert_eq!(store.terminal_cursor_color(), "#ABCDEF");
    }

    pub(super) fn sample_session(name: &str) -> Session {
        Session {
            name: name.into(),
            host: "192.168.100.2".into(),
            port: 22,
            user: "root".into(),
            ..Session::new_empty()
        }
    }

    #[test]
    fn restores_and_syncs_user_config_backup() {
        let base = std::env::temp_dir().join(format!("ms-backup-{}", Uuid::new_v4()));
        let primary = base.join("portable");
        let backup = base.join("user");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&backup).unwrap();

        // A legacy-format backup (sessions.json from a pre-SQLite build) is
        // restored as files; the JSON migrates into SQLite on the next load.
        let backup_cfg = ConfigFile {
            sessions: vec![sample_session("saved")],
            ..ConfigFile::default()
        };
        std::fs::write(
            backup.join("sessions.json"),
            serde_json::to_string_pretty(&backup_cfg).unwrap(),
        )
        .unwrap();
        std::fs::write(backup.join("secret.key"), [9u8; 32]).unwrap();

        restore_user_backup_if_needed(&primary, &backup).unwrap();
        assert!(sessions_file_has_connections(
            &primary.join("sessions.json")
        ));
        assert_eq!(
            std::fs::read(primary.join("secret.key")).unwrap(),
            [9u8; 32]
        );

        // A save mirrors into dedicated passive storage, never the live
        // legacy profile directory used as a recovery source.
        let dedicated = recovery::backup_directory(&primary, &backup).unwrap();
        let store = ConfigStore {
            path: primary.join("sessions.db"),
            backup_dir: Some(dedicated.clone()),
            cache: ConfigFile {
                sessions: vec![sample_session("new")],
                ..ConfigFile::default()
            },
            key: [7u8; 32],
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(SavedState::default()).into(),
        };
        std::fs::write(primary.join("secret.key"), [7u8; 32]).unwrap();
        store.save().unwrap();

        assert!(
            db_has_sessions(&dedicated.join("sessions.db")).unwrap(),
            "the backup mirror must be a live sessions.db snapshot"
        );
        assert_eq!(
            std::fs::read(dedicated.join("secret.key")).unwrap(),
            [7u8; 32]
        );
        assert_eq!(std::fs::read(backup.join("secret.key")).unwrap(), [9u8; 32]);

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn wallpaper_defaults_to_ms_but_keeps_explicit_choice() {
        // Fresh install (no file).
        let fresh = fresh_config();
        assert_eq!(fresh.wallpaper, "builtin:ms");
        assert!((fresh.wallpaper_overlay - 0.85).abs() < f32::EPSILON);
        // User upgrading from before the feature: JSON without the key.
        let cfg: ConfigFile = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.wallpaper, "builtin:tech");
        // An explicit "无"/none (stored as "") is preserved, not re-defaulted.
        let cfg: ConfigFile = serde_json::from_str(r#"{"wallpaper":""}"#).unwrap();
        assert_eq!(cfg.wallpaper, "");
        // A custom choice is preserved.
        let cfg: ConfigFile = serde_json::from_str(r#"{"wallpaper":"builtin:light"}"#).unwrap();
        assert_eq!(cfg.wallpaper, "builtin:light");

        let mut cfg = ConfigFile {
            wallpaper: "builtin:miku".to_string(),
            defaults_rev: DEFAULTS_REV,
            ..ConfigFile::default()
        };
        assert!(!migrate_defaults(&mut cfg));
        assert_eq!(cfg.wallpaper, "builtin:miku");
    }

    #[test]
    fn wallpaper_transparency_default_migrates_without_overwriting_custom_value() {
        let mut old_default = ConfigFile {
            wallpaper_overlay: PREVIOUS_DEFAULT_WALLPAPER_OVERLAY,
            defaults_rev: 2,
            ..ConfigFile::default()
        };
        assert!(migrate_defaults(&mut old_default));
        assert!((old_default.wallpaper_overlay - 0.85).abs() < f32::EPSILON);

        let mut custom = ConfigFile {
            wallpaper_overlay: 0.70,
            defaults_rev: 2,
            ..ConfigFile::default()
        };
        assert!(migrate_defaults(&mut custom));
        assert!((custom.wallpaper_overlay - 0.70).abs() < f32::EPSILON);
    }

    #[test]
    fn mcp_preview_defaults_are_off_and_get_reset_once() {
        // A config that never carried the fields (new install, hand-written
        // file) deserializes to all-off — the serde default is false.
        let cfg: ConfigFile = serde_json::from_str("{}").unwrap();
        assert!(!cfg.mcp_enabled);
        assert!(!cfg.mcp_use_saved_credentials);
        assert!(!cfg.mcp_allow_commands);
        assert!(!cfg.mcp_allow_file_transfers);

        // A preview build persisted its forced-true defaults; rev 4 resets all
        // four once, because a `true` written as a default is indistinguishable
        // from one deliberately chosen (H-04/H-05).
        let mut preview = ConfigFile {
            mcp_enabled: true,
            mcp_use_saved_credentials: true,
            mcp_allow_commands: true,
            mcp_allow_file_transfers: true,
            defaults_rev: 3,
            ..ConfigFile::default()
        };
        assert!(migrate_defaults(&mut preview));
        assert!(!preview.mcp_enabled);
        assert!(!preview.mcp_use_saved_credentials);
        assert!(!preview.mcp_allow_commands);
        assert!(!preview.mcp_allow_file_transfers);
        assert_eq!(preview.defaults_rev, DEFAULTS_REV);

        // After the reset a deliberate re-enable sticks.
        preview.mcp_enabled = true;
        assert!(!migrate_defaults(&mut preview));
        assert!(preview.mcp_enabled);
    }

    #[test]
    fn the_retired_default_font_migrates_and_a_chosen_font_stands() {
        // A config still naming "Meatshell Mono" — the face the previous
        // binary embedded and the picker offered first — moves to empty, which
        // means "the default". Any other value is a deliberate
        // choice and stands, as does an already-empty one.
        let mut stale = ConfigFile {
            font_family: RETIRED_DEFAULT_FONT.to_string(),
            defaults_rev: 4,
            ..ConfigFile::default()
        };
        assert!(migrate_defaults(&mut stale));
        assert!(
            stale.font_family.is_empty(),
            "the retired face must not survive as the configured font"
        );

        let mut chosen = ConfigFile {
            font_family: "Cascadia Code".to_string(),
            defaults_rev: 4,
            ..ConfigFile::default()
        };
        assert!(migrate_defaults(&mut chosen));
        assert_eq!(chosen.font_family, "Cascadia Code");

        let mut empty = ConfigFile {
            font_family: String::new(),
            defaults_rev: 4,
            ..ConfigFile::default()
        };
        assert!(migrate_defaults(&mut empty));
        assert!(empty.font_family.is_empty());
    }

    /// Rev 6: the terminal went monospace-only, and the proportional faces
    /// rev 5 defaulted to are no longer terminal candidates. A config naming
    /// either resets to the default — the same covenant as the MCP reset, a
    /// value that arrived as a default is indistinguishable from a deliberate
    /// one — and a monospace choice stands.
    #[test]
    fn proportional_default_fonts_migrate_to_the_monospace_default() {
        for face in RETIRED_PROPORTIONAL_DEFAULTS {
            let mut cfg = ConfigFile {
                font_family: (*face).to_string(),
                defaults_rev: 5,
                ..ConfigFile::default()
            };
            assert!(migrate_defaults(&mut cfg));
            assert!(
                cfg.font_family.is_empty(),
                "{face} is proportional and must not survive as the terminal font"
            );
        }

        let mut mono = ConfigFile {
            font_family: "Cascadia Code".to_string(),
            defaults_rev: 5,
            ..ConfigFile::default()
        };
        assert!(migrate_defaults(&mut mono));
        assert_eq!(mono.font_family, "Cascadia Code");
    }

    #[test]
    fn output_highlight_defaults_and_preset_validation() {
        let mut store = temp_store();
        assert!(store.output_highlight_enabled());
        assert!(store.json_format_output());
        assert_eq!(store.output_highlight_preset(), "log");

        store.set_output_highlight_enabled(false);
        store.set_output_highlight_preset("devops".to_string());
        assert!(!store.output_highlight_enabled());
        store.set_json_format_output(false);
        assert!(!store.json_format_output());
        assert_eq!(store.output_highlight_preset(), "devops");

        store.set_output_highlight_preset("future-preset".to_string());
        assert_eq!(store.output_highlight_preset(), "log");

        store.add_output_highlight_rule(OutputHighlightRule {
            pattern: "  connection refused  ".to_string(),
            regex: false,
            case_sensitive: false,
            whole_line: true,
            color: "unknown".to_string(),
            enabled: true,
        });
        assert_eq!(store.output_highlight_rules().len(), 1);
        assert_eq!(
            store.output_highlight_rules()[0].pattern,
            "connection refused"
        );
        assert_eq!(store.output_highlight_rules()[0].color, "red");
        store.set_output_highlight_rule_enabled(0, false);
        assert!(!store.output_highlight_rules()[0].enabled);
        store.remove_output_highlight_rule(0);
        assert!(store.output_highlight_rules().is_empty());

        // An older settings file without either field retains the feature that
        // shipped in the previous version: enabled with the log preset.
        let legacy: ConfigFile = serde_json::from_str("{}").unwrap();
        store.cache = legacy;
        assert!(store.output_highlight_enabled());
        assert_eq!(store.output_highlight_preset(), "log");
    }

    #[test]
    fn saved_password_encrypts_and_decrypts_without_changes() {
        let mut store = temp_store();
        let password = "p@ss word!^&*中文";
        store.cache.sessions.push(Session {
            name: "windows-password".into(),
            host: "192.168.100.2".into(),
            port: 22,
            user: "root".into(),
            password: Secret::new(password),
            ..Session::new_empty()
        });

        store.save().unwrap();
        let id = store.cache.sessions[0].id.clone();
        let raw = disk_row(&store, &id);
        assert!(!raw.contains(password));
        let disk: Session = serde_json::from_str(&raw).unwrap();
        let encrypted = disk.password.as_str();
        assert!(encrypted.starts_with(ConfigStore::ENC_PREFIX));
        assert_eq!(
            ConfigStore::try_decrypt(&store.key, encrypted).as_deref(),
            Some(password)
        );

        let _ = std::fs::remove_file(&store.path);
    }

    #[test]
    fn export_import_roundtrip_preserves_password() {
        let mut a = temp_store();
        a.cache.sessions.push(Session {
            name: "pve".into(),
            host: "192.168.100.2".into(),
            port: 22,
            user: "root".into(),
            password: Secret::new("s3cr3t"),
            ..Session::new_empty()
        });

        let export_path = std::env::temp_dir().join(format!("ms-exp-{}.json", Uuid::new_v4()));
        assert_eq!(a.export_to(&export_path).unwrap(), 1);

        // The file keeps host/user plaintext but the password is obfuscated.
        let raw = std::fs::read_to_string(&export_path).unwrap();
        assert!(raw.contains("192.168.100.2"));
        assert!(raw.contains(ConfigStore::EXPORT_PREFIX));
        assert!(!raw.contains("s3cr3t"));

        // Importing into a fresh store recovers the plaintext password.
        let mut b = temp_store();
        assert_eq!(b.import_from(&export_path).unwrap(), (1, 0));
        assert_eq!(b.cache.sessions.len(), 1);
        assert_eq!(b.cache.sessions[0].password.as_str(), "s3cr3t");
        assert_eq!(b.cache.sessions[0].host, "192.168.100.2");

        // Re-importing the same file skips the duplicate.
        assert_eq!(b.import_from(&export_path).unwrap(), (0, 1));

        let _ = std::fs::remove_file(&export_path);
        let _ = std::fs::remove_file(&a.path);
        let _ = std::fs::remove_file(&b.path);
    }

    #[test]
    fn imports_finalshell_export_and_reencrypts_password_at_rest() {
        let mut store = temp_store();
        let raw = r#"{
            "conection_type": 100,
            "name": "FinalShell host",
            "host": "192.0.2.20",
            "port": 22,
            "user_name": "operator",
            "password": "AwcLDRETFx1OXQgZJNatCplesw+x/P04",
            "authentication_type": 1,
            "terminal_encoding": "UTF-8"
        }"#;

        assert_eq!(store.import_json(raw).unwrap(), (1, 0));
        let session = &store.cache.sessions[0];
        assert_eq!(session.host, "192.0.2.20");
        assert_eq!(session.user, "operator");
        assert_eq!(session.password.as_str(), "meatshell-test");

        let id = session.id.clone();
        let at_rest = disk_row(&store, &id);
        assert!(!at_rest.contains("meatshell-test"));
        let disk: Session = serde_json::from_str(&at_rest).unwrap();
        assert!(disk.password.as_str().starts_with(ConfigStore::ENC_PREFIX));

        let _ = std::fs::remove_file(&store.path);
    }

    #[test]
    fn issue_300_interface_defaults_and_ranges_are_safe() {
        let mut store = temp_store();

        // Legacy configs keep the safe confirmation and familiar paste aliases.
        store.cache = serde_json::from_str("{}").unwrap();
        assert!(store.paste_confirm_enabled());
        assert!(store.extra_paste_shortcuts_enabled());
        assert!(!store.zen_mode());
        assert_eq!(store.terminal_line_spacing(), 1.0);

        store.set_terminal_line_spacing(0.1);
        assert_eq!(store.terminal_line_spacing(), 0.8);
        store.set_terminal_line_spacing(9.0);
        assert_eq!(store.terminal_line_spacing(), 1.5);

        store.set_paste_confirm_enabled(false);
        store.set_extra_paste_shortcuts_enabled(false);
        store.set_zen_mode(true);
        assert!(!store.paste_confirm_enabled());
        assert!(!store.extra_paste_shortcuts_enabled());
        assert!(store.zen_mode());
    }

    #[test]
    fn reorder_session_swaps_same_group_siblings_only() {
        let mut store = temp_store();
        let mk = |id: &str, group: &str| Session {
            id: id.into(),
            name: id.into(),
            group: group.into(),
            ..Session::new_empty()
        };
        store.cache.sessions = vec![mk("a", ""), mk("x", "ops"), mk("b", ""), mk("c", "")];

        // Moving "c" up swaps it with the nearest ungrouped sibling ("b"),
        // leaving the grouped session in between untouched.
        assert!(store.reorder_session("c", -1));
        assert_eq!(
            store
                .sessions()
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "x", "c", "b"]
        );
        // "a" is already the first ungrouped session; "x" is alone in its
        // group; unknown ids are no-ops.
        assert!(!store.reorder_session("a", -1));
        assert!(!store.reorder_session("x", 1));
        assert!(!store.reorder_session("nope", 1));
    }

    // ── Master key & proxy-password encryption ────────────────────────────

    /// A keyring stand-in for tests, built on keyring's public credential
    /// API. Unlike the crate's own `mock` store (which hands every
    /// `Entry::new` an independent in-memory credential), this one shares a
    /// single map keyed by (service, user) — the way the real OS stores
    /// behave — so write-then-read flows like the master-key migration are
    /// exercised realistically. The read-back kill switch simulates a
    /// keychain that accepts a write but won't hand the secret back.
    pub(super) mod fake_keyring {
        use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi};
        use std::any::Any;
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Mutex, OnceLock};

        type Map = Mutex<HashMap<(String, String), Vec<u8>>>;

        fn map() -> &'static Map {
            static MAP: OnceLock<Map> = OnceLock::new();
            MAP.get_or_init(|| Mutex::new(HashMap::new()))
        }

        static READBACK: AtomicBool = AtomicBool::new(true);
        static READ_ERROR: AtomicBool = AtomicBool::new(false);
        static WRITES: AtomicUsize = AtomicUsize::new(0);
        static FAIL_WRITES_AFTER: AtomicUsize = AtomicUsize::new(usize::MAX);

        pub fn install() {
            keyring::set_default_credential_builder(Box::new(Builder));
        }

        pub fn clear() {
            map().lock().unwrap().clear();
            READBACK.store(true, Ordering::SeqCst);
            READ_ERROR.store(false, Ordering::SeqCst);
            WRITES.store(0, Ordering::SeqCst);
            FAIL_WRITES_AFTER.store(usize::MAX, Ordering::SeqCst);
        }

        pub fn set_readback(enabled: bool) {
            READBACK.store(enabled, Ordering::SeqCst);
        }

        pub fn fail_reads(enabled: bool) {
            READ_ERROR.store(enabled, Ordering::SeqCst);
        }

        pub fn fail_writes_after(successful_writes: usize) {
            FAIL_WRITES_AFTER.store(
                WRITES.load(Ordering::SeqCst) + successful_writes,
                Ordering::SeqCst,
            );
        }

        /// How many secrets have been written since the last clear/reset —
        /// the hot-path test asserts a history push writes none.
        pub fn write_count() -> usize {
            WRITES.load(Ordering::SeqCst)
        }

        pub fn reset_writes() {
            WRITES.store(0, Ordering::SeqCst);
        }

        pub fn get(service: &str, user: &str) -> Option<String> {
            let bytes = map()
                .lock()
                .unwrap()
                .get(&(service.into(), user.into()))
                .cloned()?;
            String::from_utf8(bytes).ok()
        }

        #[derive(Debug)]
        struct SharedCredential {
            service: String,
            user: String,
        }

        impl CredentialApi for SharedCredential {
            fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
                if WRITES.fetch_add(1, Ordering::SeqCst) >= FAIL_WRITES_AFTER.load(Ordering::SeqCst)
                {
                    return Err(keyring::Error::NoEntry);
                }
                map()
                    .lock()
                    .unwrap()
                    .insert((self.service.clone(), self.user.clone()), secret.to_vec());
                Ok(())
            }

            fn get_secret(&self) -> keyring::Result<Vec<u8>> {
                if READ_ERROR.load(Ordering::SeqCst) {
                    return Err(keyring::Error::PlatformFailure(Box::new(
                        std::io::Error::other("fixture keyring is unavailable"),
                    )));
                }
                if !READBACK.load(Ordering::SeqCst) {
                    return Err(keyring::Error::NoEntry);
                }
                map()
                    .lock()
                    .unwrap()
                    .get(&(self.service.clone(), self.user.clone()))
                    .cloned()
                    .ok_or(keyring::Error::NoEntry)
            }

            fn delete_credential(&self) -> keyring::Result<()> {
                match map()
                    .lock()
                    .unwrap()
                    .remove(&(self.service.clone(), self.user.clone()))
                {
                    Some(_) => Ok(()),
                    None => Err(keyring::Error::NoEntry),
                }
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        struct Builder;

        impl CredentialBuilderApi for Builder {
            fn build(
                &self,
                _target: Option<&str>,
                service: &str,
                user: &str,
            ) -> keyring::Result<Box<Credential>> {
                Ok(Box::new(SharedCredential {
                    service: service.into(),
                    user: user.into(),
                }))
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }
    }

    /// The fake keyring's map and keyring's process-wide default credential
    /// builder are global state; keyring tests serialize on this so they
    /// never observe each other's entries.
    pub(super) static KEYRING_TESTS: Mutex<()> = Mutex::new(());

    #[test]
    #[cfg(feature = "desktop")]
    fn installed_store_migrates_secret_key_into_the_keychain_and_deletes_the_file() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();

        let dir = std::env::temp_dir().join(format!("ms-key-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let file_key = [3u8; 32];
        fs::write(dir.join("secret.key"), file_key).unwrap();

        // First installed launch: the file's key moves into the keychain and
        // the now-redundant file copy is deleted.
        assert_eq!(
            ConfigStore::resolve_master_key(&dir, false).unwrap(),
            file_key
        );
        assert!(
            !dir.join("secret.key").exists(),
            "a verified migration must delete the file"
        );
        assert_eq!(
            fake_keyring::get(
                ConfigStore::KEYRING_SERVICE,
                ConfigStore::MASTER_KEY_ACCOUNT
            ),
            Some(URL_SAFE_NO_PAD.encode(file_key))
        );
        // The next launch reads the keychain copy without recreating the file.
        assert_eq!(
            ConfigStore::resolve_master_key(&dir, false).unwrap(),
            file_key
        );
        assert!(!dir.join("secret.key").exists());

        // Portable stores never touch the keychain: the file travels with the
        // directory, and that is the key.
        fs::write(dir.join("secret.key"), [9u8; 32]).unwrap();
        assert_eq!(
            ConfigStore::resolve_master_key(&dir, true).unwrap(),
            [9u8; 32]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unverified_keychain_write_keeps_the_secret_key_file() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();
        fake_keyring::set_readback(false);

        let dir = std::env::temp_dir().join(format!("ms-key-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("secret.key"), [7u8; 32]).unwrap();

        // The keychain accepted the write but won't hand it back: the
        // migration must bail out and keep the only other copy on disk.
        assert!(ConfigStore::seed_master_key_in_keyring(&dir).is_none());
        assert!(dir.join("secret.key").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_a_session_or_its_password_drops_the_keyring_credential() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();

        let mut store = temp_store();
        store.keyring_enabled = true;
        let id = Uuid::new_v4().to_string();
        let parked = keyring::Entry::new(ConfigStore::KEYRING_SERVICE, &id).unwrap();
        parked.set_password("hunter2").unwrap();

        // Editing the session down to no password drops the parked credential…
        store.cache.sessions.push(Session {
            id: id.clone(),
            name: "behind-proxy".into(),
            host: "10.0.0.9".into(),
            password: Secret::new("hunter2"),
            ..Session::new_empty()
        });
        let mut cleared = store.get(&id).unwrap().clone();
        cleared.password = Secret::default();
        store.upsert(cleared);
        assert_eq!(parked.get_password().unwrap(), "hunter2");
        store.save().unwrap();
        assert!(parked.get_password().is_err());

        // …and so does deleting the session outright.
        parked.set_password("hunter3").unwrap();
        store.remove(&id);
        assert_eq!(parked.get_password().unwrap(), "hunter3");
        store.save().unwrap();
        assert!(parked.get_password().is_err());
    }

    #[test]
    fn failed_editor_save_keeps_password_until_database_accepts_retry() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();

        let mut store = temp_store();
        store.keyring_enabled = true;
        let mut original = sample_session("editor-save-fixture");
        original.password = Secret::new("fixture-password");
        let id = original.id.clone();
        store.upsert_and_save(original.clone()).unwrap();
        let before = disk_row(&store, &id);
        let parked = keyring::Entry::new(ConfigStore::KEYRING_SERVICE, &id).unwrap();
        assert_eq!(parked.get_password().unwrap(), "fixture-password");

        // A real SQLite transaction failure, after opening the database. The
        // keyring is in-memory test infrastructure, never the user's credentials.
        let connection = rusqlite::Connection::open(&store.path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER refuse_editor_update BEFORE UPDATE ON sessions
                 BEGIN SELECT RAISE(ABORT, 'fixture refuses the update'); END;",
            )
            .unwrap();
        let mut cleared = original.clone();
        cleared.password = Secret::default();
        cleared.name = "edited-fixture".into();
        assert!(store.upsert_and_save(cleared.clone()).is_err());
        assert_eq!(store.sessions().len(), 1);
        assert_eq!(store.get(&id).unwrap().name, original.name);
        assert_eq!(
            store.get(&id).unwrap().password.as_str(),
            "fixture-password"
        );
        assert_eq!(disk_row(&store, &id), before);
        assert_eq!(parked.get_password().unwrap(), "fixture-password");

        connection
            .execute_batch("DROP TRIGGER refuse_editor_update")
            .unwrap();
        // An unrelated save must not leak the failed edit back onto disk.
        store.save().unwrap();
        assert_eq!(disk_row(&store, &id), before);
        store.upsert_and_save(cleared).unwrap();
        assert_eq!(store.sessions().len(), 1);
        assert_eq!(store.get(&id).unwrap().name, "edited-fixture");
        assert!(store.get(&id).unwrap().password.is_empty());
        assert!(matches!(
            parked.get_password(),
            Err(keyring::Error::NoEntry)
        ));
        let disk: Session = serde_json::from_str(&disk_row(&store, &id)).unwrap();
        assert_eq!(disk.name, "edited-fixture");
        assert!(disk.password.is_empty());

        drop(connection);
        let _ = fs::remove_file(&store.path);
    }

    fn refuse_session_writes(store: &ConfigStore) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(&store.path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER refuse_session_write BEFORE INSERT ON sessions
                 BEGIN SELECT RAISE(ABORT, 'fixture refuses the write'); END;",
            )
            .unwrap();
        connection
    }

    #[test]
    fn failed_password_update_restores_keyring_before_retry() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();
        let mut store = temp_store();
        store.keyring_enabled = true;
        let mut original = sample_session("password-update-fixture");
        original.password = Secret::new("old-fixture-password");
        store.upsert_and_save(original.clone()).unwrap();
        let before = disk_row(&store, &original.id);
        let connection = refuse_session_writes(&store);
        let mut edited = original.clone();
        edited.password = Secret::new("new-fixture-password");

        let error = store.upsert_and_save(edited.clone()).unwrap_err();
        assert!(!error.is::<SessionCredentialRollbackFailed>());
        assert_eq!(disk_row(&store, &original.id), before);
        assert_eq!(
            store.get(&original.id).unwrap().password.as_str(),
            original.password.as_str()
        );
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &original.id).as_deref(),
            Some("old-fixture-password")
        );
        assert_eq!(
            fake_keyring::write_count(),
            3,
            "initial, failed edit, compensation"
        );

        connection
            .execute_batch("DROP TRIGGER refuse_session_write")
            .unwrap();
        store.upsert_and_save(edited).unwrap();
        assert_eq!(store.sessions().len(), 1);
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &original.id).as_deref(),
            Some("new-fixture-password")
        );
        drop(connection);
        let _ = fs::remove_file(&store.path);
    }

    #[test]
    fn failed_new_session_save_removes_its_keyring_credential() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();
        let mut store = temp_store();
        store.keyring_enabled = true;
        store.save().unwrap();
        let connection = refuse_session_writes(&store);
        let mut session = sample_session("new-password-fixture");
        session.password = Secret::new("new-fixture-password");
        let error = store.upsert_and_save(session.clone()).unwrap_err();
        assert!(!error.is::<SessionCredentialRollbackFailed>());
        assert!(store.sessions().is_empty());
        assert!(fake_keyring::get(ConfigStore::KEYRING_SERVICE, &session.id).is_none());
        let rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);

        connection
            .execute_batch("DROP TRIGGER refuse_session_write")
            .unwrap();
        store.upsert_and_save(session.clone()).unwrap();
        assert_eq!(store.sessions().len(), 1);
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &session.id).as_deref(),
            Some("new-fixture-password")
        );
        drop(connection);
        let _ = fs::remove_file(&store.path);
    }

    #[test]
    fn a_failed_keyring_compensation_reports_that_the_password_may_have_changed() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();
        let mut store = temp_store();
        store.keyring_enabled = true;
        let mut original = sample_session("compensation-failure-fixture");
        original.password = Secret::new("old-fixture-password");
        store.upsert_and_save(original.clone()).unwrap();
        let before = disk_row(&store, &original.id);
        let connection = refuse_session_writes(&store);
        fake_keyring::fail_writes_after(1);
        let mut edited = original.clone();
        edited.password = Secret::new("new-fixture-password");

        let error = store.upsert_and_save(edited).unwrap_err();
        assert!(error.is::<SessionCredentialRollbackFailed>());
        assert_eq!(disk_row(&store, &original.id), before);
        assert_eq!(
            store.get(&original.id).unwrap().password.as_str(),
            original.password.as_str()
        );
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &original.id).as_deref(),
            Some("new-fixture-password"),
            "the error must accurately report the incomplete compensation"
        );
        drop(connection);
        let _ = fs::remove_file(&store.path);
    }

    #[test]
    fn an_unreadable_keyring_is_not_overwritten_by_an_editor_save() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();
        let mut store = temp_store();
        store.keyring_enabled = true;
        let mut original = sample_session("unreadable-keyring-fixture");
        original.password = Secret::new("old-fixture-password");
        store.upsert_and_save(original.clone()).unwrap();
        let writes_before = fake_keyring::write_count();
        fake_keyring::fail_reads(true);
        let mut edited = original.clone();
        edited.password = Secret::new("new-fixture-password");
        store.upsert_and_save(edited).unwrap();

        assert_eq!(fake_keyring::write_count(), writes_before);
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &original.id).as_deref(),
            Some("old-fixture-password")
        );
        let disk: Session = serde_json::from_str(&disk_row(&store, &original.id)).unwrap();
        assert!(disk.password.as_str().starts_with(ConfigStore::ENC_PREFIX));
        assert_eq!(
            ConfigStore::try_decrypt(&store.key, disk.password.as_str()).as_deref(),
            Some("new-fixture-password")
        );
        fake_keyring::fail_reads(false);
        let _ = fs::remove_file(&store.path);
    }

    #[test]
    fn master_key_entry_decoding_roundtrips_and_rejects_other_shapes() {
        let key = [11u8; 32];
        let stored = URL_SAFE_NO_PAD.encode(key);
        assert_eq!(ConfigStore::decode_master_key(&stored), Some(key));
        assert_eq!(
            ConfigStore::decode_master_key("definitely!not-base64"),
            None
        );
        assert_eq!(
            ConfigStore::decode_master_key(&URL_SAFE_NO_PAD.encode([0u8; 31])),
            None
        );
    }

    #[test]
    fn proxy_password_is_encrypted_at_rest_and_recovered() {
        let mut store = temp_store();
        store.cache.sessions.push(Session {
            name: "behind-proxy".into(),
            host: "192.168.100.3".into(),
            proxy: "socks5://alice:hunter2@127.0.0.1:1080".into(),
            ..Session::new_empty()
        });

        store.save().unwrap();
        let id = store.cache.sessions[0].id.clone();
        let raw = disk_row(&store, &id);
        assert!(
            !raw.contains("hunter2"),
            "the proxy password must not sit in the clear"
        );
        assert!(raw.contains("alice"), "the username stays readable");
        assert!(raw.contains("127.0.0.1:1080"));

        let disk: Session = serde_json::from_str(&raw).unwrap();
        let url = disk.proxy.as_str();
        assert!(url.starts_with("socks5://alice:enc:v1:"));
        assert!(url.ends_with("@127.0.0.1:1080"));
        assert_eq!(
            ConfigStore::map_proxy_password(url, |p| ConfigStore::try_decrypt(&store.key, p)),
            Some("socks5://alice:hunter2@127.0.0.1:1080".to_string())
        );

        // URLs without a password stay exactly as they are.
        for url in [
            "http://proxy.lan:8080",
            "socks5://127.0.0.1:1080",
            "socks5://bob:@127.0.0.1:1080",
        ] {
            assert_eq!(
                ConfigStore::map_proxy_password(url, |p| Some(p.to_string())),
                None
            );
        }

        let _ = fs::remove_file(&store.path);
    }

    #[test]
    fn export_carries_the_proxy_password_obfuscated_and_import_restores_it() {
        let mut a = temp_store();
        a.cache.sessions.push(Session {
            name: "proxied".into(),
            host: "192.168.100.4".into(),
            user: "root".into(),
            port: 22,
            proxy: "http://alice:hunter9@proxy.lan:3128".into(),
            ..Session::new_empty()
        });

        let export_path = std::env::temp_dir().join(format!("ms-exp-{}.json", Uuid::new_v4()));
        a.export_to(&export_path).unwrap();
        let raw = fs::read_to_string(&export_path).unwrap();
        assert!(!raw.contains("hunter9"));
        assert!(raw.contains("proxy.lan:3128"));

        let mut b = temp_store();
        assert_eq!(b.import_from(&export_path).unwrap(), (1, 0));
        assert_eq!(
            b.cache.sessions[0].proxy,
            "http://alice:hunter9@proxy.lan:3128"
        );

        let _ = fs::remove_file(&export_path);
        let _ = fs::remove_file(&a.path);
        let _ = fs::remove_file(&b.path);
    }

    #[test]
    fn plan_save_diffs_only_what_changed() {
        let mut store = temp_store();
        let mut session = sample_session("one");
        session.id = "a".into();
        store.cache.sessions.push(session);
        store.cache.command_history.push("ls".into());
        store.save().unwrap();

        // Nothing changed since the first write: the save is a no-op.
        let saved = store.saved_state.lock().unwrap().clone();
        assert!(ConfigStore::plan_save(&store.cache, &saved).is_empty());

        // Editing one session plans exactly that row — not settings, not
        // history, not the other sessions.
        store.cache.sessions[0].name = "renamed".into();
        let plan = ConfigStore::plan_save(&store.cache, &saved);
        assert_eq!(plan.sessions, vec!["a".to_string()]);
        assert!(!plan.settings);
        assert!(!plan.all_sessions);
        assert!(!plan.history);

        // Deleting a session plans its row for deletion.
        store.cache.sessions.clear();
        let plan = ConfigStore::plan_save(&store.cache, &saved);
        assert_eq!(plan.sessions, vec!["a".to_string()]);

        // Reordering leaves every session's content untouched but moves the
        // ordinals, so every row is rewritten.
        store.cache.sessions = vec![sample_session("two"), sample_session("three")];
        store.cache.sessions.reverse();
        let plan = ConfigStore::plan_save(&store.cache, &saved);
        assert!(plan.all_sessions);

        // A history push plans only the history table — the command box's
        // hot path.
        store.save().unwrap();
        let saved = store.saved_state.lock().unwrap().clone();
        store.cache.command_history.push("cargo test".into());
        let plan = ConfigStore::plan_save(&store.cache, &saved);
        assert!(plan.history);
        assert!(!plan.settings);
        assert!(!plan.all_sessions);
        assert!(plan.sessions.is_empty());
    }

    #[test]
    fn legacy_json_config_migrates_into_sqlite_with_secrets_intact() {
        let dir = std::env::temp_dir().join(format!("ms-migrate-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let key = [3u8; 32];
        fs::write(dir.join("secret.key"), key).unwrap();

        // The old on-disk form: an enc blob, and history with a duplicate.
        let mut old = ConfigFile::default();
        let mut session = sample_session("migrated");
        session.id = "legacy-1".into();
        session.password = Secret::new(ConfigStore::encrypt(&key, "old-secret").unwrap());
        old.sessions.push(session);
        old.command_history = vec!["ls".into(), "top".into(), "ls".into()];
        let json_path = dir.join("sessions.json");
        fs::write(&json_path, serde_json::to_string_pretty(&old).unwrap()).unwrap();

        let mut cache = ConfigStore::read_json_store(&json_path, &key).unwrap();
        assert_eq!(cache.sessions[0].password.as_str(), "old-secret");
        dedup_keep_last(&mut cache.command_history);
        // Dedup keeps the last occurrence: "ls" was used most recently.
        assert_eq!(cache.command_history, ["top", "ls"]);

        // First save after the import lands everything in SQLite, and the
        // snapshot it leaves says the disk is now current.
        let db_path = dir.join("sessions.db");
        let mut state = SavedState::default();
        ConfigStore::persist_snapshot(
            &cache,
            &mut state,
            Some(SavePlan::all()),
            key,
            &db_path,
            None,
            false,
        )
        .unwrap();
        assert!(ConfigStore::plan_save(&cache, &state).is_empty());

        let conn = ConfigStore::open_db(&db_path).unwrap();
        let (settings_raw, mut sessions_disk, history) =
            ConfigStore::read_disk_store(&conn).unwrap().unwrap();
        let settings: ConfigFile = serde_json::from_str(&settings_raw).unwrap();
        assert!(settings.sessions.is_empty());
        assert_eq!(history, ["top", "ls"]);
        assert_eq!(sessions_disk.len(), 1);
        assert!(sessions_disk[0]
            .password
            .as_str()
            .starts_with(ConfigStore::ENC_PREFIX));

        // Round-trip: the row decrypts back to what the connect path needs.
        ConfigStore::session_from_disk_form(&mut sessions_disk[0], &key);
        assert_eq!(sessions_disk[0].password.as_str(), "old-secret");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_database_is_preserved_without_recovery_overwrite() {
        let dir = std::env::temp_dir().join(format!("ms-broken-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("sessions.db");
        let original = b"this is definitely not a sqlite database";
        fs::write(&db, original).unwrap();
        assert!(ConfigStore::read_cache_snapshot(&db, &[7u8; 32]).is_err());
        assert_eq!(fs::read(&db).unwrap(), original);
        assert!(!dir.join("sessions.db.broken").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_history_push_touches_no_session_secrets() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        fake_keyring::install();
        fake_keyring::clear();

        let mut store = temp_store();
        store.keyring_enabled = true;
        for name in ["one", "two"] {
            let mut session = sample_session(name);
            session.password = Secret::new(format!("secret-{name}"));
            store.cache.sessions.push(session);
        }
        store.save().unwrap();
        assert!(
            fake_keyring::write_count() >= 2,
            "the first save parks every saved password in the keyring"
        );

        // The hot path: a command pushed from the command box. It must write
        // history and nothing else — no keyring calls, no session rows.
        fake_keyring::reset_writes();
        store.push_command_history("cargo build".into());
        store.save().unwrap();
        assert_eq!(
            fake_keyring::write_count(),
            0,
            "a history push must not touch the keyring"
        );
        let conn = ConfigStore::open_db(&store.path).unwrap();
        let (_, sessions_disk, history) = ConfigStore::read_disk_store(&conn).unwrap().unwrap();
        assert_eq!(history, ["cargo build"]);
        assert!(
            sessions_disk
                .iter()
                .all(|s| s.password.as_str() == ConfigStore::KEYRING_MARKER),
            "session rows are untouched by the history push"
        );

        let _ = fs::remove_file(&store.path);
    }
}

#[cfg(test)]
mod log_path_tests {
    use super::*;

    #[test]
    fn windows_user_logs_are_outside_config() {
        let base = Path::new("profile").join("xenterm").join("xenterm");
        assert_eq!(
            user_log_dir_from_config(&base.join("config"), true),
            base.join("log").join("log")
        );
    }

    #[test]
    fn unix_user_log_path_is_unchanged() {
        let config = Path::new("home/.config/xenterm");
        assert_eq!(user_log_dir_from_config(config, false), config.join("log"));
    }
}
