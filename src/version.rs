//! The application version string (版本号规范 v1).
//!
//! The value is assembled at build time by `scripts/version.sh` — the only
//! legal assembly point — and injected through `build.rs` as the compile-time
//! `XT_VERSION`. Runtime code only reads it: no reassembly, no git, no
//! network (§3). Stable builds carry `xt-vX.Y.Z`; prerelease builds carry
//! `xt-<channel>-<hash7>-T<unix_seconds>` with channel ∈ beta|alpha|dev (§1).

/// The full version string, e.g. `xt-v0.7.3` or `xt-beta-1a2b3c4-T1760000000`.
///
/// This is the canonical identity of the running build: display it whole,
/// report it to MCP clients and persist it wherever a build needs to be
/// identified later (§8).
pub const VERSION: &str = env!("XT_VERSION");

/// The release channel a version belongs to (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
    Alpha,
    Dev,
}

/// A parsed `xt-` version string.
///
/// Stable keeps only the semver triple; prerelease channels keep the build
/// hash and Unix timestamp instead (§1). Which fields are set is therefore a
/// property of the channel, and the accessors below return `None` rather
/// than a made-up value when a field does not apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    pub channel: Channel,
    semver: Option<(u64, u64, u64)>,
    hash: Option<String>,
    timestamp: Option<u64>,
}

impl VersionInfo {
    /// Parse a version string per the §1 grammar. Anything the format regex
    /// rejects returns `None`.
    pub fn parse(s: &str) -> Option<Self> {
        let rest = s.strip_prefix("xt-")?;
        if let Some(triple) = rest.strip_prefix('v') {
            // Stable: exactly three numeric segments (the regex allows any
            // run of digits, so leading zeros stay parseable, just unusual).
            let mut parts = triple.split('.');
            let major = numeric(parts.next()?)?;
            let minor = numeric(parts.next()?)?;
            let patch = numeric(parts.next()?)?;
            if parts.next().is_some() {
                return None;
            }
            return Some(Self {
                channel: Channel::Stable,
                semver: Some((major, minor, patch)),
                hash: None,
                timestamp: None,
            });
        }

        // Prerelease: <channel>-<7 lowercase hex>-T<10 digits>.
        let mut parts = rest.splitn(3, '-');
        let channel = match parts.next()? {
            "beta" => Channel::Beta,
            "alpha" => Channel::Alpha,
            "dev" => Channel::Dev,
            _ => return None,
        };
        let hash = parts.next()?;
        let is_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
        if hash.len() != 7 || !hash.bytes().all(is_hex) {
            return None;
        }
        let timestamp = numeric(parts.next()?.strip_prefix('T')?)?;
        // §1 pins the timestamp to exactly ten digits; anything outside
        // 1e9..=9999999999 is a different width and therefore invalid.
        if !(1_000_000_000..=9_999_999_999).contains(&timestamp) {
            return None;
        }
        Some(Self {
            channel,
            semver: None,
            hash: Some(hash.to_string()),
            timestamp: Some(timestamp),
        })
    }

    /// The `X.Y.Z` triple of a stable version.
    pub fn semver(&self) -> Option<(u64, u64, u64)> {
        self.semver
    }

    /// The 7-char build hash of a prerelease version.
    pub fn hash(&self) -> Option<&str> {
        self.hash.as_deref()
    }

    /// The Unix seconds of a prerelease build.
    pub fn timestamp(&self) -> Option<u64> {
        self.timestamp
    }
}

fn numeric(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// What an update check should do with a remote version (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    /// The remote build is newer — offer the update.
    Update,
    /// The remote is the same build (same stable version, or same beta hash).
    Current,
    /// The remote differs but is not newer. Don't update, and record the
    /// anomaly (§8: a beta with a different hash but an older/equal
    /// timestamp, or a stable older than the running one).
    Skew,
    /// No update detection for this pair: alpha/dev builds never check (§8),
    /// and two different channels (stable vs beta) are separate series that
    /// the spec does not compare.
    Disabled,
}

