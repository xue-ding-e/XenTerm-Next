//! Bounded, no-clobber uploads for OS drops only. Legacy uploads are unchanged.
use super::*;
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, Semaphore};

#[path = "drop_policy.rs"]
mod policy;
use policy::{destination, Reservations, MAX_DROPPED};

type Cancels = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;
type Notifications = Arc<Mutex<HashMap<String, (Arc<Notify>, Arc<AtomicBool>)>>>;

pub(super) struct DropQueue {
    slots: Arc<Semaphore>,
    destinations: Arc<Mutex<Reservations>>,
    notifications: Notifications,
}

// Releasing a slot, destination and cancellation entry also works on task unwind.
struct Reservation {
    destinations: Arc<Mutex<Reservations>>,
    cancels: Cancels,
    remote: String,
    id: String,
    notifications: Notifications,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.destinations.lock().unwrap().release(&self.remote);
        self.cancels.lock().unwrap().remove(&self.id);
        self.notifications.lock().unwrap().remove(&self.id);
    }
}

impl Drop for DropQueue {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

impl DropQueue {
    pub(super) fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
            destinations: Arc::default(),
            notifications: Arc::default(),
        }
    }

    pub(super) fn wake_cancelled(&self, id: &str) {
        if let Some((notify, _)) = self.notifications.lock().unwrap().get(id) {
            notify.notify_one();
        }
    }

    pub(super) fn cancel_all(&self) {
        for (notify, cancel) in self.notifications.lock().unwrap().values() {
            cancel.store(true, Ordering::Relaxed);
            notify.notify_one();
        }
    }

    pub(super) fn enqueue(
        &self,
        locals: Vec<PathBuf>,
        remote_dir: String,
        handle: Arc<client::Handle<SftpClientHandler>>,
        events: UnboundedSender<SessionEvent>,
        cancels: Cancels,
    ) {
        if locals.is_empty() || locals.len() > MAX_DROPPED {
            let _ = events.send(SessionEvent::SftpStatus(
                t(
                    "一次最多拖入 128 个普通文件。",
                    "Drop between 1 and 128 regular files at a time.",
                )
                .into(),
            ));
            return;
        }
        for local in locals {
            let id = Uuid::new_v4().to_string();
            let name = local
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let remote = match destination(&local, &remote_dir).and_then(|remote| {
                self.destinations.lock().unwrap().reserve(&remote)?;
                Ok(remote)
            }) {
                Ok(remote) => remote,
                Err(error) => {
                    emit_transfer(&events, &id, &name, true, 0, 0, 2, error);
                    continue;
                }
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let notify = Arc::new(Notify::new());
            cancels.lock().unwrap().insert(id.clone(), cancel.clone());
            self.notifications
                .lock()
                .unwrap()
                .insert(id.clone(), (notify.clone(), cancel.clone()));
            let reservation = Reservation {
                destinations: self.destinations.clone(),
                cancels: cancels.clone(),
                remote: remote.clone(),
                id: id.clone(),
                notifications: self.notifications.clone(),
            };
            let slots = self.slots.clone();
            let handle = handle.clone();
            let events = events.clone();
            emit_transfer(
                &events,
                &id,
                &name,
                true,
                0,
                0,
                0,
                t(
                    "排队中（同名不覆盖）",
                    "Queued (existing files are never replaced)",
                ),
            );
            tokio::spawn(async move {
                let _reservation = reservation;
                let result = async {
                    let Some(_slot) = wait_slot(slots, &cancel, &notify).await? else {
                        return Ok(false);
                    };
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(false);
                    }
                    let mut file = open_regular(&local).await?;
                    let total = file.metadata().await?.len();
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(false);
                    }
                    let channel = tokio::time::timeout(
                        Duration::from_secs(15),
                        handle.channel_open_session(),
                    )
                    .await??;
                    tokio::time::timeout(
                        Duration::from_secs(15),
                        channel.request_subsystem(true, "sftp"),
                    )
                    .await??;
                    let raw = RawSftpSession::new(
                        super::super::capped_stream::CappedSftpStream::new(channel.into_stream()),
                    );
                    raw.init().await.context("dropped upload handshake")?;
                    let result = upload_exclusive(
                        &raw, &mut file, &remote, &name, &id, total, &events, &cancel,
                    )
                    .await;
                    let _ = raw.close_session();
                    result
                }
                .await;
                match result {
                    Ok(true) => {}
                    Ok(false) => emit_transfer(&events, &id, &name, true, 0, 0, 4, t("已取消；如已创建远程文件，保留不完整文件供检查。", "Cancelled; any newly created remote partial file is retained for inspection.")),
                    Err(error) => emit_transfer(&events, &id, &name, true, 0, 0, 2, &format!("{error:#}; {}", t("不覆盖已有文件；可能存在新建的不完整文件。", "Existing files are not replaced; a newly created partial file may remain."))),
                }
            });
        }
    }
}

