//! Lightweight poller for local machine stats (CPU / memory / network).
//!
//! `sysinfo` is already a dependency for many Rust desktop apps; it gives us
//! cross-platform data with ~2% CPU overhead at 1-second cadence.

use std::time::Duration;

use sysinfo::{Disks, Networks, System};

use super::system_types::{DiskUsage, InfoRow, ProcRow, SystemSampler, SystemSnapshot, TabStatus};
use crate::session::protocol::ProcInfo;

impl SystemSampler {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        let nets = Networks::new_with_refreshed_list();
        let last_rx_total = nets.iter().map(|(_, d)| d.total_received()).sum();
        let last_tx_total = nets.iter().map(|(_, d)| d.total_transmitted()).sum();
        let disks = Disks::new_with_refreshed_list();
        Self {
            sys,
            nets,
            disks,
            last_rx_total,
            last_tx_total,
            last_instant: std::time::Instant::now(),
        }
    }

    /// Recommended poll interval for a UI sidebar.
    pub fn recommended_interval() -> Duration {
        Duration::from_millis(1000)
    }

    pub fn sample(&mut self) -> SystemSnapshot {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.nets.refresh(true);

        let cpu_percent = self.sys.global_cpu_usage() / 100.0;

        let mem_total = self.sys.total_memory();
        let mem_used = self.sys.used_memory();
        let mem_percent = if mem_total > 0 {
            mem_used as f32 / mem_total as f32
        } else {
            0.0
        };

        let swap_total = self.sys.total_swap();
        let swap_used = self.sys.used_swap();
        let swap_percent = if swap_total > 0 {
            swap_used as f32 / swap_total as f32
        } else {
            0.0
        };

        // RX / TX bytes/sec from the delta across the iface list.
        let rx_total: u64 = self.nets.iter().map(|(_, d)| d.total_received()).sum();
        let tx_total: u64 = self.nets.iter().map(|(_, d)| d.total_transmitted()).sum();
        let now = std::time::Instant::now();
        let elapsed = now
            .duration_since(self.last_instant)
            .as_secs_f64()
            .max(0.001);
        let rx_delta = rx_total.saturating_sub(self.last_rx_total);
        let tx_delta = tx_total.saturating_sub(self.last_tx_total);
        self.last_rx_total = rx_total;
        self.last_tx_total = tx_total;
        self.last_instant = now;
        let net_rx_per_sec = (rx_delta as f64 / elapsed) as u64;
        let net_tx_per_sec = (tx_delta as f64 / elapsed) as u64;

        // Local filesystems (slow-changing, but cheap to refresh).
        self.disks.refresh(true);
        let disks: Vec<(String, u64, u64)> = self
            .disks
            .iter()
            .map(|d| {
                (
                    d.mount_point().to_string_lossy().to_string(),
                    d.available_space(),
                    d.total_space(),
                )
            })
            .filter(|(_, _, total)| *total > 0)
            .collect();

        SystemSnapshot {
            cpu_percent,
            mem_percent,
            swap_percent,
            mem_used_mib: mem_used / 1024 / 1024,
            mem_total_mib: mem_total / 1024 / 1024,
            swap_used_mib: swap_used / 1024 / 1024,
            swap_total_mib: swap_total / 1024 / 1024,
            net_bytes_per_sec: net_rx_per_sec + net_tx_per_sec,
            net_rx_per_sec,
            net_tx_per_sec,
            disks,
        }
    }
}

/// Format a used/total memory pair (both in MiB) for the narrow sidebar.
/// Below 1 GiB it stays in megabytes (`512/2048M`); at or above, it switches to
/// gigabytes and drops the decimal for whole or large values to stay compact
/// (`1.5G/16G`, `120G/256G`).
pub fn format_mem(used_mib: u64, total_mib: u64) -> String {
    if total_mib < 1024 {
        return format!("{used_mib}/{total_mib}M");
    }
    // MiB → GiB, with a tidy width: integer when round or ≥100, else one decimal.
    fn gib(mib: u64) -> String {
        let g = mib as f64 / 1024.0;
        if g.fract() == 0.0 || g >= 100.0 {
            (g as u64).to_string()
        } else {
            format!("{g:.1}")
        }
    }
    format!("{}G/{}G", gib(used_mib), gib(total_mib))
}

