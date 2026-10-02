//! Value types for one SFTP panel's directory listing.

use crate::session::protocol::RemoteEntry;
use crate::ssh::{format_mtime, format_size};

/// One file or directory shown in the SFTP panel.
///
/// The projected `SftpEntry` row is derived from this. It carries the raw
/// `size`/`modified` values rather than the pre-formatted strings the row shows,
/// so ordering is computed on exact integers; see `SftpSort` for why that
/// matters.
///
/// `selected` lives here and nowhere else. It used to exist only inside the
/// toolkit's model, which made that model the sole record of what the user had
/// ticked — unreadable from Rust without walking it, and invisible to anything
/// outside the toolkit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpFile {
    pub name: String,
    pub full_path: String,
    pub is_dir: bool,
    /// Raw size in bytes. 0 for directories.
    pub size: u64,
    /// Modification time as a Unix timestamp (SFTP reports seconds in a u32).
    pub modified: u32,
    /// POSIX permission bits, the low 12 — prefills the chmod dialog (#84).
    pub mode: u32,
    /// Whether the panel's checkbox for this row is ticked (#100).
    pub selected: bool,
}

impl SftpFile {
    /// Adopt an entry reported by the SFTP worker. Nothing arrives selected:
    /// reloading a directory always presents an unchecked list.
    pub fn from_remote(entry: &RemoteEntry) -> Self {
        Self {
            name: entry.name.clone(),
            full_path: entry.full_path.clone(),
            is_dir: entry.is_dir,
            size: entry.size,
            modified: entry.modified,
            mode: entry.mode & 0o7777,
            selected: false,
        }
    }

    /// The size column. Directories render blank rather than "0 B" — a
    /// directory has no size to report, and showing one invites the reading
    /// that it is an empty file.
    pub fn size_text(&self) -> String {
        if self.is_dir {
            String::new()
        } else {
            format_size(self.size)
        }
    }

    /// The modified column, in the machine's local timezone (#168).
    pub fn modified_text(&self) -> String {
        format_mtime(self.modified)
    }
}

/// A sortable column of the SFTP file list (#248).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpColumn {
    Name,
    Size,
    Modified,
}

impl SftpColumn {
    /// The key string the panel's header sends with a sort request.
    pub fn wire_key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Modified => "modified",
        }
    }

    /// Decode a header's key string. Unknown columns are not sortable, so they
    /// report `None` and leave the listing in server order rather than guessing.
    pub fn from_wire_key(key: &str) -> Option<Self> {
        match key {
            "name" => Some(Self::Name),
            "size" => Some(Self::Size),
            "modified" => Some(Self::Modified),
            _ => None,
        }
    }
}

/// Sort direction of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpSortDir {
    Ascending,
    Descending,
}

impl SftpSortDir {
    /// The `sftp-sort-dir` int the panel row carries: 1 ascending, -1 descending.
    pub fn wire_dir(self) -> i32 {
        match self {
            Self::Ascending => 1,
            Self::Descending => -1,
        }
    }
}

/// How the file list is ordered.
///
/// One enum rather than the `(sort_key: string, sort_dir: int)` pair the
/// projected row carried. That pair could represent four states that mean
/// nothing — an empty key with a direction, or a key with direction 0 — and the
/// callbacks had to reason about both to work out what the user had clicked.
/// Here the only unsorted state is `Unsorted`, and a sorted state always names
/// both its column and its direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SftpSort {
    /// Server order, with directories grouped ahead of files.
    #[default]
    Unsorted,
    Name(SftpSortDir),
    Size(SftpSortDir),
    Modified(SftpSortDir),
}

impl SftpSort {
    pub fn column(self) -> Option<SftpColumn> {
        match self {
            Self::Unsorted => None,
            Self::Name(_) => Some(SftpColumn::Name),
            Self::Size(_) => Some(SftpColumn::Size),
            Self::Modified(_) => Some(SftpColumn::Modified),
        }
    }

    pub fn dir(self) -> Option<SftpSortDir> {
        match self {
            Self::Unsorted => None,
            Self::Name(d) | Self::Size(d) | Self::Modified(d) => Some(d),
        }
    }

