//! Conservative capacity for mounted local filesystems, not physical hardware.
//! Inputs are synthetic-testable df observations and a restricted mountinfo projection.
use std::collections::BTreeMap;

/// mountinfo escapes whitespace and backslashes as octal sequences. Reject other
/// escapes rather than accidentally matching two different mount paths.
fn mount_path(value: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let escaped = rest.get(at..at + 4)?;
        out.push(match escaped {
            "\\040" => ' ',
            "\\011" => '\t',
            "\\012" => '\n',
            "\\134" => '\\',
            _ => return None,
        });
        rest = &rest[at + 4..];
    }
    out.push_str(rest);
    Some(out)
}

/// Some(true): supported local identity; Some(false): explicitly excluded view.
/// None: an unknown or shared storage model, including unmeasured mount entries.
fn filesystem_is_local(kind: &str) -> Option<bool> {
    match kind {
        "ramfs" | "rootfs" | "efivarfs" | "nsfs" | "bpf" | "selinuxfs" | "tmpfs" | "devtmpfs"
        | "proc" | "sysfs" | "devpts" | "cgroup" | "cgroup2" | "securityfs" | "pstore"
        | "debugfs" | "tracefs" | "configfs" | "mqueue" | "hugetlbfs" | "fusectl" | "autofs"
        | "binfmt_misc" | "rpc_pipefs" | "overlay" | "aufs" | "squashfs" | "iso9660" | "nfs"
        | "nfs4" | "cifs" | "smb3" | "9p" | "ceph" | "fuse.sshfs" => Some(false),
        "ext2" | "ext3" | "ext4" | "xfs" | "vfat" | "exfat" | "ntfs" | "ntfs3" => Some(true),
        // btrfs subvolumes, ZFS datasets, FUSE and unknown filesystems may
        // share a pool. Device numbers alone do not establish independence.
        _ => None,
    }
}