/// Human-readable network throughput (e.g. `"1.2 MB/s"`).
pub fn format_bytes_per_sec(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bytes as f64;
    let mut idx = 0;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", bytes, UNITS[idx])
    } else {
        format!("{:.1} {}", value, UNITS[idx])
    }
}

/// Number of samples a throughput sparkline keeps.
///
/// One minute at the sampler's cadence, which is the window FinalShell's own graph
/// shows. Shared rather than written out twice: the local ring and a session's ring
/// have to be the same length, or the two graphs in one panel would cover different
/// spans of time and read as different scales.
pub const NET_HISTORY_LEN: usize = 60;

/// Append one sample to a throughput ring, oldest first.
///
/// A wrong-length buffer is reset rather than patched up: it means the buffer was built
/// from something other than this constant (a stale config, a shorter history read back
/// from an older build), and a ring that silently kept its odd length would make the
/// sparkline's time axis wrong for the rest of the session.
pub fn push_ring(buf: &mut Vec<f32>, val: f32) {
    if buf.len() != NET_HISTORY_LEN {
        *buf = vec![0.0; NET_HISTORY_LEN];
    }
    buf.remove(0);
    buf.push(val);
}

/// Auto-scale a raw bytes/sec history to 0..1 against its own window peak.
///
/// Its own peak, rather than a fixed ceiling, is what keeps the graph readable on both
/// a 10 KB/s link and a 10 GB/s one: an absolute scale would draw one of them as a flat
/// line. A floor of 1.0 keeps an all-zero window at zero instead of dividing by nothing.
pub fn normalized_history(buf: &[f32]) -> Vec<f32> {
    let max = buf.iter().copied().fold(1.0_f32, f32::max);
    buf.iter().map(|v| (v / max).clamp(0.0, 1.0)).collect()
}

/// The filesystem rows for a `(mount, available, total)` list.
///
/// Rows whose total is zero are kept: a pseudo-filesystem reporting no size is still a
/// mount the user can see in `df`, and dropping it here would make the two panels
/// disagree about what exists. What is dropped is the *fraction's* reliance on a
/// division that cannot be done.
pub fn disk_usage(disks: &[(String, u64, u64)]) -> Vec<DiskUsage> {
    disks
        .iter()
        .map(|(mount, avail, total)| {
            let used = total.saturating_sub(*avail);
            let percent = if *total > 0 {
                used as f32 / *total as f32
            } else {
                0.0
            };
            DiskUsage {
                path: mount.clone(),
                detail: format!(
                    "{}/{}",
                    crate::ssh::format_size(*avail),
                    crate::ssh::format_size(*total)
                ),
                percent,
            }
        })
        .collect()
}

/// Resolve which interface drives the top network graph: the user's selection if it
/// still exists, otherwise the busiest (the list is sorted busiest-first).
///
/// Returns `(name, rx_bytes_per_sec, tx_bytes_per_sec)`; an empty list yields an empty
/// name and zero rates, which is what a session that has not reported its NICs yet
/// should draw.
pub fn selected_iface(status: &TabStatus) -> (String, u64, u64) {
    if !status.selected_iface.is_empty() {
        if let Some(entry) = status.net.iter().find(|e| e.0 == status.selected_iface) {
            return entry.clone();
        }
    }
    status.net.first().cloned().unwrap_or_default()
}

/// The copyable host from a `user@host` connection label (#192): the part after the
/// last `@`, trimmed. Falls back to the whole string when there is no `@` (already a
/// bare host or IP).
///
/// Shared with the resource panel, which offers the same click-to-copy. Two strips of
/// the same label would be two different addresses to copy, which is the kind of
/// difference nobody notices until they paste the wrong host into a terminal.
pub fn connection_host(label: &str) -> String {
    label.rsplit('@').next().unwrap_or(label).trim().to_string()
}

