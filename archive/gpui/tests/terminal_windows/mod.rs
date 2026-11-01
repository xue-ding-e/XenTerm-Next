use super::*;
use crate::config::Session;
use crate::session::protocol::{SessionCommand, SessionEvent};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

struct CloseOnDrop(UnboundedSender<SessionCommand>);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(SessionCommand::Close);
    }
}

#[cfg(windows)]
#[tokio::test]
async fn conpty_powershell_preserves_long_unicode_output() {
    let mut session = Session::new_empty();
    session.host = "powershell".into();
    session.name = "xenterm-conpty-test".into();
    let (handle, mut events) = local::spawn_local_session(
        &tokio::runtime::Handle::current(),
        "conpty-test".into(),
        session,
        120,
        30,
    );
    let cleanup = CloseOnDrop(handle.commands.clone());
    let mut terminal = TermBuffer::new(30, 120);
    let mut output = String::new();
    let mut connected = false;
    let mut closed = false;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = events.recv().await.expect("ConPTY event channel closed");
            match event {
                SessionEvent::Connected => {
                    assert!(!connected);
                    connected = true;
                    let command = concat!(
                        "$u='中文'+[char]::ConvertFromUtf32(0x1F600); ",
                        "[Console]::Write(([char]27+'[31m'+('BEGIN-'+'MARKER')",
                        "+[char]27+'[0m'+($u*5000)+('END-'+'MARKER'))); exit\r\n"
                    );
                    handle
                        .commands
                        .send(SessionCommand::RawInput(command.as_bytes().to_vec()))
                        .expect("send PowerShell command");
                }
                SessionEvent::Output(chunk) => {
                    let replies = terminal.ingest(chunk.as_bytes());
                    if !replies.is_empty() {
                        handle
                            .commands
                            .send(SessionCommand::RawInput(replies))
                            .expect("reply to shell terminal query");
                    }
                    output.push_str(&chunk);
                }
                SessionEvent::Closed(_) => closed = true,
                _ => {}
            }
            if closed && output.contains("END-MARKER") {
                break;
            }
        }
    })
    .await;
    drop(cleanup);
    let joined = tokio::time::timeout(Duration::from_secs(2), handle.join).await;
    assert!(joined.is_ok(), "local session worker did not stop");
    assert!(result.is_ok(), "ConPTY output/close exceeded 15 seconds");
    assert!(connected);
    assert!(closed);
    let begin = output.find("BEGIN-MARKER").expect("begin marker");
    let end = output.rfind("END-MARKER").expect("end marker");
    let payload = &output[begin + "BEGIN-MARKER".len()..end];
    // ConPTY emits redraws as well as new text, so its VT transport is not a
    // plain stdout transcript. Count logical text only after applying the VT
    // stream; raw chunks still must decode without replacement characters.
    assert!(!payload.contains('\u{fffd}'));

    terminal.render();
    let retained = terminal
        .history_lines()
        .map(|line| line.0)
        .chain(terminal.displayed_text.iter().cloned())
        .map(|line| line.trim_end().to_string())
        .collect::<String>();
    let begin = retained
        .find("BEGIN-MARKER")
        .expect("rendered begin marker");
    let end = retained.rfind("END-MARKER").expect("rendered end marker");
    let payload = &retained[begin + "BEGIN-MARKER".len()..end];
    assert_eq!(payload.matches("中文").count(), 5000);
    assert_eq!(payload.matches('😀').count(), 5000);
    assert!(!payload.contains('\u{fffd}'));
}
