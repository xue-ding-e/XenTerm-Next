use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::session::protocol::{ProcInfo, SystemDetails};

use sysinfo::{Disks, Networks, System};

/// Snapshot passed to the UI each tick.
#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    pub cpu_percent: f32,
    pub mem_percent: f32,
    pub swap_percent: f32,
    pub mem_used_mib: u64,
    pub mem_total_mib: u64,
    pub swap_used_mib: u64,
    pub swap_total_mib: u64,
    pub net_bytes_per_sec: u64,
    pub net_rx_per_sec: u64,
    pub net_tx_per_sec: u64,
    /// Per-filesystem (mount, available_bytes, total_bytes).
    pub disks: Vec<(String, u64, u64)>,
}

/// Stateful sampler. Construct once per process and poll via [`Self::sample`].
pub struct SystemSampler {
    pub(super) sys: System,
    pub(super) nets: Networks,
    pub(super) disks: Disks,
    pub(super) last_rx_total: u64,
    pub(super) last_tx_total: u64,
    pub(super) last_instant: std::time::Instant,
}

#[derive(Clone, Default)]
pub(crate) struct LocalHardwareInfo {
    pub(crate) os: String,
    pub(crate) kernel: String,
    pub(crate) kernel_version: String,
    pub(crate) arch: String,
    pub(crate) hostname: String,
    pub(crate) cpu_name: String,
    pub(crate) cpu_vendor: String,
    pub(crate) cpu_cores: String,
    pub(crate) cpu_frequency: String,
    pub(crate) gpus: Vec<LocalGpuInfo>,
}

#[derive(Clone, Default)]
pub(crate) struct LocalGpuInfo {
    pub(crate) name: String,
    pub(crate) vendor: String,
    pub(crate) driver: String,
    pub(crate) memory: String,
}

/// One filesystem row for the resource panel: where it is mounted, how full it is,
/// and the free/total figure already formatted for a narrow column.
///
/// Framework-neutral on purpose. The rows are derived here rather than inside a
/// panel's own projection, so every caller gets the same answer and no second
/// caller can disagree about how full a disk is; only the projection into a
/// toolkit's model stays on the toolkit's side.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskUsage {
    pub path: String,
    /// "available/total", e.g. `12.3G/256G`.
    pub detail: String,
    /// Used fraction, 0.0..=1.0.
    pub percent: f32,
}

/// One process row, as the process window's table draws it.
///
/// Framework-neutral for the same reason [`DiskUsage`] is: the numbers and the
/// privilege question are the same whichever toolkit renders the table, and a second
/// derivation would be a second answer to "may this user signal this process".
#[derive(Debug, Clone, PartialEq)]
pub struct ProcRow {
    /// The tab the sample came from. Carried on the row because terminating it needs
    /// the session that reported it, and the user may have switched tabs since.
    pub tab_id: String,
    pub pid: String,
    pub user: String,
    /// CPU percent, one decimal.
    pub cpu: String,
    /// Memory percent, one decimal.
    pub mem: String,
    pub command: String,
    /// 0.0..=1.0, drives the row's load bar.
    pub cpu_frac: f32,
    /// May be terminated without administrator authentication.
    pub own_process: bool,
}

/// One row of a system-information table: five cells, most of which a given table
/// leaves empty.
///
/// Framework-neutral for the same reason [`DiskUsage`] and [`ProcRow`] are: which pair
/// goes in which cell, and what an absent value reads as, is a decision about the data
/// and not about the toolkit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InfoRow {
    pub c1: String,
    pub c2: String,
    pub c3: String,
    pub c4: String,
    pub c5: String,
}

impl InfoRow {
    /// The cells in order, for a table that draws them by index.
    pub fn cells(&self) -> [&str; 5] {
        [&self.c1, &self.c2, &self.c3, &self.c4, &self.c5]
    }

    /// Whether every cell is empty, which is what a chunk of missing data looks like.
    pub fn is_blank(&self) -> bool {
        self.cells()
            .iter()
            .all(|cell| cell.trim().is_empty() || cell.trim() == "-")
    }
}

#[derive(Clone, Default)]
pub(crate) struct TabStatus {
    pub(crate) host: String,
    pub(crate) user: String,
    pub(crate) session_id: String,
    pub(crate) state: u8,
    /// True for built-in local shell tabs (system:*). Those should show the
    /// local machine's resource panel, not the (empty) remote stats fields.
    pub(crate) is_local: bool,
    pub(crate) cpu: f32,
    pub(crate) mem_used_kib: u64,
    pub(crate) mem_total_kib: u64,
    pub(crate) swap_used_kib: u64,
    pub(crate) swap_total_kib: u64,
    pub(crate) net: Vec<(String, u64, u64)>,
    pub(crate) selected_iface: String,
    pub(crate) net_hist: Vec<f32>,
    pub(crate) disks: Vec<(String, u64, u64)>,
    pub(crate) procs: Vec<ProcInfo>,
    pub(crate) sys: SystemDetails,
}

pub(crate) type TabStatuses = Arc<Mutex<HashMap<String, TabStatus>>>;
pub(crate) type LocalSnap = Arc<Mutex<SystemSnapshot>>;
pub(crate) type NetHist = Arc<Mutex<Vec<f32>>>;