/// Compare the running `current` against a `remote` version per §8.
///
/// Returns `None` only when either string is not a valid version — callers
/// treat that as "no information", not as "up to date".
pub fn update_decision(current: &str, remote: &str) -> Option<UpdateDecision> {
    use std::cmp::Ordering;
    use Channel::*;

    let current = VersionInfo::parse(current)?;
    let remote = VersionInfo::parse(remote)?;

    if matches!(current.channel, Alpha | Dev) || matches!(remote.channel, Alpha | Dev) {
        return Some(UpdateDecision::Disabled);
    }
    if current.channel != remote.channel {
        return Some(UpdateDecision::Disabled);
    }

    match current.channel {
        Stable => {
            let local = current.semver()?;
            let offered = remote.semver()?;
            Some(match offered.cmp(&local) {
                Ordering::Greater => UpdateDecision::Update,
                Ordering::Equal => UpdateDecision::Current,
                Ordering::Less => UpdateDecision::Skew,
            })
        }
        Beta => {
            let local_hash = current.hash()?;
            let remote_hash = remote.hash()?;
            let local_ts = current.timestamp()?;
            let remote_ts = remote.timestamp()?;
            // Same hash → same build, not an update (§8). Different hash →
            // newer timestamp wins; older/equal is an anomaly to record.
            Some(if local_hash == remote_hash {
                UpdateDecision::Current
            } else if remote_ts > local_ts {
                UpdateDecision::Update
            } else {
                UpdateDecision::Skew
            })
        }
        _ => Some(UpdateDecision::Disabled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use UpdateDecision::*;

    #[test]
    fn injected_version_parses() {
        // build.rs validated the string already; this guards the parser
        // against drifting away from the injected format.
        assert!(VersionInfo::parse(VERSION).is_some(), "VERSION={VERSION}");
    }

    #[test]
    fn parses_stable() {
        let v = VersionInfo::parse("xt-v0.7.3").unwrap();
        assert_eq!(v.channel, Channel::Stable);
        assert_eq!(v.semver(), Some((0, 7, 3)));
        assert_eq!(v.hash(), None);
        assert_eq!(v.timestamp(), None);
    }

    #[test]
    fn parses_prerelease_channels() {
        for (s, channel) in [
            ("xt-beta-1a2b3c4-T1760000000", Channel::Beta),
            ("xt-alpha-0000000-T1000000000", Channel::Alpha),
            ("xt-dev-abcdef0-T9999999999", Channel::Dev),
        ] {
            let v = VersionInfo::parse(s).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(v.channel, channel);
            assert!(v.hash().unwrap().len() == 7);
            assert!(v.timestamp().is_some());
        }
    }

    #[test]
    fn rejects_malformed_strings() {
        for s in [
            "",
            "0.7.3",
            "xt-0.7.3",
            "xt-v0.7",
            "xt-v0.7.3.1",
            "xt-v0.7.x",
            "xt-rc-1a2b3c4-T1760000000", // rc folds into channel whitelist (§1)
            "xt-beta-1A2B3C4-T1760000000", // uppercase hash
            "xt-beta-1a2b3c-T1760000000", // 6-char hash
            "xt-beta-1a2b3c4a-T1760000000", // 8-char hash
            "xt-beta-1a2b3c4-1760000000", // missing T
            "xt-beta-1a2b3c4-T176000000", // 9-digit timestamp
            "xt-beta-1a2b3c4-T17600000000", // 11-digit timestamp
            "xt-beta-1a2b3c4-T1760000000-extra",
            "XT-V0.7.3",
        ] {
            assert!(VersionInfo::parse(s).is_none(), "should reject {s:?}");
        }
    }

    #[test]
    fn stable_update_detection_compares_semver() {
        let d = |cur: &str, rem: &str| update_decision(cur, rem).unwrap();
        assert_eq!(d("xt-v0.7.3", "xt-v0.7.4"), Update);
        assert_eq!(d("xt-v0.7.3", "xt-v0.8.0"), Update);
        assert_eq!(d("xt-v0.7.3", "xt-v1.0.0"), Update);
        assert_eq!(d("xt-v0.7.3", "xt-v0.7.3"), Current);
        // Remote older than local: not an update, and worth recording (§8).
        assert_eq!(d("xt-v0.7.3", "xt-v0.7.2"), Skew);
    }

    #[test]
    fn beta_update_detection_uses_hash_then_timestamp() {
        let d = |cur: &str, rem: &str| update_decision(cur, rem).unwrap();
        // Same hash → same build regardless of timestamp.
        assert_eq!(
            d("xt-beta-1a2b3c4-T1760000000", "xt-beta-1a2b3c4-T1760009999"),
            Current
        );
        // Different hash, newer build → update.
        assert_eq!(
            d("xt-beta-1a2b3c4-T1760000000", "xt-beta-abcdef0-T1760000001"),
            Update
        );
        // Different hash, equal timestamp → anomaly, no update.
        assert_eq!(
            d("xt-beta-1a2b3c4-T1760000000", "xt-beta-abcdef0-T1760000000"),
            Skew
        );
        // Different hash, older timestamp → anomaly, no update.
        assert_eq!(
            d("xt-beta-1a2b3c4-T1760000001", "xt-beta-abcdef0-T1760000000"),
            Skew
        );
    }

    #[test]
    fn alpha_dev_and_cross_channel_have_no_update_detection() {
        let d = |cur: &str, rem: &str| update_decision(cur, rem).unwrap();
        assert_eq!(
            d("xt-dev-1a2b3c4-T1760000000", "xt-dev-abcdef0-T1760009999"),
            Disabled
        );
        assert_eq!(
            d(
                "xt-alpha-1a2b3c4-T1760000000",
                "xt-alpha-abcdef0-T1760009999"
            ),
            Disabled
        );
        assert_eq!(d("xt-beta-1a2b3c4-T1760000000", "xt-v0.7.3"), Disabled);
        assert_eq!(d("xt-v0.7.3", "xt-beta-1a2b3c4-T1760000000"), Disabled);
    }

    #[test]
    fn invalid_input_reports_no_information() {
        assert_eq!(update_decision("xt-v0.7.3", "0.7.4"), None);
        assert_eq!(update_decision("garbage", "xt-v0.7.3"), None);
    }
}