/// `metadata` lines contain only major:minor, mount root, mount target and type.
/// No source, mount options, credentials, or physical-device inventory is needed.
/// Unknown/pool-backed types make the result unknown, never a misleading partial sum.
pub(super) fn mounted_local_capacity(
    disks: &[(String, u64, u64)],
    metadata: &[String],
    complete: bool,
) -> Option<(u64, u64)> {
    if !complete
        || metadata.is_empty()
        || metadata.len() > 512
        || disks.is_empty()
        || disks.len() > 64
    {
        return None;
    }
    let mut mounts = BTreeMap::new();
    let mut identities = BTreeMap::new();
    for line in metadata {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 4 {
            return None;
        }
        let (major, minor) = fields[0].split_once(':')?;
        let device = (major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?);
        let root = mount_path(fields[1])?;
        let target = mount_path(fields[2])?;
        if !root.starts_with('/') || !target.starts_with('/') {
            return None;
        }
        let local = filesystem_is_local(fields[3])?;
        // Linux loop devices (major 7) may be files within another counted
        // filesystem; their backing storage is intentionally not inspected.
        if local && (device.0 == 0 || device.0 == 7) {
            return None;
        }
        if local {
            if let Some(previous) = identities.insert(device, fields[3]) {
                if previous != fields[3] {
                    return None;
                }
            }
        }
        // Stacked mounts cannot be unambiguously joined to df by path alone.
        if mounts.insert(target, (device, local)).is_some() {
            return None;
        }
    }
    let mut devices: BTreeMap<(u32, u32), (u64, u64)> = BTreeMap::new();
    for (path, available, total) in disks {
        let (device, kind) = mounts.get(path)?;
        if !*kind {
            continue;
        }
        if device.0 == 0 || *total == 0 || *total == u64::MAX || available > total {
            return None;
        }
        match devices.get_mut(device) {
            Some((old_available, old_total)) => {
                if old_total != total {
                    return None;
                }
                // df is not atomic. Repeated/bind views count once, using the
                // lower availability rather than claiming extra free space.
                *old_available = (*old_available).min(*available);
            }
            None => {
                devices.insert(*device, (*available, *total));
            }
        }
    }
    // A failed df row or an unreported local filesystem must not turn into a
    // plausible-looking partial sum. Hidden bind views are fine only when their
    // device was measured elsewhere. Shared pools remain unknown even if df
    // omitted their duplicate-looking rows.
    for (device, local) in mounts.values() {
        if *local && !devices.contains_key(device) {
            return None;
        }
    }
    if devices.is_empty() {
        return None;
    }
    devices
        .values()
        .try_fold((0u64, 0u64), |(a, t), (available, total)| {
            Some((a.checked_add(*available)?, t.checked_add(*total)?))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn local_volumes_count_once_but_equal_sized_independent_devices_both_count() {
        let disks = vec![
            ("/".into(), 40, 100),
            ("/data".into(), 40, 100),
            ("/bind".into(), 39, 100),
            ("/tmp".into(), 400, 500),
            ("/remote".into(), 800, 1000),
            ("/container".into(), 40, 100),
        ];
        let mut info = metadata(&[
            "8:1 / / ext4",
            "8:2 / /data xfs",
            "8:1 /sub /bind ext4",
            "0:1 / /tmp tmpfs",
            "0:2 / /remote nfs4",
            "0:3 / /container overlay",
        ]);
        assert_eq!(mounted_local_capacity(&disks, &info, true), Some((79, 200)));
        info.reverse();
        let mut reversed = disks.clone();
        reversed.reverse();
        assert_eq!(
            mounted_local_capacity(&reversed, &info, true),
            Some((79, 200))
        );
    }
    #[test]
    fn absent_truncated_ambiguous_and_shared_pool_metadata_never_claim_a_total() {
        let disks = vec![("/".into(), 40, 100)];
        for info in [
            metadata(&[]),
            metadata(&["8:1 / / btrfs"]),
            metadata(&["0:1 / / zfs"]),
            metadata(&["8:1 / / ext4", "8:2 / / ext4"]),
            metadata(&["8:1 / /data ext4"]),
            metadata(&["bad data"]),
            metadata(&["0:1 / / ext4"]),
            metadata(&["7:0 / / ext4"]),
            metadata(&["8:1 / / ext4", "8:2 / /unmeasured xfs"]),
            metadata(&["8:1 / / ext4", "0:2 / /unknown unknownfs"]),
            metadata(&["8:1 / / ext4", "8:1 / /alias xfs"]),
        ] {
            assert_eq!(mounted_local_capacity(&disks, &info, true), None);
        }
        assert_eq!(
            mounted_local_capacity(&disks, &metadata(&["8:1 / / ext4"]), false),
            None
        );
    }
    #[test]
    fn full_filesystem_is_zero_available_not_unknown_and_invalid_totals_are_unknown() {
        let info = metadata(&["8:1 / / ext4"]);
        assert_eq!(
            mounted_local_capacity(&[("/".into(), 0, 100)], &info, true),
            Some((0, 100))
        );
        for capacity in [(0, 0), (101, 100)] {
            assert_eq!(
                mounted_local_capacity(&[("/".into(), capacity.0, capacity.1)], &info, true),
                None
            );
        }
    }
    #[test]
    fn escaped_mounts_and_overflow_are_handled_without_guessing() {
        let info = metadata(&[r"8:1 / /data\040volume ext4"]);
        assert_eq!(
            mounted_local_capacity(&[("/data volume".into(), 1, 2)], &info, true),
            Some((1, 2))
        );
        let info = metadata(&["8:1 / / ext4", "8:2 / /data ext4"]);
        assert_eq!(
            mounted_local_capacity(
                &[("/".into(), 1, u64::MAX), ("/data".into(), 1, 2)],
                &info,
                true
            ),
            None
        );
    }
    #[test]
    fn capped_or_inconsistent_samples_do_not_create_partial_totals() {
        let disks = vec![("/".into(), 40, 100)];
        let mut info = metadata(&["8:1 / / ext4"]);
        for index in 0..512 {
            info.push(format!("0:{} / /tmp/{} tmpfs", index + 1, index));
        }
        assert_eq!(mounted_local_capacity(&disks, &info, true), None);
        let info = metadata(&["8:1 / / ext4", "8:1 /sub /bind ext4"]);
        assert_eq!(
            mounted_local_capacity(
                &[("/".into(), 40, 100), ("/bind".into(), 40, 101)],
                &info,
                true
            ),
            None
        );
        assert_eq!(
            mounted_local_capacity(&vec![("/".into(), 40, 100); 65], &info, true),
            None
        );
        assert_eq!(
            mounted_local_capacity(&[("/".into(), u64::MAX, u64::MAX)], &info, true),
            None
        );
    }
}
