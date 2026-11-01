use super::{event_requires_immediate_ui, record_ingested_chunk, INGEST_FRAME_BUDGET};
use crate::app::session_runtime::take_closed_event;
use crate::session::protocol::SessionEvent;

fn count_requests(chunk_lengths: &[usize]) -> (usize, usize) {
    let mut since_checkpoint = 0usize;
    let mut requests = 0usize;
    let mut dirty_since_request = false;
    for &chunk_len in chunk_lengths {
        dirty_since_request = true;
        if record_ingested_chunk(chunk_len, &mut since_checkpoint) {
            requests += 1;
            dirty_since_request = false;
        }
    }
    if dirty_since_request {
        requests += 1;
    }
    (requests, since_checkpoint)
}

#[test]
fn exact_frame_budget_chunks_do_not_add_an_empty_tail_request() {
    let (requests, remainder) = count_requests(&[INGEST_FRAME_BUDGET, INGEST_FRAME_BUDGET]);
    assert_eq!(requests, 2);
    assert_eq!(remainder, 0);
}

#[test]
fn a_partial_tail_gets_one_final_request() {
    let (requests, remainder) = count_requests(&[INGEST_FRAME_BUDGET, INGEST_FRAME_BUDGET, 1]);
    assert_eq!(requests, 3);
    assert_eq!(remainder, 1);
}

#[test]
fn checkpoint_budget_carries_across_input_events() {
    let mut since_checkpoint = 0usize;
    assert!(!record_ingested_chunk(
        INGEST_FRAME_BUDGET - 1,
        &mut since_checkpoint
    ));
    assert!(record_ingested_chunk(1, &mut since_checkpoint));
    assert_eq!(since_checkpoint, 0);
}

#[test]
fn an_oversized_output_event_stays_one_atomic_checkpoint() {
    let (requests, remainder) = count_requests(&[INGEST_FRAME_BUDGET * 2 + 1]);
    assert_eq!(requests, 1);
    assert_eq!(remainder, 1);
}

#[test]
fn routine_shell_metadata_does_not_disable_tail_pacing() {
    assert!(!event_requires_immediate_ui(&SessionEvent::CommandRan(
        "tail -n 1000000 app.log".into()
    )));
    assert!(!event_requires_immediate_ui(&SessionEvent::CwdChanged(
        "/var/log".into()
    )));
    assert!(event_requires_immediate_ui(&SessionEvent::Connected));
    assert!(event_requires_immediate_ui(&SessionEvent::Closed(
        "connection lost".into()
    )));
}

#[test]
fn closed_event_discards_stale_output_backlog() {
    let mut events = vec![
        SessionEvent::Output("old-1".into()),
        SessionEvent::Output("old-2".into()),
        SessionEvent::Closed("connection closed".into()),
        SessionEvent::Output("stale-after-close".into()),
    ];

    let closed = take_closed_event(&mut events);

    assert!(matches!(closed, Some(SessionEvent::Closed(reason)) if reason == "connection closed"));
    assert!(events.is_empty());
}
