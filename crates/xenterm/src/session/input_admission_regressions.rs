// These same regressions also run against the untouched pre-fix protocol module.
use super::*;

#[test]
fn starting_ssh_rejects_handle_input_before_any_prompt_event() {
    let mut fixture = Fixture::ssh();
    let _ = fixture.handle.send_raw(b"startup-keystrokes".to_vec());
    assert!(matches!(
        fixture.receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn starting_ssh_rejects_cloned_command_sender_input() {
    let mut fixture = Fixture::ssh();
    let replies = fixture.handle.commands.clone();
    let result = replies.send(SessionCommand::RawInput(b"terminal-reply".to_vec()));
    assert!(matches!(
        fixture.receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
    assert!(result.is_err());
}

#[test]
fn rejected_startup_input_is_never_replayed_after_admission_opens() {
    let mut fixture = Fixture::ssh();
    let _ = fixture.handle.send_raw(b"before-authentication".to_vec());
    fixture.open();
    let _ = fixture.handle.send_raw(b"after-authentication".to_vec());
    assert!(
        matches!(fixture.receiver.try_recv(), Ok(SessionCommand::RawInput(bytes))
        if bytes == b"after-authentication")
    );
    assert!(matches!(
        fixture.receiver.try_recv(),
        Err(TryRecvError::Empty)
    ));
}
