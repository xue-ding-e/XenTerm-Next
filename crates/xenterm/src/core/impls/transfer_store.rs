//! Ordered store of live and finished transfers.

use std::collections::HashMap;

use super::transfer::Transfer;

/// What one `TransferStore::upsert` changed, so a UI projection can apply a
/// minimal update instead of rebuilding its whole model.
///
/// Progress events arrive per transferred chunk, which is far too often to
/// reconstruct a list on every tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferChange {
    /// Index of the row in the store's insertion order.
    pub slot: usize,
    /// Row count after the change.
    pub len: usize,
    /// True when this created a row; false when it updated an existing one.
    pub inserted: bool,
}

impl TransferChange {
    /// Index of the row in a newest-first projection: the order the transfer
    /// manager displays, and the order its rows are kept in.
    ///
    /// The store itself appends, so a newly inserted row is last in insertion
    /// order and first on screen.
    ///
    /// Point-in-time: the index is computed from the row count as it stood when
    /// this change was produced, so apply it before making any further change.
    /// A `TransferChange` held across later upserts reports a stale index.
    pub fn display_index(&self) -> usize {
        self.len - 1 - self.slot
    }
}

/// Transfer bookkeeping, owned by Rust.
///
/// This replaces the `VecModel<TransferInfo>` that used to be the *only* record
/// of transfer progress in the app. That made the UI toolkit the source of
/// truth: matching a progress event to its row meant a linear scan of the model
/// on every chunk, and answering "is anything still running?" meant rescanning
/// every row again on the same tick. Rows are now kept in insertion order with
/// an id→slot index (O(1) upsert — the index is stable because rows are only
/// ever appended) and the in-progress count is maintained incrementally.
#[derive(Default, Debug)]
pub struct TransferStore {
    rows: Vec<Transfer>,
    slots: HashMap<String, usize>,
    in_progress: usize,
}

