//! Recovery and mirrors must never replace an active profile's files.
//!
//! Startup publishes complete files without replacement. Mirrors have a
//! profile-specific destination and serialize with primary SQLite writers.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use sha2::{Digest, Sha256};
use uuid::Uuid;

struct Scratch(PathBuf);

impl Scratch {
    fn new(parent: &Path) -> Result<Self> {
        private_directory(parent)?;
        let path = parent.join(format!(".xenterm-recovery-{}", Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .context("cannot stage profile recovery")?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .context("cannot create profile recovery directory")
}

fn write_stage(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("cannot stage recovered profile file")?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Publish an owned, complete staged file without replacing a destination.
/// tempfile uses native no-replace rename where available, including portable
/// FAT/exFAT volumes. Never fall back to a racy existence check plus overwrite.
fn publish_new(staged: &Path, destination: &Path) -> Result<bool> {
    let temporary = tempfile::TempPath::try_from_path(staged.to_path_buf())
        .context("cannot own staged profile recovery file")?;
    match temporary.persist_noclobber(destination) {
        Ok(()) => Ok(true),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(std::io::Error::from(error))
            .context("cannot atomically publish recovered profile file"),
    }
}

fn profile_exists(directory: &Path) -> bool {
    directory.join("sessions.db").exists() || directory.join("sessions.json").exists()
}

pub(super) fn backup_directory(primary: &Path, legacy_root: &Path) -> Result<PathBuf> {
    let canonical = primary
        .canonicalize()
        .context("cannot identify profile backup directory")?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    Ok(legacy_root
        .join("xenterm-backups")
        .join(format!("{digest:x}")))
}

/// This runs during data-dir selection as well as on load. Use the same
/// dedicated-first recovery policy in both places.
pub(super) fn migrate_legacy(legacy_root: &Path, primary: &Path) -> Result<()> {
    restore_user_backup_if_needed(primary, legacy_root)
}

pub(super) fn restore_user_backup_if_needed(primary: &Path, legacy_root: &Path) -> Result<()> {
    if primary == legacy_root || profile_exists(primary) {
        return Ok(());
    }
    private_directory(primary)?;
    let dedicated = backup_directory(primary, legacy_root)?;
    if restore_from(primary, &dedicated, false)? {
        return Ok(());
    }
    // The legacy root is only a source; never mirror into or replace it.
    restore_from(primary, legacy_root, true)?;
    Ok(())
}

fn file_state(path: &Path) -> Result<Option<(u64, std::time::SystemTime)>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(Some((meta.len(), meta.modified()?))),
        Ok(_) => bail!("profile recovery source must be a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("cannot inspect profile recovery source"),
    }
}

/// Do not open a live legacy source with SQLite: even a read-only WAL open can
/// create sidecars. Copy DB/WAL privately, reject any observed source change,
/// then let SQLite materialize a consistent standalone snapshot there.
fn stage_database(source: &Path, scratch: &Path) -> Result<Connection> {
    let mut wal = source.as_os_str().to_os_string();
    wal.push("-wal");
    let files = [source.to_path_buf(), PathBuf::from(wal)];
    let before = files
        .iter()
        .map(|path| file_state(path))
        .collect::<Result<Vec<_>>>()?;
    if before[0].is_none() {
        bail!("profile recovery source disappeared");
    }
    for (index, (path, state)) in files.iter().zip(&before).enumerate() {
        if state.is_some() {
            let bytes = fs::read(path).context("cannot read profile recovery source")?;
            let name = if index == 0 {
                "source.db"
            } else {
                "source.db-wal"
            };
            write_stage(&scratch.join(name), &bytes)?;
        }
    }
    let after = files
        .iter()
        .map(|path| file_state(path))
        .collect::<Result<Vec<_>>>()?;
    if before != after {
        bail!("profile recovery source changed; retry after other writers finish");
    }
    let conn =
        Connection::open_with_flags(scratch.join("source.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("cannot inspect staged profile recovery database")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

fn source_key(directory: &Path) -> Result<Option<Vec<u8>>> {
    let path = directory.join("secret.key");
    if file_state(&path)?.is_none() {
        return Ok(None);
    }
    let bytes = fs::read(path).context("cannot read profile recovery key")?;
    if bytes.len() != 32 {
        bail!("profile recovery key is invalid; existing files preserved");
    }
    Ok(Some(bytes))
}

fn ensure_matching_key(primary: &Path, source: Option<&[u8]>, scratch: &Path) -> Result<()> {
    let destination = primary.join("secret.key");
    match (source, source_key(primary)?) {
        (Some(expected), Some(actual)) if expected == actual.as_slice() => return Ok(()),
        (None, None) => return Ok(()),
        (Some(expected), None) => {
            let staged = scratch.join("secret.key");
            publish_new(&staged, &destination)?;
            // A concurrent initializer might have won publication. Adopt only
            // the exact key matching this backup, never replace its key.
            if fs::read(&destination).ok().as_deref() == Some(expected) {
                return Ok(());
            }
        }
        _ => {}
    }
    bail!("profile recovery key differs from the existing key; existing files preserved")
}

fn restore_from(primary: &Path, source: &Path, require_sessions: bool) -> Result<bool> {
    if profile_exists(primary) {
        return Ok(true);
    }
    let database = source.join("sessions.db");
    let json = source.join("sessions.json");
    if !database.exists() && !json.exists() {
        return Ok(false);
    }
    let scratch = Scratch::new(primary)?;
    let (staged, name) = if database.exists() {
        let conn = stage_database(&database, &scratch.0)?;
        let settings: i64 = conn.query_row(
            "SELECT COUNT(*) FROM meta WHERE key = 'settings'",
            [],
            |row| row.get(0),
        )?;
        let sessions: i64 =
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        if settings == 0 || (require_sessions && sessions == 0) {
            return Ok(false);
        }
        let staged = scratch.0.join("sessions.db");
        conn.execute("VACUUM INTO ?1", [staged.to_string_lossy().as_ref()])?;
        (staged, "sessions.db")
    } else {
        let bytes = fs::read(&json).context("cannot read legacy profile recovery JSON")?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("invalid legacy profile recovery JSON")?;
        let sessions = value
            .get("sessions")
            .and_then(serde_json::Value::as_array)
            .context("legacy profile recovery JSON has no sessions array")?;
        if require_sessions && sessions.is_empty() {
            return Ok(false);
        }
        let staged = scratch.0.join("sessions.json");
        write_stage(&staged, &bytes)?;
        (staged, "sessions.json")
    };
    let key = source_key(source)?;
    if let Some(key) = &key {
        write_stage(&scratch.0.join("secret.key"), key)?;
    }
    // Validate the exact staged candidate and its matching source key before
    // publishing any destination file. A backup tied to an installed OS
    // keyring cannot silently initialize a portable profile with a fresh key.
    super::ConfigStore::preflight_explicit_profile(&scratch.0)?;
    // Recheck after staging, before installing even a key. The final
    // publication also remains no-clobber if another initializer races us.
    if profile_exists(primary) {
        return Ok(true);
    }
    ensure_matching_key(primary, key.as_deref(), &scratch.0)?;
    if !publish_new(&staged, &primary.join(name))? {
        return Ok(true);
    }
    let known_hosts = source.join("known_hosts");
    if file_state(&known_hosts)?.is_some() {
        let staged_hosts = scratch.0.join("known_hosts");
        write_stage(&staged_hosts, &fs::read(known_hosts)?)?;
        publish_new(&staged_hosts, &primary.join("known_hosts"))?;
    }
    Ok(true)
}

/// Archive only after the database migration has committed. Retain the source
/// as well: removing it after a contents check could delete a concurrent edit.
pub(super) fn finish_legacy_migration(json: &Path) -> Result<()> {
    let migrated = json.with_extension("json.migrated");
    if migrated.exists() || !json.exists() {
        return Ok(());
    }
    let scratch = Scratch::new(json.parent().context("legacy JSON has no directory")?)?;
    let staged = scratch.0.join("sessions.json");
    write_stage(&staged, &fs::read(json)?)?;
    publish_new(&staged, &migrated)?;
    Ok(())
}

/// The caller's primary commit is complete. Lock the primary again before
/// reading the latest committed state and keep that lock through publication;
/// an older mirror therefore cannot land after a newer cooperating writer.
pub(super) fn sync_backup_to(
    backup_dir: Option<&Path>,
    db_path: &Path,
    config_dir: Option<&Path>,
) -> Result<()> {
    let Some(backup_dir) = backup_dir else {
        return Ok(());
    };
    let primary = db_path
        .parent()
        .context("primary database has no directory")?;
    let legacy_root = backup_dir
        .parent()
        .and_then(Path::parent)
        .context("profile backup destination is not dedicated storage")?;
    if backup_directory(primary, legacy_root)? != backup_dir {
        bail!("profile backup destination is not this profile's dedicated storage");
    }
    let mut guard = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .context("cannot open primary profile for backup")?;
    guard.busy_timeout(Duration::from_secs(5))?;
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("cannot lock primary profile for backup")?;
    private_directory(backup_dir)?;
    let scratch = Scratch::new(backup_dir)?;
    let staged = scratch.0.join("sessions.db");
    let reader = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    reader.busy_timeout(Duration::from_secs(5))?;
    reader
        .execute("VACUUM INTO ?1", [staged.to_string_lossy().as_ref()])
        .context("cannot snapshot primary profile for backup")?;
    drop(reader);
    // Auxiliary files are staged before publishing any part of the mirror.
    let mut auxiliary = Vec::new();
    if let Some(config_dir) = config_dir {
        for name in ["secret.key", "known_hosts"] {
            let source = config_dir.join(name);
            if file_state(&source)?.is_some() {
                let temporary = scratch.0.join(name);
                write_stage(&temporary, &fs::read(source)?)?;
                auxiliary.push((temporary, backup_dir.join(name)));
            }
        }
    }
    for (temporary, destination) in auxiliary {
        fs::rename(temporary, destination)
            .context("cannot publish profile backup auxiliary file")?;
    }
    fs::rename(staged, backup_dir.join("sessions.db"))
        .context("cannot publish profile database backup")?;
    tx.rollback()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{ConfigStore, Secret, Session};
    use super::*;

    fn fixture() -> Scratch {
        Scratch::new(&std::env::temp_dir()).unwrap()
    }

    fn database(path: &Path, name: &str) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE sessions(ordinal INTEGER, id TEXT PRIMARY KEY, data TEXT); CREATE TABLE command_history(seq INTEGER PRIMARY KEY, command TEXT);").unwrap();
        conn.execute("INSERT INTO meta VALUES('settings', '{}')", [])
            .unwrap();
        let data = serde_json::to_string(&session(name)).unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES(0, ?1, ?2)",
            rusqlite::params![name, data],
        )
        .unwrap();
        conn
    }

    fn session(name: &str) -> Session {
        Session {
            id: name.into(),
            name: name.into(),
            host: format!("{name}.example.invalid"),
            ..Session::new_empty()
        }
    }

    fn directory_files(
        directory: &Path,
    ) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_os_string(),
                    fs::read(&path).unwrap(),
                )
            })
            .collect()
    }