/// Whether signalling `process_user`'s process needs administrator authentication.
///
/// A root login can signal anything. Anyone else may signal their own processes
/// directly, and needs `su` for root's or another user's — which is the rule the process
/// window's confirmation enforces before it offers the password field at all.
pub fn process_needs_root(current_user: &str, process_user: &str) -> bool {
    current_user != "root" && process_user != current_user
}

/// The process table's rows for one session's latest sample.
///
/// `cpu_frac` is the same measurement as `cpu` in another shape: the text is what the
/// column reads and the fraction is what the row's load bar draws. Both are derived
/// together so a row cannot show one percentage and draw another.
pub fn proc_rows(procs: &[ProcInfo], current_user: &str, tab_id: &str) -> Vec<ProcRow> {
    procs
        .iter()
        .map(|p| ProcRow {
            tab_id: tab_id.to_string(),
            pid: p.pid.to_string(),
            user: p.user.clone(),
            cpu: format!("{:.1}", p.cpu),
            mem: format!("{:.1}", p.mem),
            command: p.command.clone(),
            cpu_frac: (p.cpu / 100.0).clamp(0.0, 1.0),
            own_process: !process_needs_root(current_user, &p.user),
        })
        .collect()
}

/// The overview card: two label/value pairs per row.
pub fn overview_rows(pairs: &[(String, String)]) -> Vec<InfoRow> {
    pairs
        .chunks(2)
        .map(|chunk| {
            let first = &chunk[0];
            let second = chunk.get(1);
            InfoRow {
                c1: first.0.clone(),
                c2: first.1.clone(),
                c3: second.map(|p| p.0.clone()).unwrap_or_default(),
                c4: second.map(|p| p.1.clone()).unwrap_or_default(),
                c5: String::new(),
            }
        })
        .collect()
}

/// One table row holding the first five values of a key/value list.
///
/// A missing value reads as `-` rather than as nothing: this table is read by comparing
/// one machine's row against another's, and a blank cell is indistinguishable from a
/// probe that failed to fill it.
pub fn single_row(pairs: &[(String, String)]) -> Vec<InfoRow> {
    vec![row_from_values(pairs, 5)]
}

/// Rows of `width` values each, with all-blank chunks dropped.
///
/// Dropping them is what keeps a GPU card from growing empty rows on a machine with one
/// GPU: the probe pads its list out, and the padding is not data.
pub fn chunked_rows(pairs: &[(String, String)], width: usize) -> Vec<InfoRow> {
    let width = width.max(1);
    pairs
        .chunks(width)
        .map(|chunk| row_from_values(chunk, width))
        .filter(|row| !row.is_blank())
        .collect()
}

/// The CPU-usage card: user, system, nice, idle, then everything else as one line.
///
/// The first four come out in the order the *table* prints them (User, System, Nice,
/// Idle) rather than the order the probe collects them, which is why the indices look
/// shuffled. That is the table's mapping, kept as it is: its headers and this function
/// are one decision written in two places, and changing one alone would put each
/// number under the wrong heading.
pub fn cpu_usage_rows(pairs: &[(String, String)]) -> Vec<InfoRow> {
    let value = |idx: usize| {
        pairs
            .get(idx)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "0.0%".to_string())
    };
    let extra = pairs
        .iter()
        .skip(4)
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(" / ");
    vec![InfoRow {
        c1: value(0),
        c2: value(2),
        c3: value(1),
        c4: value(3),
        c5: extra,
    }]
}

/// Rows that already have five columns of their own.
pub fn tuple5_rows(rows: &[(String, String, String, String, String)]) -> Vec<InfoRow> {
    rows.iter()
        .map(|r| InfoRow {
            c1: r.0.clone(),
            c2: r.1.clone(),
            c3: r.2.clone(),
            c4: r.3.clone(),
            c5: r.4.clone(),
        })
        .collect()
}