impl TransferStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record progress for `transfer.id`, creating the row on first sight.
    pub fn upsert(&mut self, transfer: Transfer) -> TransferChange {
        match self.slots.get(&transfer.id) {
            Some(&slot) => {
                let row = &mut self.rows[slot];
                // Maintain the in-progress count across the phase transition
                // instead of rescanning every row afterwards.
                match (row.phase.is_in_progress(), transfer.phase.is_in_progress()) {
                    (true, false) => self.in_progress -= 1,
                    (false, true) => self.in_progress += 1,
                    _ => {}
                }
                *row = transfer;
                TransferChange {
                    slot,
                    len: self.rows.len(),
                    inserted: false,
                }
            }
            None => {
                if transfer.phase.is_in_progress() {
                    self.in_progress += 1;
                }
                let slot = self.rows.len();
                self.slots.insert(transfer.id.clone(), slot);
                self.rows.push(transfer);
                TransferChange {
                    slot,
                    len: self.rows.len(),
                    inserted: true,
                }
            }
        }
    }

    /// Whether any transfer is still running — drives the breathing indicator
    /// on the Transfers toolbar button (#breathing-light).
    pub fn has_in_progress(&self) -> bool {
        self.in_progress > 0
    }

    pub fn get(&self, id: &str) -> Option<&Transfer> {
        self.slots.get(id).map(|&slot| &self.rows[slot])
    }

    /// Every row, oldest first.
    ///
    /// Insertion order, which is also the order `TransferChange::slot` indexes into. A
    /// manager displays it reversed — newest at the top — but that is the projection's
    /// decision, not the store's: a caller that wanted oldest-first would otherwise have
    /// to reverse it back.
    pub fn rows(&self) -> &[Transfer] {
        &self.rows
    }

    /// Drop every row (`on_clear_transfers`).
    ///
    /// The whole index is rebuilt rather than spliced, so clearing also resets
    /// the in-progress count — a stale counter here would leave the toolbar
    /// indicator breathing forever.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.slots.clear();
        self.in_progress = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::transfer::TransferPhase;
    use crate::core::Transfer;

    fn transfer(id: &str, phase: TransferPhase) -> Transfer {
        Transfer {
            id: id.into(),
            name: format!("{id}.bin"),
            is_upload: false,
            transferred: 0,
            total: 100,
            phase,
            message: String::new(),
        }
    }

    #[test]
    fn first_sight_inserts_and_later_events_update_in_place() {
        let mut store = TransferStore::new();

        let first = store.upsert(transfer("a", TransferPhase::Active));
        assert!(first.inserted);
        assert_eq!(first.len, 1);

        let again = store.upsert(transfer("a", TransferPhase::Active));
        assert!(!again.inserted);
        assert_eq!(again.len, 1, "a repeat progress event must not duplicate");
    }

    #[test]
    fn rows_are_read_in_insertion_order_whatever_the_display_order() {
        let mut store = TransferStore::new();
        store.upsert(transfer("a", TransferPhase::Active));
        store.upsert(transfer("b", TransferPhase::Active));
        store.upsert(transfer("a", TransferPhase::Done));

        let ids: Vec<&str> = store.rows().iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "an update keeps a row in its own slot");
        assert_eq!(
            store.rows()[0].phase,
            TransferPhase::Done,
            "and the update is visible through the read"
        );
    }

    #[test]
    fn display_index_is_the_slot_to_apply_this_change_at() {
        let mut store = TransferStore::new();

        // A lone row is also the newest, so it belongs at the top.
        let first = store.upsert(transfer("a", TransferPhase::Active));
        assert_eq!(first.display_index(), 0);

        // The newer row takes the top slot, pushing "a" down.
        let second = store.upsert(transfer("b", TransferPhase::Active));
        assert_eq!(second.display_index(), 0);

        // Updating the older row now lands below "b".
        let again = store.upsert(transfer("a", TransferPhase::Active));
        assert_eq!(again.display_index(), 1);

        // And updating the newer one stays at the top.
        let b_again = store.upsert(transfer("b", TransferPhase::Active));
        assert_eq!(b_again.display_index(), 0);
    }

    #[test]
    fn display_indices_are_a_distinct_newest_first_permutation() {
        let mut store = TransferStore::new();
        for id in ["a", "b", "c"] {
            store.upsert(transfer(id, TransferPhase::Active));
        }

        // Touch every row in turn. Two rows reporting the same slot would make
        // the projection overwrite one of them, so distinctness matters as much
        // as the ordering.
        let mut slots = Vec::new();
        for id in ["a", "b", "c"] {
            let change = store.upsert(transfer(id, TransferPhase::Active));
            assert!(!change.inserted);
            slots.push((id, change.display_index()));
        }

        slots.sort_by_key(|(_, slot)| *slot);
        assert_eq!(slots, [("c", 0), ("b", 1), ("a", 2)]);
    }

    #[test]
    fn in_progress_flag_tracks_the_living_transfer() {
        let mut store = TransferStore::new();
        assert!(!store.has_in_progress());

        store.upsert(transfer("a", TransferPhase::Active));
        assert!(store.has_in_progress());

        // Still running after more progress on the same row.
        store.upsert(transfer("a", TransferPhase::Active));
        assert!(store.has_in_progress());

        store.upsert(transfer("a", TransferPhase::Done));
        assert!(!store.has_in_progress());
    }

    #[test]
    fn every_terminal_phase_clears_the_in_progress_flag() {
        for done in [
            TransferPhase::Done,
            TransferPhase::Failed,
            TransferPhase::Cancelled,
        ] {
            let mut store = TransferStore::new();
            store.upsert(transfer("a", TransferPhase::Preparing));
            assert!(store.has_in_progress());
            store.upsert(transfer("a", done));
            assert!(!store.has_in_progress(), "{done:?} must stop the indicator");
        }
    }

    #[test]
    fn in_progress_count_survives_mixed_phases() {
        let mut store = TransferStore::new();
        store.upsert(transfer("a", TransferPhase::Active));
        store.upsert(transfer("b", TransferPhase::Preparing));
        store.upsert(transfer("c", TransferPhase::Active));
        assert!(store.has_in_progress());

        store.upsert(transfer("a", TransferPhase::Done));
        store.upsert(transfer("c", TransferPhase::Failed));
        assert!(
            store.has_in_progress(),
            "b is still preparing, so the indicator must stay lit"
        );

        store.upsert(transfer("b", TransferPhase::Done));
        assert!(!store.has_in_progress());
    }

    #[test]
    fn a_row_first_seen_finished_does_not_light_the_indicator() {
        let mut store = TransferStore::new();
        store.upsert(transfer("a", TransferPhase::Done));
        assert!(!store.has_in_progress());
    }

    #[test]
    fn clear_resets_rows_and_the_in_progress_count() {
        let mut store = TransferStore::new();
        store.upsert(transfer("a", TransferPhase::Active));
        store.upsert(transfer("b", TransferPhase::Active));
        store.clear();

        assert!(store.get("a").is_none());
        assert!(store.get("b").is_none());
        assert!(
            !store.has_in_progress(),
            "a stale count would breathe forever"
        );
    }

    #[test]
    fn ids_are_reusable_after_a_clear() {
        let mut store = TransferStore::new();
        store.upsert(transfer("a", TransferPhase::Active));
        store.clear();

        // The index must not still point at the pre-clear slot.
        let change = store.upsert(transfer("a", TransferPhase::Done));
        assert!(change.inserted);
        assert_eq!(change.len, 1);
        assert!(!store.has_in_progress());
    }

    #[test]
    fn get_returns_the_latest_progress() {
        let mut store = TransferStore::new();
        let mut t = transfer("a", TransferPhase::Active);
        t.transferred = 10;
        store.upsert(t);

        let mut t = transfer("a", TransferPhase::Active);
        t.transferred = 80;
        store.upsert(t);

        assert_eq!(store.get("a").map(|t| t.transferred), Some(80));
        assert!(store.get("missing").is_none());
    }
}
