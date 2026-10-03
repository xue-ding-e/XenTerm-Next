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
fn read_database(path: &Path) -> Result<Option<ConfigFile>> {
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
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .map_err(|_| anyhow::anyhow!("explicit profile database schema is incomplete"))?;
        if rows != 0 {
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
        let db = directory.join("sessions.db");
        let json = directory.join("sessions.json");
        if super::profile_io::pending_journal(&db)? {
            bail!("profile has unfinished desktop credential recovery; reopen it in the original desktop application before selecting it as an explicit service profile");
        }
        let from_db = if db.exists() {
            read_database(&db)?
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
        let Some(config) = config else {
            return Ok(());
        };
        let key_path = directory.join("secret.key");
        let key = if key_path.exists() {
            let raw = fs::read(key_path).map_err(|_| anyhow::anyhow!(INCOMPATIBLE))?;
            Some(<[u8; 32]>::try_from(raw.as_slice()).map_err(|_| anyhow::anyhow!(INCOMPATIBLE))?)
        } else {
            None
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
        Ok(())
    }
}
