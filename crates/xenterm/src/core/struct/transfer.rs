//! One file transfer, as a framework-free record.

use crate::i18n::t;
use crate::ssh::format_size;

/// Lifecycle of a transfer reported by the SFTP and ZMODEM workers through
/// `SessionEvent::SftpTransfer`.
///
/// The wire form is a `u8` because it crosses the session-event channel from
/// worker threads. The codes are part of that contract and must keep matching
/// what `src/sftp/impls/sftp.rs` and `src/terminal/impls/zmodem.rs` send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferPhase {
    /// Bytes are flowing.
    Active,
    /// Finished successfully.
    Done,
    /// Finished with an error; `Transfer::message` carries the reason.
    Failed,
    /// Remote-side prep (e.g. tar packing) before any bytes flow (#100).
    Preparing,
    /// User cancelled (#100).
    Cancelled,
}

impl TransferPhase {
    /// Decode the `u8` carried by `SessionEvent::SftpTransfer`.
    ///
    /// Unknown codes decode to `Active` rather than erroring: a transfer we
    /// cannot classify should keep the toolbar's in-progress indicator lit and
    /// stay visible in the manager, not silently disappear from view.
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Done,
            2 => Self::Failed,
            3 => Self::Preparing,
            4 => Self::Cancelled,
            _ => Self::Active,
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Done => 1,
            Self::Failed => 2,
            Self::Preparing => 3,
            Self::Cancelled => 4,
        }
    }

    /// Whether the transfer manager's breathing indicator should stay lit:
    /// bytes are flowing, or will be once remote prep finishes.
    pub fn is_in_progress(self) -> bool {
        matches!(self, Self::Active | Self::Preparing)
    }
}

/// One transfer. The projected `TransferInfo` row is derived from this.
#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    pub id: String,
    pub name: String,
    pub is_upload: bool,
    pub transferred: u64,
    pub total: u64,
    pub phase: TransferPhase,
    /// Worker-supplied detail. Meaningful for `Failed`, where it carries the
    /// real error text instead of a generic label; empty otherwise.
    pub message: String,
}

impl Transfer {
    pub fn from_event(
        id: String,
        name: String,
        is_upload: bool,
        transferred: u64,
        total: u64,
        state: u8,
        message: String,
    ) -> Self {
        Self {
            id,
            name,
            is_upload,
            transferred,
            total,
            phase: TransferPhase::from_code(state),
            message,
        }
    }

    /// Completion as 0.0..=1.0.
    ///
    /// A finished transfer reports 1.0 even when the worker never sent a byte
    /// total, so the progress bar always reaches full on success.
    pub fn percent(&self) -> f32 {
        if self.phase == TransferPhase::Done {
            1.0
        } else if self.total > 0 {
            (self.transferred as f32 / self.total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The progress column of the transfer manager: a human-readable size pair,
    /// or the terminal-state label once the transfer is no longer running.
    ///
    /// Formatting lives here rather than in a UI layer because it is identical
    /// for every consumer — the window and any plugin reading transfer progress
    /// get the same string without reimplementing the rules.
    pub fn detail(&self) -> String {
        match self.phase {
            TransferPhase::Failed => {
                if self.message.is_empty() {
                    t("失败", "Failed").to_string()
                } else {
                    self.message.clone()
                }
            }
            TransferPhase::Done => t("已完成", "Done").to_string(),
            TransferPhase::Preparing => t("文件准备中", "Preparing...").to_string(),
            TransferPhase::Cancelled => t("已取消", "Cancelled").to_string(),
            TransferPhase::Active => {
                if self.total > 0 {
                    format!(
                        "{}/{}",
                        format_size(self.transferred),
                        format_size(self.total)
                    )
                } else {
                    format_size(self.transferred)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer(phase: TransferPhase, transferred: u64, total: u64) -> Transfer {
        Transfer {
            id: "t1".into(),
            name: "file.bin".into(),
            is_upload: false,
            transferred,
            total,
            phase,
            message: String::new(),
        }
    }

    #[test]
    fn phase_codes_round_trip() {
        for code in 0..=4u8 {
            assert_eq!(TransferPhase::from_code(code).code(), code);
        }
    }

    #[test]
    fn unknown_phase_code_falls_back_to_active() {
        // A worker sending a code we don't know must not vanish from the manager.
        assert_eq!(TransferPhase::from_code(9), TransferPhase::Active);
        assert!(TransferPhase::from_code(9).is_in_progress());
    }

    #[test]
    fn only_active_and_preparing_count_as_in_progress() {
        for (phase, want) in [
            (TransferPhase::Active, true),
            (TransferPhase::Preparing, true),
            (TransferPhase::Done, false),
            (TransferPhase::Failed, false),
            (TransferPhase::Cancelled, false),
        ] {
            assert_eq!(phase.is_in_progress(), want, "{phase:?}");
        }
    }

    #[test]
    fn percent_completes_on_done_even_without_a_byte_total() {
        assert_eq!(transfer(TransferPhase::Done, 0, 0).percent(), 1.0);
    }

    #[test]
    fn percent_clamps_when_the_worker_overshoots_the_total() {
        assert_eq!(transfer(TransferPhase::Active, 500, 100).percent(), 1.0);
    }

    #[test]
    fn percent_is_zero_while_total_is_unknown() {
        assert_eq!(transfer(TransferPhase::Active, 4096, 0).percent(), 0.0);
    }

    #[test]
    fn percent_tracks_progress() {
        assert_eq!(transfer(TransferPhase::Active, 25, 100).percent(), 0.25);
    }

    #[test]
    fn failed_detail_prefers_the_worker_message() {
        let mut tr = transfer(TransferPhase::Failed, 0, 0);
        tr.message = "permission denied".into();
        assert_eq!(tr.detail(), "permission denied");

        tr.message.clear();
        assert!(!tr.detail().is_empty(), "must fall back to a generic label");
    }

    #[test]
    fn active_detail_shows_both_sizes_only_when_total_is_known() {
        assert!(
            transfer(TransferPhase::Active, 10, 0)
                .detail()
                .chars()
                .filter(|c| *c == '/')
                .count()
                == 0
        );
        assert!(transfer(TransferPhase::Active, 10, 100)
            .detail()
            .contains('/'));
    }
}