    pub fn sorted(column: SftpColumn, dir: SftpSortDir) -> Self {
        match column {
            SftpColumn::Name => Self::Name(dir),
            SftpColumn::Size => Self::Size(dir),
            SftpColumn::Modified => Self::Modified(dir),
        }
    }

    /// Next state when the user clicks `column`'s header: ascending, then
    /// descending, then back to server order (#248).
    ///
    /// Clicking a *different* column restarts the cycle at ascending rather
    /// than inheriting the old column's direction, so a header always shows the
    /// arrow the user just asked for.
    pub fn advance(self, column: SftpColumn) -> Self {
        if self.column() != Some(column) {
            return Self::sorted(column, SftpSortDir::Ascending);
        }
        match self.dir() {
            Some(SftpSortDir::Ascending) => Self::sorted(column, SftpSortDir::Descending),
            // Descending completes the cycle; `Unsorted` cannot reach here
            // because its column is `None` and so never equals `Some(column)`.
            _ => Self::Unsorted,
        }
    }

}

/// The directory above `path`, for the panel's `..` row.
///
/// Everything below the root folds to `/` rather than to an empty string, so
/// navigating up from `/etc` and from `/` both land on a listing that still
/// works instead of requesting a path the server will reject.
pub fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
        None => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_cycle_is_ascending_then_descending_then_server_order() {
        let mut sort = SftpSort::Unsorted;

        sort = sort.advance(SftpColumn::Name);
        assert_eq!(sort, SftpSort::Name(SftpSortDir::Ascending));

        sort = sort.advance(SftpColumn::Name);
        assert_eq!(sort, SftpSort::Name(SftpSortDir::Descending));

        sort = sort.advance(SftpColumn::Name);
        assert_eq!(
            sort,
            SftpSort::Unsorted,
            "the third click returns to server order"
        );

        sort = sort.advance(SftpColumn::Name);
        assert_eq!(sort, SftpSort::Name(SftpSortDir::Ascending));
    }

    #[test]
    fn clicking_a_different_column_restarts_at_ascending() {
        // Size was descending; switching to Modified must not inherit that.
        let sort = SftpSort::Size(SftpSortDir::Descending).advance(SftpColumn::Modified);
        assert_eq!(sort, SftpSort::Modified(SftpSortDir::Ascending));
    }

    #[test]
    fn clicking_a_column_while_unsorted_starts_at_ascending() {
        let sort = SftpSort::Unsorted.advance(SftpColumn::Size);
        assert_eq!(sort, SftpSort::Size(SftpSortDir::Ascending));
    }

    #[test]
    fn directories_have_no_size_text() {
        let dir = SftpFile {
            name: "etc".into(),
            full_path: "/etc".into(),
            is_dir: true,
            size: 0,
            modified: 0,
            mode: 0o755,
            selected: false,
        };
        assert!(dir.size_text().is_empty());
    }

    #[test]
    fn from_remote_masks_permissions_to_the_low_twelve_bits() {
        let entry = RemoteEntry {
            name: "f".into(),
            full_path: "/f".into(),
            is_dir: false,
            size: 10,
            modified: 0,
            // The server can report a file-type nibble above the permission
            // bits; the chmod dialog only wants rwx + setuid/setgid/sticky.
            mode: 0o100644,
        };
        assert_eq!(SftpFile::from_remote(&entry).mode, 0o644);
        assert!(!SftpFile::from_remote(&entry).selected);
    }

    #[test]
    fn parent_path_climbs_to_the_root_and_stops() {
        assert_eq!(parent_path("/etc/hosts"), "/etc");
        assert_eq!(parent_path("/etc"), "/");
        assert_eq!(parent_path("/"), "/");
        assert_eq!(parent_path(""), "/");
        // Trailing separators must not make a directory its own parent.
        assert_eq!(parent_path("/etc/"), "/");
        assert_eq!(parent_path("/var/log/"), "/var");
        // A relative path has no parent to climb to.
        assert_eq!(parent_path("etc"), "/");
    }
}
