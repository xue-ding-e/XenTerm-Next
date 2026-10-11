//! Read-only admission checks before an explicit profile can initialize keys,
//! tracing, migrations or a writable SQLite connection.
use std::fs;
use std::path::{Path, PathBuf};

use super::{ConfigFile, ConfigStore};
use anyhow::{bail, Result};

const INCOMPATIBLE: &str = "explicit profile credentials are not usable with its local key; export sessions from the original application and import the portable export into a separate --data-dir";

struct Snapshot(PathBuf);
impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Opening a WAL database even with SQLITE_OPEN_READ_ONLY can create -wal and
/// -shm files in its directory. Inspect a private temporary copy instead, so a
/// rejected profile has no new sidecars or other file changes.
fn read_database(path: &Path, preview: bool) -> Result<Option<ConfigFile>> {
    let directory =
        std::env::temp_dir().join(format!("xenterm-profile-check-{}", uuid::Uuid::new_v4()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&directory)
        .map_err(|_| anyhow::anyhow!("cannot create a private profile inspection snapshot"))?;
    let snapshot = Snapshot(directory);
    let files = [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
    ];
    let state = |path: &Path| -> Result<Option<(u64, std::time::SystemTime)>> {
        match fs::metadata(path) {
            Ok(meta) if meta.is_file() => Ok(Some((meta.len(), meta.modified()?))),
            Ok(_) => bail!("explicit profile database files must be regular files"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => bail!("cannot inspect explicit profile database"),
        }
    };
    let before = files
        .iter()
        .map(|path| state(path))
        .collect::<Result<Vec<_>>>()?;
    for (source, metadata) in files.iter().zip(&before) {
        if metadata.is_some() {
            let name = source
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid profile database filename"))?;
            fs::copy(source, snapshot.0.join(name))
                .map_err(|_| anyhow::anyhow!("cannot read explicit profile database"))?;
        }
    }
    let after = files
        .iter()
        .map(|path| state(path))
        .collect::<Result<Vec<_>>>()?;
    if before != after {
        bail!("profile changed during inspection; stop other users of this profile and retry");
    }
    let db = snapshot.0.join("sessions.db");
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| {
                anyhow::anyhow!(
                    "explicit profile database could not be read; original files preserved"
                )
            })?;
    let raw = ConfigStore::read_disk_store(&conn).map_err(|_| {
        anyhow::anyhow!("explicit profile database could not be read; original files preserved")
    })?;
    let Some((settings, sessions, history)) = raw else {
        // Ordinary startup keeps its existing admission probe, then load
        // checks the complete fingerprint under its transaction. A preview
        // must perform that check here because it never enters writable load.
        let has_rows = if preview {
            ConfigStore::disk_fingerprint(&conn)
                .map_err(|_| anyhow::anyhow!("explicit profile database schema is incomplete"))?
                .is_some()
        } else {
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|_| anyhow::anyhow!("explicit profile database schema is incomplete"))?
                != 0
        };
        if has_rows {
            bail!("explicit profile database settings are missing; original files preserved");
        }
        return Ok(None);
    };
    let mut config: ConfigFile = serde_json::from_str(&settings).map_err(|_| {
        anyhow::anyhow!("explicit profile database settings are invalid; original files preserved")
    })?;
    config.sessions = sessions;
    config.command_history = history;
    Ok(Some(config))
}

impl ConfigStore {
    pub(super) fn preflight_explicit_profile(directory: &Path) -> Result<()> {
        Self::inspect_explicit_profile(directory, false).map(|_| ())
    }

    /// Return the same disk-form snapshot whose credentials were checked, so
    /// a preview never reopens the live database or consults the OS keyring.
    pub(super) fn inspect_explicit_profile(
        directory: &Path,
        preview: bool,
    ) -> Result<(Option<ConfigFile>, Option<[u8; 32]>)> {
        let db = directory.join("sessions.db");
        let json = directory.join("sessions.json");
        if super::profile_io::pending_journal(&db)? {
            bail!("profile has unfinished desktop credential recovery; reopen it in the original desktop application before selecting it as an explicit service profile");
        }
        let from_db = if db.exists() {
            read_database(&db, preview)?
        } else {
            None
        };
        let config = if from_db.is_some() {
            from_db
        } else if json.exists() {
            let raw = fs::read(&json).map_err(|_| {
                anyhow::anyhow!("explicit profile JSON could not be read; original file preserved")
            })?;
            Some(serde_json::from_slice::<ConfigFile>(&raw).map_err(|_| {
                anyhow::anyhow!("explicit profile JSON is invalid; original file preserved")
            })?)
        } else {
            None
        };
        // Keep ordinary admission unchanged for a key-only directory. Its
        // normal key loader validates the file later; preview cannot call it.
        if config.is_none() && !preview {
            return Ok((None, None));
        }
        let key_path = directory.join("secret.key");
        let key = if key_path.exists() {
            let raw = fs::read(key_path).map_err(|_| anyhow::anyhow!(INCOMPATIBLE))?;
            Some(<[u8; 32]>::try_from(raw.as_slice()).map_err(|_| anyhow::anyhow!(INCOMPATIBLE))?)
        } else {
            None
        };
        let Some(config) = config else {
            return Ok((None, key));
        };
        let check = |value: &str, keyring_possible: bool| -> Result<()> {
            if keyring_possible && value == Self::KEYRING_MARKER {
                bail!(INCOMPATIBLE);
            }
            if value.starts_with(Self::ENC_PREFIX) {
                let key = key.as_ref().ok_or_else(|| anyhow::anyhow!(INCOMPATIBLE))?;
                if Self::try_decrypt(key, value).is_none() {
                    bail!(INCOMPATIBLE);
                }
            } else if value.starts_with("enc:") {
                bail!(INCOMPATIBLE);
            }
            Ok(())
        };
        check(config.webdav_password.as_str(), false)?;
        for session in &config.sessions {
            check(session.password.as_str(), true)?;
            check(session.private_key_inline.as_str(), false)?;
            for trigger in &session.triggers {
                check(trigger.response.as_str(), false)?;
            }
            let mut invalid = false;
            Self::map_proxy_password(&session.proxy, |password| {
                invalid = check(password, false).is_err();
                None
            });
            if invalid {
                bail!(INCOMPATIBLE);
            }
        }
        Ok((Some(config), key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_checks_key_only_profiles_without_changing_ordinary_admission() {
        let directory = tempfile::tempdir().unwrap();
        let key = directory.path().join("secret.key");
        fs::write(&key, b"synthetic malformed key").unwrap();
        assert!(ConfigStore::preflight_explicit_profile(directory.path()).is_ok());
        assert!(ConfigStore::inspect_explicit_profile(directory.path(), true).is_err());
        assert_eq!(fs::read(key).unwrap(), b"synthetic malformed key");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn preview_checks_partial_schema_without_changing_ordinary_admission() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(ConfigStore::SCHEMA_SQL).unwrap();
        connection
            .execute("INSERT INTO meta(key,value) VALUES('schema_version','1')", [])
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(ConfigStore::preflight_explicit_profile(directory.path()).is_ok());
        assert!(ConfigStore::inspect_explicit_profile(directory.path(), true).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
