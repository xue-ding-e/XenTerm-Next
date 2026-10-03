//! Serialize the SQLite/keyring boundary, including automatic rollback and crashes.
//!
//! The OS owns the short-lived lock, not SQLite, so an implicit SQL rollback
//! cannot let another writer race compensation. An authenticated, encrypted
//! journal makes unfinished compensation recoverable after the process exits.
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ConfigStore, Secret, SessionCredentialRollbackFailed};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(20);
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;

/// Keep this guard through SQL commit or verified credential compensation.
/// Closing its file, including process termination, releases the OS lock.
/// Never unlink the lockfile: a second inode would split the writer lock.
pub(super) struct ProfileIoGuard {
    _lock: File,
    journal: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    commit_token: String,
    baseline: Option<[u8; 32]>,
    credentials: Vec<Credential>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    id: String,
    // None is an absent entry; Some(encrypt("")) is a present empty entry.
    previous: Option<String>,
}

/// A failed staging attempt must not leave copies of the journal behind.
struct StagedFile(PathBuf);

impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn io_error() -> anyhow::Error {
    anyhow::anyhow!("cannot safely access the profile transaction files")
}

fn recovery_error() -> anyhow::Error {
    SessionCredentialRollbackFailed.into()
}

/// Resolve directory aliases and existing database symlinks before choosing
/// sidecars. Before initial creation, the canonical parent plus filename is
/// the same identity that a later canonicalization of the database returns.
fn canonical_database(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink does not establish a safe initial identity.
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(io_error()),
            }
            let name = path.file_name().ok_or_else(io_error)?;
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            Ok(parent.canonicalize().map_err(|_| io_error())?.join(name))
        }
        Err(_) => Err(io_error()),
    }
}