    fn only_session(path: &Path) -> String {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT id FROM sessions", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn atomic_publication_never_replaces_an_existing_file() {
        let root = fixture();
        let staged = root.0.join("staged");
        let destination = root.0.join("existing");
        write_stage(&staged, b"replacement").unwrap();
        fs::write(&destination, b"original").unwrap();
        assert!(!publish_new(&staged, &destination).unwrap());
        assert_eq!(fs::read(destination).unwrap(), b"original");
        assert!(
            !staged.exists(),
            "failed publication cleans up its owned stage"
        );
    }

    #[test]
    fn no_replace_publication_moves_a_complete_owned_stage() {
        let root = fixture();
        let staged = root.0.join("staged");
        let destination = root.0.join("published");
        write_stage(&staged, b"complete contents").unwrap();
        assert!(publish_new(&staged, &destination).unwrap());
        assert_eq!(fs::read(destination).unwrap(), b"complete contents");
        assert!(!staged.exists());
    }

    #[test]
    fn an_initialized_empty_primary_is_never_restored_over() {
        let root = fixture();
        let primary = root.0.join("primary");
        let legacy = root.0.join("legacy");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        let primary_conn = database(&primary.join("sessions.db"), "deleted");
        primary_conn.execute("DELETE FROM sessions", []).unwrap();
        let _source = database(&legacy.join("sessions.db"), "legacy");
        restore_user_backup_if_needed(&primary, &legacy).unwrap();
        let count: i64 = primary_conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        drop(primary_conn);
        fs::remove_file(primary.join("sessions.db")).unwrap();
        fs::write(primary.join("sessions.json"), br#"{"sessions":[]}"#).unwrap();
        restore_user_backup_if_needed(&primary, &legacy).unwrap();
        assert!(!primary.join("sessions.db").exists());
        assert_eq!(
            fs::read(primary.join("sessions.json")).unwrap(),
            br#"{"sessions":[]}"#
        );
    }

    #[test]
    fn recovery_rejects_a_different_existing_key_without_publishing_database() {
        let root = fixture();
        let primary = root.0.join("primary");
        let legacy = root.0.join("legacy");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(primary.join("secret.key"), [1; 32]).unwrap();
        fs::write(legacy.join("secret.key"), [2; 32]).unwrap();
        let _source = database(&legacy.join("sessions.db"), "legacy");
        assert!(restore_user_backup_if_needed(&primary, &legacy).is_err());
        assert!(!primary.join("sessions.db").exists());
        assert_eq!(fs::read(primary.join("secret.key")).unwrap(), [1; 32]);
    }

    #[test]
    fn recovered_credentials_require_the_matching_source_key_before_any_publication() {
        for format in ["database", "json"] {
            for key_case in ["missing", "wrong", "correct", "keyring"] {
                let root = fixture();
                let primary = root.0.join("primary");
                let legacy = root.0.join("legacy");
                fs::create_dir_all(&primary).unwrap();
                fs::create_dir_all(&legacy).unwrap();
                fs::write(primary.join("unrelated.txt"), b"preserve destination").unwrap();
                fs::write(legacy.join("known_hosts"), b"preserve source").unwrap();
                let encryption_key = [17; 32];
                let mut saved = session("encrypted-fixture");
                saved.password = Secret::new(if key_case == "keyring" {
                    ConfigStore::KEYRING_MARKER.to_string()
                } else {
                    ConfigStore::encrypt(&encryption_key, "synthetic-recovery-secret").unwrap()
                });
                let connection = if format == "database" {
                    let conn = database(&legacy.join("sessions.db"), &saved.id);
                    conn.execute(
                        "UPDATE sessions SET data = ?1",
                        [serde_json::to_string(&saved).unwrap()],
                    )
                    .unwrap();
                    Some(conn)
                } else {
                    fs::write(
                        legacy.join("sessions.json"),
                        serde_json::json!({"sessions": [saved.clone()]}).to_string(),
                    )
                    .unwrap();
                    None
                };
                if key_case != "missing" {
                    let key = if key_case == "wrong" {
                        [19; 32]
                    } else {
                        encryption_key
                    };
                    fs::write(legacy.join("secret.key"), key).unwrap();
                }
                let source_before = directory_files(&legacy);
                let destination_before = directory_files(&primary);
                let result = restore_user_backup_if_needed(&primary, &legacy);
                if key_case == "correct" {
                    result.unwrap();
                    assert_eq!(
                        fs::read(primary.join("secret.key")).unwrap(),
                        encryption_key
                    );
                    ConfigStore::preflight_explicit_profile(&primary).unwrap();
                    if format == "database" {
                        assert_eq!(only_session(&primary.join("sessions.db")), saved.id);
                    } else {
                        assert_eq!(
                            fs::read(primary.join("sessions.json")).unwrap(),
                            fs::read(legacy.join("sessions.json")).unwrap()
                        );
                    }
                } else {
                    let error = result.unwrap_err().to_string();
                    assert!(
                        error.contains("portable export"),
                        "{format}/{key_case}: {error}"
                    );
                    assert!(!error.contains("synthetic-recovery-secret"));
                    assert_eq!(directory_files(&primary), destination_before);
                }
                assert_eq!(directory_files(&legacy), source_before);
                drop(connection);
            }
        }
    }

    #[test]
    fn dedicated_paths_are_scoped_and_take_precedence_over_legacy() {
        let root = fixture();
        let primary = root.0.join("primary");
        let other = root.0.join("other");
        let legacy = root.0.join("legacy");
        for directory in [&primary, &other, &legacy] {
            fs::create_dir_all(directory).unwrap();
        }
        let dedicated = backup_directory(&primary, &legacy).unwrap();
        assert_ne!(dedicated, backup_directory(&other, &legacy).unwrap());
        assert!(dedicated.starts_with(legacy.join("xenterm-backups")));
        fs::create_dir_all(&dedicated).unwrap();
        let _old = database(&legacy.join("sessions.db"), "legacy");
        let _new = database(&dedicated.join("sessions.db"), "dedicated");
        restore_user_backup_if_needed(&primary, &legacy).unwrap();
        assert_eq!(only_session(&primary.join("sessions.db")), "dedicated");
        assert_eq!(only_session(&legacy.join("sessions.db")), "legacy");
    }

    #[test]
    fn migration_archive_retains_source_and_never_replaces_an_archive() {
        let root = fixture();
        let json = root.0.join("sessions.json");
        fs::write(&json, b"first").unwrap();
        finish_legacy_migration(&json).unwrap();
        fs::write(&json, b"later edit").unwrap();
        finish_legacy_migration(&json).unwrap();
        assert_eq!(fs::read(&json).unwrap(), b"later edit");
        assert_eq!(
            fs::read(json.with_extension("json.migrated")).unwrap(),
            b"first"
        );
    }

    #[test]
    fn mirror_snapshots_live_wal_and_delayed_calls_copy_latest_state() {
        let root = fixture();
        let primary = root.0.join("primary");
        let legacy = root.0.join("legacy");
        fs::create_dir_all(&primary).unwrap();
        let path = primary.join("sessions.db");
        let conn = database(&path, "first");
        let backup = backup_directory(&primary, &legacy).unwrap();
        sync_backup_to(Some(&backup), &path, Some(&primary)).unwrap();
        conn.execute("UPDATE sessions SET id = 'latest'", [])
            .unwrap();
        // A delayed callback has no stale ConfigFile to publish: it rereads
        // the authoritative database only after obtaining its writer lock.
        sync_backup_to(Some(&backup), &path, Some(&primary)).unwrap();
        assert_eq!(only_session(&backup.join("sessions.db")), "latest");
        assert!(!legacy.join("sessions.db").exists());
        assert!(!backup.join("sessions.db-wal").exists());
        assert!(sync_backup_to(Some(&legacy), &path, Some(&primary)).is_err());
        assert!(!legacy.join("sessions.db").exists());
    }

    #[test]
    fn legacy_wal_recovery_does_not_modify_its_source_files() {
        let root = fixture();
        let primary = root.0.join("primary");
        let legacy = root.0.join("legacy");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        let conn = database(&legacy.join("sessions.db"), "wal-source");
        fs::write(legacy.join("secret.key"), [9; 32]).unwrap();
        let contents = || {
            fs::read_dir(&legacy)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    (
                        path.file_name().unwrap().to_os_string(),
                        fs::read(&path).unwrap(),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let before = contents();
        assert!(legacy.join("sessions.db-wal").exists());
        restore_user_backup_if_needed(&primary, &legacy).unwrap();
        assert_eq!(only_session(&primary.join("sessions.db")), "wal-source");
        assert_eq!(fs::read(primary.join("secret.key")).unwrap(), [9; 32]);
        assert_eq!(contents(), before);
        drop(conn);
    }

    #[test]
    fn concurrent_mirrors_wait_for_the_writer_and_publish_latest_rows() {
        let root = fixture();
        let primary = root.0.join("primary");
        let legacy = root.0.join("legacy");
        fs::create_dir_all(&primary).unwrap();
        let path = primary.join("sessions.db");
        let mut conn = database(&path, "first");
        let backup = backup_directory(&primary, &legacy).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute("UPDATE sessions SET id = 'committed-latest'", [])
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let backup = backup.clone();
                let path = path.clone();
                let primary = primary.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    sync_backup_to(Some(&backup), &path, Some(&primary)).unwrap();
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        tx.commit().unwrap();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            only_session(&backup.join("sessions.db")),
            "committed-latest"
        );
        assert!(!legacy.join("sessions.db").exists());
        assert_eq!(
            fs::read_dir(&backup).unwrap().count(),
            1,
            "all unique staging directories were removed"
        );
    }
}
