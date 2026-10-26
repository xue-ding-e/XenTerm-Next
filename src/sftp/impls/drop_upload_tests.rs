use super::*;
use tokio::io::{AsyncWriteExt, DuplexStream};

const STEP: Duration = Duration::from_secs(3);
const REMOTE: &str = "/synthetic/中文 file";
const CONTENT: &[u8] = b"synthetic drop bytes";

#[derive(Clone, Copy)]
enum Case {
    Success,
    Conflict,
    Permission,
    CancelBefore,
    CancelAfterOpen,
    CancelAfterWrite,
    WriteFailure,
    CloseFailure,
    Disconnect,
}

fn field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn take<'a>(bytes: &mut &'a [u8], len: usize) -> &'a [u8] {
    assert!(bytes.len() >= len);
    let (first, rest) = bytes.split_at(len);
    *bytes = rest;
    first
}

fn number(bytes: &mut &[u8]) -> u32 {
    u32::from_be_bytes(take(bytes, 4).try_into().unwrap())
}
fn string<'a>(bytes: &mut &'a [u8]) -> &'a [u8] {
    let len = number(bytes) as usize;
    take(bytes, len)
}

async fn packet(stream: &mut DuplexStream) -> Vec<u8> {
    tokio::time::timeout(STEP, async {
        let len = stream.read_u32().await.unwrap() as usize;
        assert!((1..=65536).contains(&len));
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes).await.unwrap();
        bytes
    })
    .await
    .expect("bounded synthetic packet")
}

async fn reply(stream: &mut DuplexStream, bytes: &[u8]) {
    tokio::time::timeout(STEP, async {
        stream.write_u32(bytes.len() as u32).await.unwrap();
        stream.write_all(bytes).await.unwrap();
        stream.flush().await.unwrap();
    })
    .await
    .expect("bounded synthetic reply");
}

async fn status(stream: &mut DuplexStream, id: u32, code: u32) {
    let mut bytes = vec![101];
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&code.to_be_bytes());
    field(&mut bytes, b"synthetic status");
    field(&mut bytes, b"");
    reply(stream, &bytes).await;
}

async fn eof(stream: &mut DuplexStream) {
    // Any retry OPEN, WRITE, REMOVE or replacement fallback fails this oracle.
    let mut one = [0];
    assert_eq!(
        tokio::time::timeout(STEP, stream.read(&mut one))
            .await
            .unwrap()
            .unwrap(),
        0
    );
}

async fn peer(mut stream: DuplexStream, case: Case, cancel: Arc<AtomicBool>) {
    assert_eq!(packet(&mut stream).await, [1, 0, 0, 0, 3]);
    reply(&mut stream, &[2, 0, 0, 0, 3]).await;
    if matches!(case, Case::CancelBefore) {
        eof(&mut stream).await;
        return;
    }
    let bytes = packet(&mut stream).await;
    let mut p = bytes.as_slice();
    assert_eq!(take(&mut p, 1), [3]);
    let id = number(&mut p);
    assert_eq!(string(&mut p), REMOTE.as_bytes());
    assert_eq!(number(&mut p), 0x2a, "WRITE|CREATE|EXCLUDE, never TRUNCATE");
    assert_eq!(number(&mut p), 0);
    assert!(p.is_empty());
    if matches!(case, Case::Conflict | Case::Permission) {
        status(
            &mut stream,
            id,
            if matches!(case, Case::Permission) {
                3
            } else {
                4
            },
        )
        .await;
        eof(&mut stream).await;
        return;
    }
    if matches!(case, Case::Disconnect) {
        return;
    }
    if matches!(case, Case::CancelAfterOpen) {
        cancel.store(true, Ordering::Relaxed);
    }
    let mut handle = vec![102];
    handle.extend_from_slice(&id.to_be_bytes());
    field(&mut handle, b"owned");
    reply(&mut stream, &handle).await;
    if !matches!(case, Case::CancelAfterOpen) {
        let bytes = packet(&mut stream).await;
        let mut p = bytes.as_slice();
        assert_eq!(take(&mut p, 1), [6]);
        let id = number(&mut p);
        assert_eq!(string(&mut p), b"owned");
        assert_eq!(take(&mut p, 8), [0; 8]);
        assert_eq!(string(&mut p), CONTENT);
        assert!(p.is_empty());
        if matches!(case, Case::CancelAfterWrite) {
            cancel.store(true, Ordering::Relaxed);
        }
        status(
            &mut stream,
            id,
            if matches!(case, Case::WriteFailure) {
                3
            } else {
                0
            },
        )
        .await;
    }
    let bytes = packet(&mut stream).await;
    let mut p = bytes.as_slice();
    assert_eq!(take(&mut p, 1), [4], "must CLOSE, never REMOVE");
    let id = number(&mut p);
    assert_eq!(string(&mut p), b"owned");
    assert!(p.is_empty());
    status(
        &mut stream,
        id,
        if matches!(case, Case::CloseFailure) {
            4
        } else {
            0
        },
    )
    .await;
    eof(&mut stream).await;
}