/// `width` values from a slice of pairs, `-` where the slice ran out and nothing beyond
/// the card's own column count.
fn row_from_values(pairs: &[(String, String)], width: usize) -> InfoRow {
    let value = |idx: usize| {
        if idx >= width {
            return String::new();
        }
        pairs
            .get(idx)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "-".to_string())
    };
    InfoRow {
        c1: value(0),
        c2: value(1),
        c3: value(2),
        c4: value(3),
        c5: value(4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_newest_sample_last_and_its_length_fixed() {
        let mut buf = vec![0.0; NET_HISTORY_LEN];
        push_ring(&mut buf, 7.0);
        assert_eq!(buf.len(), NET_HISTORY_LEN, "a ring never grows");
        assert_eq!(buf[NET_HISTORY_LEN - 1], 7.0, "the newest sample is last");
        assert_eq!(buf[0], 0.0, "and the oldest fell off the front");
    }

    #[test]
    fn a_wrong_length_buffer_is_rebuilt_rather_than_kept() {
        // What an older build's shorter history would look like if it were loaded back.
        let mut buf = vec![1.0; 10];
        push_ring(&mut buf, 2.0);
        assert_eq!(buf.len(), NET_HISTORY_LEN);
        assert_eq!(buf[NET_HISTORY_LEN - 1], 2.0);
        assert_eq!(buf[0], 0.0, "nothing from the old buffer survives");
    }

    #[test]
    fn history_scales_against_its_own_peak() {
        let scaled = normalized_history(&[0.0, 50.0, 100.0]);
        assert_eq!(scaled, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn an_idle_history_stays_flat_instead_of_dividing_by_zero() {
        let scaled = normalized_history(&[0.0; 4]);
        assert_eq!(scaled, vec![0.0; 4]);
        assert!(normalized_history(&[]).is_empty());
    }

    #[test]
    fn a_disk_row_reports_used_against_total() {
        let rows = disk_usage(&[("/".to_string(), 250, 1000)]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/");
        assert!((rows[0].percent - 0.75).abs() < f32::EPSILON);
        assert_eq!(rows[0].detail, "250 B/1000 B");
    }

    #[test]
    fn a_size_less_mount_is_shown_without_a_fraction() {
        let rows = disk_usage(&[("proc".to_string(), 0, 0)]);
        assert_eq!(rows.len(), 1, "the mount is still listed");
        assert_eq!(rows[0].percent, 0.0, "and does not claim to be full");
    }

    #[test]
    fn the_selected_interface_wins_over_the_busiest() {
        let mut status = TabStatus {
            net: vec![
                ("eth0".to_string(), 900, 900),
                ("wlan0".to_string(), 10, 20),
            ],
            selected_iface: "wlan0".to_string(),
            ..Default::default()
        };
        assert_eq!(selected_iface(&status).0, "wlan0");

        // A selection that is no longer reported falls back to the busiest, which is
        // what a NIC that went down between samples looks like.
        status.selected_iface = "gone0".to_string();
        assert_eq!(selected_iface(&status).0, "eth0");

        status.net.clear();
        assert_eq!(selected_iface(&status).0, "");
    }

    #[test]
    fn a_process_row_carries_its_tab_and_its_owner() {
        let procs = vec![
            ProcInfo {
                pid: 42,
                user: "alice".to_string(),
                cpu: 12.34,
                mem: 5.6,
                command: "nginx: worker".to_string(),
            },
            ProcInfo {
                pid: 43,
                user: "root".to_string(),
                cpu: 0.0,
                mem: 0.0,
                command: "sshd".to_string(),
            },
        ];
        let rows = proc_rows(&procs, "alice", "term-a");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, "42");
        assert_eq!(rows[0].cpu, "12.3", "one decimal, as the column reads it");
        assert_eq!(rows[0].mem, "5.6");
        assert!(rows[0].own_process, "alice may signal alice's process");
        assert!(!rows[1].own_process, "but not root's without a password");
        assert!(rows.iter().all(|row| row.tab_id == "term-a"));
        assert!((rows[0].cpu_frac - 0.1234).abs() < 0.0001);
        assert_eq!(rows[1].cpu_frac, 0.0);
    }

    #[test]
    fn a_wild_cpu_percentage_does_not_overfill_the_row_bar() {
        // `ps` on a busy box can report above 100% for a multi-threaded process, and a
        // bar wider than its row would paint over the columns beside it.
        let procs = vec![ProcInfo {
            pid: 1,
            user: "root".to_string(),
            cpu: 480.0,
            mem: 0.0,
            command: "kworker".to_string(),
        }];
        assert_eq!(proc_rows(&procs, "root", "t")[0].cpu_frac, 1.0);
    }

    #[test]
    fn an_empty_sample_is_an_empty_table() {
        assert!(proc_rows(&[], "alice", "term-a").is_empty());
    }

    #[test]
    fn privilege_rules_match_effective_login_user() {
        assert!(!process_needs_root("alice", "alice"));
        assert!(process_needs_root("alice", "root"));
        assert!(process_needs_root("alice", "bob"));
        assert!(!process_needs_root("root", "root"));
        assert!(
            !process_needs_root("root", "alice"),
            "root signals anything"
        );
    }

    #[test]
    fn the_overview_pairs_two_labels_per_row() {
        let pairs = vec![
            ("OS".to_string(), "Linux".to_string()),
            ("Host".to_string(), "web-1".to_string()),
            ("Kernel".to_string(), "6.8".to_string()),
        ];
        let rows = overview_rows(&pairs);
        assert_eq!(rows.len(), 2, "three pairs is one full row and one half");
        assert_eq!(rows[0].cells(), ["OS", "Linux", "Host", "web-1", ""]);
        assert_eq!(
            rows[1].cells(),
            ["Kernel", "6.8", "", "", ""],
            "an odd pair leaves the second half empty rather than inventing one"
        );
    }

    #[test]
    fn a_single_row_reads_the_first_five_values_and_dashes_the_rest() {
        let pairs = vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), "3".to_string()),
        ];
        let rows = single_row(&pairs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cells(), ["1", "2", "3", "-", "-"]);
    }

    #[test]
    fn blank_chunks_are_dropped_from_a_chunked_card() {
        // A GPU probe with one card pads the rest of its row out; the padding must not
        // become a row of dashes.
        let pairs = vec![
            ("Name".to_string(), "RTX 3060".to_string()),
            ("Vendor".to_string(), "NVIDIA".to_string()),
            ("Driver".to_string(), "560.1".to_string()),
            ("Memory".to_string(), "6G".to_string()),
            ("".to_string(), "".to_string()),
            ("".to_string(), "-".to_string()),
        ];
        let rows = chunked_rows(&pairs, 4);
        assert_eq!(rows.len(), 1, "the padded chunk is not a row");
        assert_eq!(rows[0].cells(), ["RTX 3060", "NVIDIA", "560.1", "6G", ""]);
    }

    #[test]
    fn the_cpu_usage_row_puts_system_before_nice() {
        // The probe collects User, Nice, System, Idle; the table prints User, System,
        // Nice, Idle. Swapping these is invisible in the code and obvious on screen.
        let pairs = vec![
            ("User".to_string(), "11.0%".to_string()),
            ("Nice".to_string(), "22.0%".to_string()),
            ("System".to_string(), "33.0%".to_string()),
            ("Idle".to_string(), "44.0%".to_string()),
            ("IO".to_string(), "55.0%".to_string()),
            ("IRQ".to_string(), "66.0%".to_string()),
        ];
        let rows = cpu_usage_rows(&pairs);
        assert_eq!(rows[0].c1, "11.0%", "user");
        assert_eq!(rows[0].c2, "33.0%", "system");
        assert_eq!(rows[0].c3, "22.0%", "nice");
        assert_eq!(rows[0].c4, "44.0%", "idle");
        assert_eq!(rows[0].c5, "IO 55.0% / IRQ 66.0%");
    }

    #[test]
    fn five_column_rows_pass_straight_through() {
        let rows = tuple5_rows(&[(
            "eth0".to_string(),
            "1 KB".to_string(),
            "2 KB".to_string(),
            "3 KB/s".to_string(),
            "4 KB/s".to_string(),
        )]);
        assert_eq!(
            rows[0].cells(),
            ["eth0", "1 KB", "2 KB", "3 KB/s", "4 KB/s"]
        );
    }
}
