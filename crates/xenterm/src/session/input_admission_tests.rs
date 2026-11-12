use super::*;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc::error::TryRecvError;

// No worker, network, PTY, profile or authentication service is run. The
// production handle, admission sender and Tokio command queue are real.
struct Fixture {
    _runtime: Runtime,
    handle: SessionHandle,
    receiver: UnboundedReceiver<SessionCommand>,
    ready: Option<Box<dyn FnOnce()>>,
}

impl Fixture {
    fn ssh() -> Self {
        let runtime = Builder::new_current_thread().build().unwrap();
        let (commands, receiver, ready) = SessionCommandSender::awaiting_shell();
        let handle = SessionHandle {
            tab_id: "synthetic-input-admission".into(),
            commands,
            join: runtime.spawn(std::future::pending()),
        };
        Self {
            _runtime: runtime,
            handle,
            receiver,
            ready: Some(Box::new(move || ready.mark_ready())),
        }
    }

    fn open(&mut self) {
        self.ready.take().unwrap()();
    }
}

#[path = "input_admission_regressions.rs"]
mod regressions;

#[test]
fn send_raw_reports_actual_admission_and_closed_receiver() {
    let mut fixture = Fixture::ssh();
    assert!(!fixture.handle.send_raw(b"before".to_vec()));
    fixture.open();
    assert!(fixture.handle.send_raw(b"after".to_vec()));
    drop(fixture.receiver);
    assert!(!fixture.handle.send_raw(b"after-receiver-drop".to_vec()));
}

#[test]
fn prompt_answers_do_not_open_the_command_gate() {
    let mut fixture = Fixture::ssh();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    HostKeyResponder::new(tx).respond(true);
    assert!(rx.try_recv().unwrap());
    assert!(!fixture.handle.send_raw(b"after-host-key".to_vec()));
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    CredentialResponder::new(tx).respond(Some(("fixture".into(), "test-only".into(), false)));
    assert!(rx.try_recv().unwrap().is_some());
    assert!(!fixture.handle.send_raw(b"after-credentials".to_vec()));
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    MfaResponder::new(tx).respond(Some("fixture-answer".into()));
    assert!(rx.try_recv().unwrap().is_some());
    assert!(!fixture.handle.send_raw(b"after-mfa".to_vec()));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn non_ssh_type_ahead_stays_available_before_the_worker_reads() {
    let (sender, mut receiver) = SessionCommandSender::channel();
    sender
        .send(SessionCommand::RawInput(b"type-ahead".to_vec()))
        .unwrap();
    assert!(
        matches!(receiver.try_recv(), Ok(SessionCommand::RawInput(bytes))
        if bytes == b"type-ahead")
    );
}

#[test]
fn ready_shell_keeps_text_sudo_and_terminal_query_replies() {
    let mut fixture = Fixture::ssh();
    fixture.open();
    assert!(fixture.handle.send_raw(b"sudo command\r".to_vec()));
    assert!(fixture.handle.send_raw(b"synthetic-password\r".to_vec()));
    fixture
        .handle
        .commands
        .clone()
        .send(SessionCommand::RawInput(b"\x1b[1;1R".to_vec()))
        .unwrap();
    for expected in [
        b"sudo command\r".as_slice(),
        b"synthetic-password\r",
        b"\x1b[1;1R",
    ] {
        assert!(
            matches!(fixture.receiver.try_recv(), Ok(SessionCommand::RawInput(bytes))
            if bytes == expected)
        );
    }
}

#[test]
fn controls_remain_usable_during_startup() {
    let mut fixture = Fixture::ssh();
    fixture.handle.resize(120, 40);
    fixture.handle.set_resource_monitoring(false);
    let _reply = fixture.handle.kill_process(42, None);
    fixture.handle.close();
    assert!(matches!(
        fixture.receiver.try_recv(),
        Ok(SessionCommand::Resize(120, 40))
    ));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Ok(SessionCommand::SetResourceMonitoring(false))
    ));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Ok(SessionCommand::KillProcess { pid: 42, .. })
    ));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Ok(SessionCommand::Close)
    ));
}

#[test]
fn close_before_ready_cannot_be_reopened_by_the_worker() {
    let mut fixture = Fixture::ssh();
    fixture.handle.close();
    fixture.open();
    assert!(!fixture.handle.send_raw(b"late-ready".to_vec()));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Ok(SessionCommand::Close)
    ));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn close_after_ready_closes_all_sender_clones() {
    let mut fixture = Fixture::ssh();
    let clone = fixture.handle.commands.clone();
    fixture.open();
    fixture.handle.close();
    assert_eq!(
        clone.send(SessionCommand::RawInput(b"late-reply".to_vec())),
        Err(SessionSendError::Closed)
    );
    assert!(matches!(
        fixture.receiver.try_recv(),
        Ok(SessionCommand::Close)
    ));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn dropped_readiness_token_never_opens_input() {
    let (sender, mut receiver, ready) = SessionCommandSender::awaiting_shell();
    drop(ready); // authentication error/cancellation exits before readiness
    assert_eq!(
        sender.send(SessionCommand::RawInput(b"cancelled".to_vec())),
        Err(SessionSendError::InputNotReady)
    );
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn readiness_token_does_not_keep_the_command_queue_connected() {
    let (sender, mut receiver, ready) = SessionCommandSender::awaiting_shell();
    drop(sender);
    assert!(matches!(
        receiver.try_recv(),
        Err(TryRecvError::Disconnected)
    ));
    ready.mark_ready();
    assert!(matches!(
        receiver.try_recv(),
        Err(TryRecvError::Disconnected)
    ));
}

#[test]
fn poisoned_admission_lock_fails_closed() {
    let (sender, mut receiver, ready) = SessionCommandSender::awaiting_shell();
    let input = sender.input.clone();
    assert!(std::thread::spawn(move || {
        let _guard = input.lock().unwrap();
        panic!("synthetic admission poison");
    })
    .join()
    .is_err());
    ready.mark_ready();
    assert_eq!(
        sender.send(SessionCommand::RawInput(b"after-poison".to_vec())),
        Err(SessionSendError::Closed)
    );
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn concurrent_ready_and_close_cannot_reopen_input() {
    for _ in 0..32 {
        let (sender, mut receiver, ready) = SessionCommandSender::awaiting_shell();
        let closer = sender.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let other = barrier.clone();
        let worker = std::thread::spawn(move || {
            other.wait();
            ready.mark_ready();
        });
        barrier.wait();
        closer.send(SessionCommand::Close).unwrap();
        worker.join().unwrap();
        assert_eq!(
            sender.send(SessionCommand::RawInput(b"after-close".to_vec())),
            Err(SessionSendError::Closed)
        );
        assert!(matches!(receiver.try_recv(), Ok(SessionCommand::Close)));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }
}

#[test]
fn concurrent_input_is_never_enqueued_behind_close() {
    for _ in 0..32 {
        let (sender, mut receiver) = SessionCommandSender::channel();
        let writer = sender.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let other = barrier.clone();
        let worker = std::thread::spawn(move || {
            other.wait();
            writer
                .send(SessionCommand::RawInput(b"racing-input".to_vec()))
                .is_ok()
        });
        barrier.wait();
        sender.send(SessionCommand::Close).unwrap();
        let accepted = worker.join().unwrap();
        if accepted {
            assert!(matches!(
                receiver.try_recv(),
                Ok(SessionCommand::RawInput(_))
            ));
        }
        assert!(matches!(receiver.try_recv(), Ok(SessionCommand::Close)));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }
}