async fn exercise(case: Case) {
    let (client, server) = tokio::io::duplex(65536);
    let cancel = Arc::new(AtomicBool::new(matches!(case, Case::CancelBefore)));
    let task = tokio::spawn(peer(server, case, cancel.clone()));
    let raw = RawSftpSession::new(super::super::super::capped_stream::CappedSftpStream::new(
        client,
    ));
    raw.set_timeout(1).await;
    raw.init().await.unwrap();
    let (events, mut receiver) = mpsc::unbounded_channel();
    let mut source = CONTENT;
    let result = tokio::time::timeout(
        STEP,
        upload_exclusive(
            &raw,
            &mut source,
            REMOTE,
            "drop",
            "id",
            CONTENT.len() as u64,
            &events,
            &cancel,
        ),
    )
    .await;
    raw.close_session().unwrap();
    drop(raw);
    let joined = tokio::time::timeout(STEP, task).await;
    joined
        .expect("bounded synthetic peer")
        .expect("synthetic protocol assertions");
    let result = result.expect("bounded upload");
    let mut completed = 0;
    while let Ok(event) = receiver.try_recv() {
        if let SessionEvent::SftpTransfer {
            state: 1,
            transferred,
            total,
            ..
        } = event
        {
            assert_eq!(transferred, CONTENT.len() as u64);
            assert_eq!(total, transferred);
            completed += 1;
        }
    }
    match case {
        Case::Success => {
            assert!(result.unwrap());
            assert_eq!(completed, 1);
        }
        Case::CancelBefore | Case::CancelAfterOpen | Case::CancelAfterWrite => {
            assert!(!result.unwrap());
            assert_eq!(completed, 0);
        }
        _ => {
            assert!(result.is_err());
            assert_eq!(completed, 0);
        }
    }
}

#[tokio::test]
async fn exclusive_open_success_requires_write_and_close_ack() {
    exercise(Case::Success).await;
}
#[tokio::test]
async fn existing_name_has_no_write_remove_or_overwrite_fallback() {
    exercise(Case::Conflict).await;
}
#[tokio::test]
async fn permission_denial_has_no_fallback() {
    exercise(Case::Permission).await;
}
#[tokio::test]
async fn cancelled_before_open_sends_no_file_request() {
    exercise(Case::CancelBefore).await;
}
#[tokio::test]
async fn cancel_after_create_closes_without_write_or_remove() {
    exercise(Case::CancelAfterOpen).await;
}
#[tokio::test]
async fn cancel_after_write_closes_and_retains_partial() {
    exercise(Case::CancelAfterWrite).await;
}
#[tokio::test]
async fn write_failure_is_not_completion_and_still_closes() {
    exercise(Case::WriteFailure).await;
}
#[tokio::test]
async fn close_failure_cannot_report_completed() {
    exercise(Case::CloseFailure).await;
}
#[tokio::test]
async fn disconnect_cannot_report_completed() {
    exercise(Case::Disconnect).await;
}

#[tokio::test]
async fn queued_cancel_wakes_without_waiting_for_a_slot() {
    let slots = Arc::new(Semaphore::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(Notify::new());
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let notify = notify.clone();
        async move { wait_slot(slots, &cancel, &notify).await }
    });
    cancel.store(true, Ordering::Relaxed);
    notify.notify_one();
    assert!(tokio::time::timeout(STEP, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn only_two_transfers_acquire_slots_and_cancel_releases_reservations() {
    let queue = DropQueue::new();
    let first = queue.slots.clone().try_acquire_owned().unwrap();
    let second = queue.slots.clone().try_acquire_owned().unwrap();
    assert!(queue.slots.clone().try_acquire_owned().is_err());
    drop(first);
    assert!(queue.slots.clone().try_acquire_owned().is_ok());
    drop(second);
    let cancels: Cancels = Arc::default();
    queue
        .destinations
        .lock()
        .unwrap()
        .reserve("/queued")
        .unwrap();
    cancels
        .lock()
        .unwrap()
        .insert("id".into(), Arc::new(AtomicBool::new(false)));
    queue.notifications.lock().unwrap().insert(
        "id".into(),
        (Arc::new(Notify::new()), Arc::new(AtomicBool::new(false))),
    );
    drop(Reservation {
        destinations: queue.destinations.clone(),
        cancels: cancels.clone(),
        remote: "/queued".into(),
        id: "id".into(),
        notifications: queue.notifications.clone(),
    });
    assert!(cancels.lock().unwrap().is_empty());
    assert!(queue.notifications.lock().unwrap().is_empty());
    assert!(queue
        .destinations
        .lock()
        .unwrap()
        .reserve("/queued")
        .is_ok());
}

#[tokio::test]
async fn worker_shutdown_cancels_and_wakes_queued_drops() {
    let queue = DropQueue::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(Notify::new());
    queue
        .notifications
        .lock()
        .unwrap()
        .insert("queued".into(), (notify.clone(), cancel.clone()));
    drop(queue);
    assert!(cancel.load(Ordering::Relaxed));
    tokio::time::timeout(STEP, notify.notified()).await.unwrap();
}

#[tokio::test]
async fn local_directories_and_links_are_rejected_regular_files_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("中文 file");
    std::fs::write(&path, CONTENT).unwrap();
    assert!(open_regular(dir.path()).await.is_err());
    let mut file = open_regular(&path).await.unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, CONTENT);
    #[cfg(unix)]
    {
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(open_regular(&link).await.is_err());
    }
}