fn sidecar(database: &Path, suffix: &str) -> PathBuf {
    let mut name = database.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Read-only admission probe for service profiles, which must never recover
/// an installed profile's OS-keyring journal using a newly initialized key.
/// A new/nonexistent directory cannot contain a pending journal.
pub(super) fn pending_journal(db_path: &Path) -> Result<bool> {
    let parent = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match fs::metadata(parent) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(io_error()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(io_error()),
    }
    regular_file(&sidecar(
        &canonical_database(db_path)?,
        ".credential-journal",
    ))
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // On Windows the profile directory supplies the inherited access policy,
    // matching the encryption-key and recovery staging files.
    options
}

fn regular_file(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(io_error()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(io_error()),
    }
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path.parent().ok_or_else(io_error)?)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| io_error())?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl ProfileIoGuard {
    pub(super) fn acquire(db_path: &Path) -> Result<Self> {
        Self::acquire_with_timeout(db_path, LOCK_TIMEOUT)
    }

    fn acquire_with_timeout(db_path: &Path, timeout: Duration) -> Result<Self> {
        let database = canonical_database(db_path)?;
        let lock_path = sidecar(&database, ".profile-lock");
        // Avoid following a pre-existing sidecar symlink or opening a device.
        regular_file(&lock_path)?;
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|_| io_error())?;
        if !lock.metadata().map_err(|_| io_error())?.is_file() {
            return Err(io_error());
        }
        let start = Instant::now();
        loop {
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    if start.elapsed() >= timeout {
                        bail!("profile is busy; retry after the other operation finishes");
                    }
                    std::thread::sleep(LOCK_RETRY.min(timeout.saturating_sub(start.elapsed())));
                }
                Err(_) => return Err(io_error()),
            }
        }
        Ok(Self {
            _lock: lock,
            journal: sidecar(&database, ".credential-journal"),
        })
    }

    /// Call immediately after BEGIN IMMEDIATE, before reading credentials or
    /// changing SQL. A journal is never discarded on ambiguous or failed
    /// recovery, so later processes cannot mistake a partial rollback for a
    /// healthy profile. Errors deliberately omit IDs, values, and paths.
    pub(super) fn recover(&self, tx: &Connection, key: &[u8; 32]) -> Result<()> {
        self.recover_inner(tx, key).map_err(|_| recovery_error())
    }

    fn recover_inner(&self, tx: &Connection, key: &[u8; 32]) -> Result<()> {
        if tx.is_autocommit() {
            return Err(recovery_error());
        }
        if !regular_file(&self.journal)? {
            return Ok(());
        }
        let mut encrypted = String::new();
        File::open(&self.journal)
            .map_err(|_| io_error())?
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_string(&mut encrypted)
            .map_err(|_| recovery_error())?;
        if encrypted.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(recovery_error());
        }
        // Authenticate metadata as well as secrets. Even an all-None journal
        // must reject a wrong key, a modified token, or a modified account ID.
        let document = ConfigStore::try_decrypt(key, &encrypted).ok_or_else(recovery_error)?;
        let journal: Journal = serde_json::from_str(&document).map_err(|_| recovery_error())?;
        if journal.version != 1 || journal.commit_token.is_empty() {
            return Err(recovery_error());
        }
        let revision: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = 'write_revision'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| recovery_error())?;
        if revision.as_deref() == Some(journal.commit_token.as_str()) {
            // COMMIT happened before termination. Never restore older secrets.
            return self.finish();
        }
        if ConfigStore::disk_fingerprint(tx).map_err(|_| recovery_error())? != journal.baseline {
            return Err(recovery_error());
        }
        let mut ids = HashSet::new();
        let mut previous = Vec::with_capacity(journal.credentials.len());
        // Validate/decrypt every entry before attempting any compensation.
        for credential in journal.credentials {
            if !ids.insert(credential.id.clone()) {
                return Err(recovery_error());
            }
            let secret = credential
                .previous
                .map(|encrypted| {
                    ConfigStore::try_decrypt(key, &encrypted)
                        .map(Secret::new)
                        .ok_or_else(recovery_error)
                })
                .transpose()?;
            previous.push((credential.id, secret));
        }
        for (id, secret) in previous {
            ConfigStore::restore_keyring_password(&id, secret.as_ref())
                .map_err(|_| recovery_error())?;
        }
        self.finish()
    }

    /// Durably publish the complete pre-mutation credential snapshot. The
    /// caller stamps commit_token in its SQL transaction and must not touch
    /// keyring credentials until this method succeeds.
    pub(super) fn prepare(
        &self,
        tx: &Connection,
        key: &[u8; 32],
        credentials: &HashMap<String, Option<Secret>>,
        commit_token: &str,
    ) -> Result<()> {
        if tx.is_autocommit() || commit_token.is_empty() {
            return Err(io_error());
        }
        if regular_file(&self.journal)? {
            return Err(recovery_error());
        }
        let revision: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = 'write_revision'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| io_error())?;
        if revision.as_deref() == Some(commit_token) {
            // Reusing a token would make an uncommitted keyring edit look
            // committed after a crash. Callers normally supply a fresh UUID.
            return Err(io_error());
        }
        let mut previous = credentials
            .iter()
            .map(|(id, secret)| {
                Ok(Credential {
                    id: id.clone(),
                    previous: secret
                        .as_ref()
                        .map(|secret| ConfigStore::encrypt(key, secret.as_str()))
                        .transpose()
                        .map_err(|_| io_error())?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        previous.sort_by(|a, b| a.id.cmp(&b.id));
        let journal = Journal {
            version: 1,
            commit_token: commit_token.to_owned(),
            baseline: ConfigStore::disk_fingerprint(tx).map_err(|_| io_error())?,
            credentials: previous,
        };
        let document = serde_json::to_string(&journal).map_err(|_| io_error())?;
        let encrypted = ConfigStore::encrypt(key, &document).map_err(|_| io_error())?;
        if encrypted.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(io_error());
        }
        let staged = StagedFile(sidecar(
            &self.journal,
            &format!(".stage-{}", Uuid::new_v4()),
        ));
        let mut file = private_options()
            .create_new(true)
            .open(&staged.0)
            .map_err(|_| io_error())?;
        file.write_all(encrypted.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| io_error())?;
        drop(file);
        // Use platform-native no-replace rename (Linux/macOS/Windows), with
        // tempfile's safe fallback where supported. Unlike hard-link-only
        // publication this also works on common removable filesystems.
        tempfile::TempPath::try_from_path(staged.0.clone())
            .map_err(|_| io_error())?
            .persist_noclobber(&self.journal)
            .map_err(|_| recovery_error())?;
        private_options()
            .open(&self.journal)
            .and_then(|file| file.sync_all())
            .map_err(|_| io_error())?;
        // Include the staging-link removal in the directory durability barrier.
        drop(staged);
        sync_parent(&self.journal)?;
        Ok(())
    }

    /// Only call after proving SQL committed or all previous credentials have
    /// been restored and verified. Failed/unresolved work keeps its journal.
    pub(super) fn finish(&self) -> Result<()> {
        if !regular_file(&self.journal)? {
            return Ok(());
        }
        fs::remove_file(&self.journal).map_err(|_| io_error())?;
        sync_parent(&self.journal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fake_keyring, KEYRING_TESTS};
    use super::*;

    const KEY: [u8; 32] = [41; 32];
    const ACCOUNT: &str = "profile-io-credential";
    const TOKEN: &str = "profile-io-commit-token";

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("xenterm-profile-io-{}", Uuid::new_v4()));
            fs::create_dir(&directory).unwrap();
            Self(directory)
        }
        fn database(&self) -> PathBuf {
            self.0.join("sessions.db")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn begin(path: &Path) -> Connection {
        let conn = ConfigStore::open_db(path).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn
    }

    fn previous(secret: Option<&str>) -> HashMap<String, Option<Secret>> {
        HashMap::from([(ACCOUNT.to_owned(), secret.map(Secret::new))])
    }

    fn install_keyring() {
        fake_keyring::install();
        fake_keyring::clear();
    }

    fn set_password(password: &str) {
        ConfigStore::keyring_entry(ACCOUNT)
            .unwrap()
            .set_password(password)
            .unwrap();
    }

    fn password() -> Option<String> {
        fake_keyring::get(ConfigStore::KEYRING_SERVICE, ACCOUNT)
    }

    #[test]
    fn pending_journal_restores_exact_credential_and_hides_plaintext() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        install_keyring();
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        guard
            .prepare(&tx, &KEY, &previous(Some("previous-secret")), TOKEN)
            .unwrap();
        let bytes = fs::read_to_string(&guard.journal).unwrap();
        assert!(bytes.starts_with(ConfigStore::ENC_PREFIX));
        assert!(!bytes.contains("previous-secret"));
        assert!(!bytes.contains(ACCOUNT));
        assert_eq!(
            fs::read_dir(&fixture.0)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(".stage-"))
                .count(),
            0
        );
        set_password("uncommitted-secret");
        guard.recover(&tx, &KEY).unwrap();
        assert_eq!(password().as_deref(), Some("previous-secret"));
        assert!(!guard.journal.exists());
    }

    #[test]
    fn absent_and_present_empty_credentials_remain_distinct() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        install_keyring();
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        for expected in [None, Some("")] {
            guard
                .prepare(&tx, &KEY, &previous(expected), TOKEN)
                .unwrap();
            set_password("changed");
            guard.recover(&tx, &KEY).unwrap();
            assert_eq!(password().as_deref(), expected);
        }
    }

    #[test]
    fn committed_journal_never_restores_previous_credentials() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        install_keyring();
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        guard
            .prepare(&tx, &KEY, &previous(Some("old")), TOKEN)
            .unwrap();
        set_password("committed");
        tx.execute("INSERT INTO meta VALUES('write_revision', ?1)", [TOKEN])
            .unwrap();
        tx.execute_batch("COMMIT; BEGIN IMMEDIATE").unwrap();
        guard.recover(&tx, &KEY).unwrap();
        assert_eq!(password().as_deref(), Some("committed"));
        assert!(!guard.journal.exists());
    }

    #[test]
    fn changed_database_preserves_journal_and_current_credentials() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        install_keyring();
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        guard
            .prepare(&tx, &KEY, &previous(Some("old")), TOKEN)
            .unwrap();
        set_password("unknown-owner");
        tx.execute("INSERT INTO meta VALUES('write_revision', 'different')", [])
            .unwrap();
        tx.execute_batch("COMMIT; BEGIN IMMEDIATE").unwrap();
        let bytes = fs::read(&guard.journal).unwrap();
        let error = guard.recover(&tx, &KEY).unwrap_err();
        assert!(error
            .downcast_ref::<SessionCredentialRollbackFailed>()
            .is_some());
        assert_eq!(password().as_deref(), Some("unknown-owner"));
        assert_eq!(fs::read(&guard.journal).unwrap(), bytes);
    }

    #[test]
    fn corrupt_or_wrong_key_journal_blocks_recovery_without_keyring_changes() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        install_keyring();
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        // The None case still authenticates the key and all metadata.
        guard.prepare(&tx, &KEY, &previous(None), TOKEN).unwrap();
        set_password("current");
        let bytes = fs::read(&guard.journal).unwrap();
        assert!(guard.recover(&tx, &[0; 32]).is_err());
        assert_eq!(fs::read(&guard.journal).unwrap(), bytes);
        fs::write(&guard.journal, "enc:v1:damaged").unwrap();
        assert!(guard.recover(&tx, &KEY).is_err());
        assert_eq!(fs::read(&guard.journal).unwrap(), b"enc:v1:damaged");
        assert_eq!(password().as_deref(), Some("current"));
    }

    #[test]
    fn failed_compensation_remains_recoverable_after_a_new_guard() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        install_keyring();
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        guard
            .prepare(&tx, &KEY, &previous(Some("original")), TOKEN)
            .unwrap();
        set_password("changed");
        fake_keyring::fail_writes_after(0);
        assert!(guard.recover(&tx, &KEY).is_err());
        assert!(guard.journal.exists());
        drop(tx);
        drop(guard);
        install_keyring();
        set_password("changed");
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        guard.recover(&tx, &KEY).unwrap();
        assert_eq!(password().as_deref(), Some("original"));
        assert!(!guard.journal.exists());
    }

    #[test]
    fn prepare_never_replaces_an_unresolved_journal() {
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        guard
            .prepare(&tx, &KEY, &previous(Some("original")), TOKEN)
            .unwrap();
        let original = fs::read(&guard.journal).unwrap();
        assert!(guard
            .prepare(&tx, &KEY, &previous(Some("new")), "other")
            .is_err());
        assert_eq!(fs::read(&guard.journal).unwrap(), original);
    }

    #[test]
    fn pending_probe_is_read_only_including_nonexistent_profiles() {
        let fixture = Fixture::new();
        assert!(!pending_journal(&fixture.database()).unwrap());
        assert!(!pending_journal(&fixture.0.join("not-created").join("sessions.db")).unwrap());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
        let journal = sidecar(&fixture.database(), ".credential-journal");
        fs::write(&journal, "unresolved").unwrap();
        assert!(pending_journal(&fixture.database()).unwrap());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
        assert_eq!(fs::read(journal).unwrap(), b"unresolved");
    }

    #[test]
    fn reused_commit_token_is_rejected_before_journal_publication() {
        let fixture = Fixture::new();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        let tx = begin(&fixture.database());
        tx.execute("INSERT INTO meta VALUES('write_revision', ?1)", [TOKEN])
            .unwrap();
        assert!(guard
            .prepare(&tx, &KEY, &previous(Some("old")), TOKEN)
            .is_err());
        assert!(!guard.journal.exists());
    }

    #[test]
    fn lock_blocks_other_handles_and_releases_without_unlinking() {
        let fixture = Fixture::new();
        let first = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        assert!(ProfileIoGuard::acquire_with_timeout(
            &fixture.database(),
            Duration::from_millis(25)
        )
        .is_err());
        let independent = ProfileIoGuard::acquire(&fixture.0.join("other.db")).unwrap();
        drop(independent);
        drop(first);
        assert!(sidecar(&fixture.database(), ".profile-lock").is_file());
        let _next = ProfileIoGuard::acquire(&fixture.database()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_and_directory_aliases_share_lock_and_journal_identity() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let fixture = Fixture::new();
        let tx = begin(&fixture.database());
        let alias_directory = fixture.0.join("alias-directory");
        symlink(&fixture.0, &alias_directory).unwrap();
        let alias_file = fixture.0.join("alias.db");
        symlink(fixture.database(), &alias_file).unwrap();
        let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
        guard
            .prepare(&tx, &KEY, &previous(Some("old")), TOKEN)
            .unwrap();
        for alias in [
            alias_file,
            alias_directory.join("sessions.db"),
            fixture.0.join(".").join("sessions.db"),
        ] {
            assert!(
                ProfileIoGuard::acquire_with_timeout(&alias, Duration::from_millis(25)).is_err()
            );
            assert_eq!(
                sidecar(&canonical_database(&alias).unwrap(), ".credential-journal"),
                guard.journal
            );
        }
        for path in [
            sidecar(&fixture.database(), ".profile-lock"),
            guard.journal.clone(),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    /// Run only in the child spawned below. process::exit skips Rust drops,
    /// leaving the OS to release its lock and SQLite to recover the database.
    #[test]
    fn crash_writer_fixture() {
        let Some(path) = std::env::var_os("XENTERM_PROFILE_IO_CRASH_FIXTURE") else {
            return;
        };
        let path = PathBuf::from(path);
        let guard = ProfileIoGuard::acquire(&path).unwrap();
        let tx = begin(&path);
        guard
            .prepare(&tx, &KEY, &previous(Some("original")), TOKEN)
            .unwrap();
        tx.execute("INSERT INTO meta VALUES('write_revision', ?1)", [TOKEN])
            .unwrap();
        if std::env::var_os("XENTERM_PROFILE_IO_CRASH_COMMITTED").is_some() {
            tx.execute_batch("COMMIT").unwrap();
        }
        std::process::exit(73);
    }

    #[test]
    fn subprocess_crash_recovers_uncommitted_and_preserves_committed_credentials() {
        let _serial = KEYRING_TESTS.lock().unwrap();
        install_keyring();
        for committed in [false, true] {
            let fixture = Fixture::new();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "profile_io::tests::crash_writer_fixture",
                    "--test-threads=1",
                ])
                .env("XENTERM_PROFILE_IO_CRASH_FIXTURE", fixture.database());
            if committed {
                child.env("XENTERM_PROFILE_IO_CRASH_COMMITTED", "1");
            } else {
                child.env_remove("XENTERM_PROFILE_IO_CRASH_COMMITTED");
            }
            let mut child = child
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("profile crash fixture exceeded its bounded deadline");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(73),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            // Model the persistent OS-keyring side effect in our process-local
            // test double. The child left the real SQL and journal on disk.
            set_password("changed");
            let guard = ProfileIoGuard::acquire(&fixture.database()).unwrap();
            let tx = begin(&fixture.database());
            assert!(guard.journal.exists());
            guard.recover(&tx, &KEY).unwrap();
            assert_eq!(
                password().as_deref(),
                Some(if committed { "changed" } else { "original" })
            );
            assert!(!guard.journal.exists());
        }
    }
}
