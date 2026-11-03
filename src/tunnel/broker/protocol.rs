//! Public wire input intentionally contains no owner, principal, profile path,
//! credential, authorization deadline or selectable adapter mode.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

pub(super) const VERSION: u16 = 1;
pub(super) const MAX_FRAME: usize = 64 * 1024;
pub(super) const MAX_IN_FLIGHT: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub sequence: u64,
    pub epoch: String,
    pub operation: Operation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Operation {
    Acquire { session_id: String, forward_index: usize, ensure: bool },
    Snapshot {},
    LocalView {},
    Release { lease: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Error {
    Unauthorized,
    Unsupported,
    StaleEpoch,
    ResultExpired,
    SequenceConflict,
    Busy,
    ConfigChanged,
    PortInUse,
    Stopping,
    NotFound,
    Revoked,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Replay<T> {
    New,
    Pending,
    Completed(T),
}

/// A high-water mark survives completed-result eviction. An old acquire must
/// never be interpreted as new after losing its cached reply. This ledger is
/// connection-local; reconnecting cannot adopt its leases or request numbers.
pub(super) struct RequestLedger<T> {
    high_water: u64,
    records: BTreeMap<u64, (Operation, Option<T>)>,
    completed: VecDeque<u64>,
    capacity: usize,
}

impl<T: Clone> RequestLedger<T> {
    pub fn new(capacity: usize) -> Self {
        Self { high_water: 0, records: BTreeMap::new(), completed: VecDeque::new(), capacity }
    }

    pub fn begin(&mut self, sequence: u64, operation: &Operation) -> Result<Replay<T>, Error> {
        if let Some((previous, result)) = self.records.get(&sequence) {
            if previous != operation { return Err(Error::SequenceConflict); }
            return Ok(match result { Some(value) => Replay::Completed(value.clone()), None => Replay::Pending });
        }
        if sequence <= self.high_water { return Err(Error::ResultExpired); }
        if self.records.values().filter(|(_, value)| value.is_none()).count() >= MAX_IN_FLIGHT {
            return Err(Error::Busy);
        }
        self.high_water = sequence;
        self.records.insert(sequence, (operation.clone(), None));
        Ok(Replay::New)
    }

    pub fn complete(&mut self, sequence: u64, result: T) -> Result<(), Error> {
        let (_, value) = self.records.get_mut(&sequence).ok_or(Error::ResultExpired)?;
        if value.is_some() { return Err(Error::SequenceConflict); }
        *value = Some(result);
        self.completed.push_back(sequence);
        while self.completed.len() > self.capacity {
            if let Some(old) = self.completed.pop_front() { self.records.remove(&old); }
        }
        Ok(())
    }
}

/// Read the length before allocating. Transport authentication and immutable
/// adapter registration happen before this parser is used.
pub(super) async fn read_request<R: tokio::io::AsyncRead + Unpin>(reader: &mut R)
    -> std::io::Result<Request> {
    use tokio::io::AsyncReadExt;
    let length = reader.read_u32().await? as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid broker frame length"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(|_| std::io::Error::new(
        std::io::ErrorKind::InvalidData, "invalid broker request"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_acquire_reply_replays_without_a_second_acquire() {
        let op = Operation::Acquire { session_id: "fixture".into(), forward_index: 0, ensure: true };
        let mut ledger = RequestLedger::new(1);
        assert_eq!(ledger.begin(1, &op), Ok(Replay::<String>::New));
        assert_eq!(ledger.begin(1, &op), Ok(Replay::Pending));
        ledger.complete(1, "same-lease".into()).unwrap();
        assert_eq!(ledger.begin(1, &op), Ok(Replay::Completed("same-lease".into())));
        assert_eq!(ledger.begin(1, &Operation::Snapshot {}), Err(Error::SequenceConflict));
        assert_eq!(ledger.begin(2, &Operation::Snapshot {}), Ok(Replay::New));
        ledger.complete(2, "snapshot".into()).unwrap();
        assert_eq!(ledger.begin(1, &op), Err(Error::ResultExpired));
    }

    fn raw_request(operation: &str) -> String {
        format!(r#"{{"sequence":1,"epoch":"e","operation":{operation}}}"#)
    }

    #[test]
    fn all_wire_methods_reject_unknown_and_duplicate_fields() {
        let operations = [
            r#"{"method":"acquire","session_id":"fixture","forward_index":0,"ensure":true}"#,
            r#"{"method":"snapshot"}"#,
            r#"{"method":"local_view"}"#,
            r#"{"method":"release","lease":"fixture"}"#,
        ];
        for operation in operations {
            assert!(serde_json::from_str::<Request>(&raw_request(operation)).is_ok());
            let prefix = &operation[..operation.len() - 1];
            for field in ["owner", "profile", "kind", "ttl", "principal", "deadline"] {
                let injected = format!(r#"{prefix},"{field}":"local"}}"#);
                assert!(serde_json::from_str::<Request>(&raw_request(&injected)).is_err(), "accepted {injected}");
                let duplicate = format!(r#"{prefix},"{field}":"a","{field}":"b"}}"#);
                assert!(serde_json::from_str::<Request>(&raw_request(&duplicate)).is_err());
            }
            let duplicate_method = format!(r#"{prefix},"method":"snapshot"}}"#);
            assert!(serde_json::from_str::<Request>(&raw_request(&duplicate_method)).is_err());
        }
        for repeated in [r#""session_id":"other""#, r#""forward_index":1"#, r#""ensure":false"#] {
            let prefix = &operations[0][..operations[0].len() - 1];
            let duplicate = format!("{prefix},{repeated}}}");
            assert!(serde_json::from_str::<Request>(&raw_request(&duplicate)).is_err());
        }
        assert!(serde_json::from_str::<Request>(&raw_request(r#"{"method":"release","lease":"a","lease":"b"}"#)).is_err());
        for repeated in [r#""sequence":2"#, r#""epoch":"other""#, r#""operation":{"method":"snapshot"}"#] {
            let original = raw_request(operations[1]);
            let duplicate = format!("{},{repeated}}}", &original[..original.len() - 1]);
            assert!(serde_json::from_str::<Request>(&duplicate).is_err());
        }
    }

    #[test]
    fn pending_payload_conflict_and_full_queue_do_not_advance_high_water() {
        let mut ledger = RequestLedger::<()>::new(1);
        for seq in 1..=MAX_IN_FLIGHT as u64 {
            assert_eq!(ledger.begin(seq, &Operation::Snapshot {}), Ok(Replay::New));
        }
        assert_eq!(ledger.begin(1, &Operation::LocalView {}), Err(Error::SequenceConflict));
        assert_eq!(ledger.begin(1000, &Operation::Snapshot {}), Err(Error::Busy));
        ledger.complete(1, ()).unwrap();
        assert_eq!(ledger.begin(33, &Operation::Snapshot {}), Ok(Replay::New));
        ledger.complete(2, ()).unwrap();
        assert_eq!(ledger.begin(1, &Operation::Snapshot {}), Err(Error::ResultExpired));
        assert_eq!(ledger.begin(3, &Operation::Snapshot {}), Ok(Replay::Pending));
    }
    #[tokio::test]
    async fn oversized_frame_is_rejected_before_reading_its_body() {
        use tokio::io::AsyncWriteExt;
        let (mut sender, mut receiver) = tokio::io::duplex(16);
        sender.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), read_request(&mut receiver)).await.unwrap();
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

}