async fn wait_slot(
    slots: Arc<Semaphore>,
    cancel: &AtomicBool,
    notify: &Notify,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>> {
    let acquire = slots.acquire_owned();
    tokio::pin!(acquire);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        tokio::select! {
            permit = &mut acquire => return Ok(Some(permit?)),
            _ = notify.notified() => {}
        }
    }
}

async fn open_regular(local: &Path) -> Result<tokio::fs::File> {
    // The initial check provides a useful error; NOFOLLOW / OPEN_REPARSE_POINT
    // below prevents a swapped final symlink from being followed at open time.
    let metadata = tokio::fs::symlink_metadata(local).await?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "Only regular files may be dropped (no directories or links)"
    );
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    let file = options
        .open(local)
        .await
        .context("open dropped regular file")?;
    let opened = file.metadata().await?;
    anyhow::ensure!(
        opened.is_file(),
        "Dropped source is no longer a regular file"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(
            opened.file_attributes() & 0x400 == 0,
            "Dropped reparse points are not supported"
        );
    }
    Ok(file)
}

/// Real SFTP v3 OPEN uses EXCLUDE (0x20), never TRUNCATE. There is no stat-then-
/// overwrite fallback. CLOSE acknowledgement is required before success.
/// On failure/cancel we do not REMOVE by pathname: another actor may have
/// renamed/replaced that path after OPEN. Report the retained partial instead.
async fn upload_exclusive<R: tokio::io::AsyncRead + Unpin>(
    raw: &RawSftpSession,
    file: &mut R,
    remote: &str,
    name: &str,
    id: &str,
    total: u64,
    events: &UnboundedSender<SessionEvent>,
    cancel: &AtomicBool,
) -> Result<bool> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    let handle = raw
        .open(
            remote,
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::EXCLUDE,
            // `default()` in russh-sftp 2.1.1 sends uid/gid 0, mode 0777|DIR,
            // size and timestamps. Leave attributes absent instead: the server
            // applies its regular-file creation policy, as SftpSession::open does.
            FileAttributes::empty(),
        )
        .await
        .context("exclusive create (existing names are rejected)")?
        .handle;
    let mut offset = 0u64;
    let transfer: Result<bool> = async {
        let mut buffer = vec![0; 32 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let count =
                tokio::time::timeout(Duration::from_secs(15), file.read(&mut buffer)).await??;
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            if count == 0 {
                break;
            }
            raw.write(handle.clone(), offset, buffer[..count].to_vec())
                .await
                .context("write dropped file")?;
            offset += count as u64;
            emit_transfer(events, id, name, true, offset, total.max(offset), 0, "");
        }
        Ok(true)
    }
    .await;
    // Even cancellation awaits CLOSE. A failed CLOSE never becomes completion.
    let closed = raw.close(handle).await.context("close dropped file");
    let success = transfer?;
    closed?;
    if success && !cancel.load(Ordering::Relaxed) {
        // Do not push a listing for this old destination after navigation.
        // Completion is visible; an explicit refresh keeps directory ownership.
        emit_transfer(events, id, name, true, offset, offset, 1, t("上传完成；刷新目录查看。", "Uploaded; refresh the directory to view."));
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
#[path = "drop_upload_tests.rs"]
mod tests;
