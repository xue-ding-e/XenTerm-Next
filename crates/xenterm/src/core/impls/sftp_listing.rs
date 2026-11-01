//! One tab's SFTP panel listing, and the ordering policy applied to it.

use super::sftp::{parent_path, SftpColumn, SftpFile, SftpSort, SftpSortDir};
use crate::session::protocol::RemoteEntry;
use std::cmp::Ordering;

/// The directory listing shown in one tab's SFTP panel.
///
/// This replaces four fields of the former projected `TerminalState` row —
/// `sftp-path`, `sftp-entries`, `sftp-sort-key`, `sftp-sort-dir` — plus the
/// `selected` flag on every `SftpEntry`, all of which Rust read back out of the
/// toolkit to do real work: resolving `..`, re-sorting an inbound listing,
/// collecting the checked rows for a batch download. The row is now a projection.
///
/// `selected` is maintained as a count alongside the files rather than derived
/// by rescanning them. The row used to keep the count in a *separate* field
/// from the entries it counted, and the two drifted: reloading a directory
/// rebuilt every row unchecked but left the stale count in place, so the
/// toolbar kept offering batch actions over a list with nothing ticked. There
/// is now one value and every mutation updates it, so that class of drift has
/// nowhere to happen.
#[derive(Clone, Debug, Default)]
pub struct SftpListing {
    path: String,
    /// Display order — already sorted according to `sort`.
    files: Vec<SftpFile>,
    sort: SftpSort,
    /// Number of files with `selected` set. Always equals a fresh count of
    /// `files`; kept incrementally because the panel recounts on every click.
    selected: usize,
    /// Bumped every time this listing — or the session state around it —
    /// changed in a way the panel should hear about. The panel holds a *copy*
    /// of this listing, synced through `sync_panel`, and the copies would
    /// silently go stale forever: nothing in the event path knew the panel
    /// existed. The generation is what the shell's per-frame drain compares,
    /// so a listing (or an error, or a tree) that reached the store reaches
    /// the panel on the next frame.
    generation: u64,
}

impl SftpListing {
    /// Show a freshly read directory.
    ///
    /// The sort survives the reload — a user who asked for largest-first should
    /// keep getting largest-first as they navigate — but the selection does not,
    /// since the checked rows belonged to the previous directory.
    pub fn load(&mut self, path: String, entries: &[RemoteEntry]) {
        self.path = path;
        self.files = entries.iter().map(SftpFile::from_remote).collect();
        self.selected = 0;
        self.apply_sort();
        self.generation += 1;
    }

    /// Record that something about this session's SFTP state changed without a
    /// listing to install — an error, a tree rebuild. The panel's spinner and
    /// its tree follow the same generation as the listing, so a failed refresh
    /// ends the spinner instead of leaving it to the timeout.
    pub fn touch(&mut self) {
        self.generation += 1;
    }

    /// How many times this listing has changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Move to `path` before its listing has arrived.
    ///
    /// The shell reports a `cd` as soon as it happens, but the entries for the
    /// new directory take another round trip. Anything resolving "the current
    /// directory" in that gap — a file dropped onto the terminal, the upload
    /// button — has to see the new one already, and if the listing then fails
    /// this is what stops the panel and the store from settling on two
    /// different directories.
    ///
    /// Only the path moves. The files still shown, the sort and the selection
    /// all belong to the previous listing and stay until `load` replaces them.
    pub fn set_path(&mut self, path: String) {
        self.path = path;
        self.generation += 1;
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The directory above the current one, for the panel's `..` row.
    pub fn parent(&self) -> String {
        parent_path(&self.path)
    }

    pub fn files(&self) -> &[SftpFile] {
        &self.files
    }

    pub fn sort(&self) -> SftpSort {
        self.sort
    }

    pub fn clear_selection(&mut self) {
        for file in &mut self.files {
            file.selected = false;
        }
        self.selected = 0;
    }

    pub fn selected_count(&self) -> usize {
        self.selected
    }

    /// Reorder by `sort`, keeping whatever is selected.
    ///
    /// Sorting must not disturb the selection: the header is clickable while
    /// rows are ticked, and losing the user's checked set to a reorder would
    /// make the batch actions unusable on any large listing.
    pub fn set_sort(&mut self, sort: SftpSort) {
        self.sort = sort;
        self.apply_sort();
    }

    /// Advance the sort as if the user clicked `column`'s header, and reorder.
    pub fn advance_sort(&mut self, column: SftpColumn) -> SftpSort {
        let next = self.sort.advance(column);
        self.set_sort(next);
        next
    }

    /// Flip the checkbox on the row at `index`, returning its new state.
    ///
    /// An out-of-range index reports `None` and changes nothing: the panel
    /// passes a row index captured before the click was dispatched, and a
    /// directory reload in between makes it stale rather than an error.
    pub fn toggle_selected(&mut self, index: usize) -> Option<bool> {
        let file = self.files.get_mut(index)?;
        file.selected = !file.selected;
        if file.selected {
            self.selected += 1;
        } else {
            self.selected -= 1;
        }
        Some(file.selected)
    }

    /// Uncheck every row (#100).

    /// Absolute paths of the checked rows, in display order.
    pub fn selected_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| f.selected)
            .map(|f| f.full_path.clone())
            .collect()
    }

    fn apply_sort(&mut self) {
        sort_files(&mut self.files, self.sort);
    }
}

/// Order a listing for display: directories first, then by the chosen column.
///
/// Directories stay ahead of files even when the direction is descending, so a
/// reversed size sort does not scatter folders through the file rows.
pub fn sort_files(files: &mut [SftpFile], sort: SftpSort) {
    let dirs_first = |a: &SftpFile, b: &SftpFile| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) | (false, false) => Ordering::Equal,
    };

    let Some(dir) = sort.dir() else {
        // Server order: group directories, then natural name order within each.
        files.sort_by(|a, b| dirs_first(a, b).then_with(|| natural_name_cmp(&a.name, &b.name)));
        return;
    };

    let column = sort.column().expect("a direction implies a column");
    files.sort_by(|a, b| {
        let grouped = dirs_first(a, b);
        if grouped != Ordering::Equal {
            return grouped;
        }
        let ord = match column {
            // Exact integer comparison. The former projected row carried these
            // as `f32` for sorting, whose 24-bit mantissa cannot distinguish two
            // files above 16 MB that differ by a few bytes — a size sort could
            // report them as equal and leave them in whatever order they
            // arrived in.
            SftpColumn::Size => a.size.cmp(&b.size),
            SftpColumn::Modified => a.modified.cmp(&b.modified),
            SftpColumn::Name => Ordering::Equal,
        }
        .then_with(|| natural_name_cmp(&a.name, &b.name));
        match dir {
            SftpSortDir::Ascending => ord,
            SftpSortDir::Descending => ord.reverse(),
        }
    });
}

/// Compare two names case-insensitively, falling back to a byte compare so
/// equal-folded names still get a stable, total order.
pub fn natural_name_cmp(a: &str, b: &str) -> Ordering {
    natural_ascii_cmp(&a.to_lowercase(), &b.to_lowercase()).then_with(|| a.cmp(b))
}

/// Compare two ASCII-ish strings treating digit runs as numbers, so `file2`
/// sorts before `file10` instead of after it.
///
/// A digit run is compared by significant-digit length first and then
/// lexicographically, which orders arbitrarily large numbers correctly without
/// parsing them into an integer that could overflow. Leading zeros are skipped
/// for the length comparison but break ties, so `file01` and `file1` are
/// ordered deterministically rather than left to the sort's stability.
pub fn natural_ascii_cmp(a: &str, b: &str) -> Ordering {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let mut ai = 0;
    let mut bi = 0;
    while ai < ab.len() && bi < bb.len() {
        if ab[ai].is_ascii_digit() && bb[bi].is_ascii_digit() {
            let a_start = ai;
            let b_start = bi;
            while ai < ab.len() && ab[ai].is_ascii_digit() {
                ai += 1;
            }
            while bi < bb.len() && bb[bi].is_ascii_digit() {
                bi += 1;
            }

            let mut a_sig = a_start;
            let mut b_sig = b_start;
            while a_sig < ai && ab[a_sig] == b'0' {
                a_sig += 1;
            }
            while b_sig < bi && bb[b_sig] == b'0' {
                b_sig += 1;
            }

            let ord = (ai - a_sig)
                .cmp(&(bi - b_sig))
                .then_with(|| ab[a_sig..ai].cmp(&bb[b_sig..bi]))
                .then_with(|| (ai - a_start).cmp(&(bi - b_start)));
            if ord != Ordering::Equal {
                return ord;
            }
            continue;
        }

        let ord = ab[ai].cmp(&bb[bi]);
        if ord != Ordering::Equal {
            return ord;
        }
        ai += 1;
        bi += 1;
    }
    ab.len().cmp(&bb.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sftp::SftpSortDir;

    fn file(name: &str, is_dir: bool) -> SftpFile {
        SftpFile {
            name: name.into(),
            full_path: format!("/{name}"),
            is_dir,
            size: 0,
            modified: 0,
            mode: 0,
            selected: false,
        }
    }

    fn listing(files: Vec<SftpFile>) -> SftpListing {
        let mut l = SftpListing::default();
        l.path = "/srv".into();
        l.files = files;
        l
    }

    fn names(l: &SftpListing) -> Vec<&str> {
        l.files.iter().map(|f| f.name.as_str()).collect()
    }

    fn remote(name: &str, size: u64) -> RemoteEntry {
        RemoteEntry {
            name: name.into(),
            full_path: format!("/srv/{name}"),
            is_dir: false,
            size,
            modified: 0,
            mode: 0o644,
        }
    }

    // --- ordering policy --------------------------------------------------

    #[test]
    fn name_sort_uses_natural_numeric_order() {
        let mut files = ["file100", "file10", "file2", "file11", "file1"]
            .map(|n| file(n, false))
            .to_vec();

        sort_files(&mut files, SftpSort::Name(SftpSortDir::Ascending));
        let got: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(got, ["file1", "file2", "file10", "file11", "file100"]);

        sort_files(&mut files, SftpSort::Name(SftpSortDir::Descending));
        let got: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(got, ["file100", "file11", "file10", "file2", "file1"]);
    }

    #[test]
    fn unsorted_keeps_dirs_first_with_natural_names() {
        let mut files = vec![
            file("file100", false),
            file("dir10", true),
            file("file11", false),
            file("dir2", true),
        ];
        sort_files(&mut files, SftpSort::Unsorted);
        let got: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(got, ["dir2", "dir10", "file11", "file100"]);
    }

    #[test]
    fn descending_keeps_directories_ahead_of_files() {
        let mut files = vec![
            file("zeta", false),
            file("alpha", true),
            file("beta", false),
        ];
        sort_files(&mut files, SftpSort::Name(SftpSortDir::Descending));
        let got: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        // Reversed within each group, but the directory group still leads.
        assert_eq!(got, ["alpha", "zeta", "beta"]);
    }

    #[test]
    fn size_sort_distinguishes_large_files_that_f32_cannot() {
        // These differ by one byte above f32's 24-bit mantissa, so sorting on
        // the `f32` the former projected row carried compared them as equal.
        let mut files = vec![file("b", false), file("a", false), file("c", false)];
        files[0].size = 100_000_002;
        files[1].size = 100_000_001;
        files[2].size = 100_000_003;
        assert_eq!(
            files[0].size as f32, files[1].size as f32,
            "f32 loses the byte"
        );

        sort_files(&mut files, SftpSort::Size(SftpSortDir::Ascending));
        let got: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(got, ["a", "b", "c"]);
    }

    #[test]
    fn equal_keys_fall_back_to_natural_name_order() {
        let mut files = vec![file("b", false), file("a", false)];
        // Identical size and mtime: the tiebreak must be deterministic.
        for f in &mut files {
            f.size = 10;
            f.modified = 5;
        }
        sort_files(&mut files, SftpSort::Size(SftpSortDir::Ascending));
        let got: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(got, ["a", "b"]);
    }

    #[test]
    fn natural_compare_orders_leading_zeros_deterministically() {
        assert_eq!(natural_name_cmp("file1", "file01"), Ordering::Less);
        assert_eq!(natural_name_cmp("file01", "file1"), Ordering::Greater);
        assert_eq!(natural_name_cmp("file2", "file10"), Ordering::Less);
    }

    // --- listing state ----------------------------------------------------

    #[test]
    fn load_replaces_the_listing_and_clears_the_selection() {
        let mut l = listing(vec![file("old", false)]);
        l.toggle_selected(0);
        assert_eq!(l.selected_count(), 1);

        l.load("/srv/next".into(), &[remote("a", 1), remote("b", 2)]);

        assert_eq!(l.path(), "/srv/next");
        assert_eq!(names(&l), ["a", "b"]);
        assert_eq!(
            l.selected_count(),
            0,
            "the previous directory's checkboxes must not survive a reload"
        );
        assert!(l.selected_paths().is_empty());
    }

    /// The generation is what the shell's drain compares to decide the panel's
    /// copy has gone stale, so every session-side change must move it — and
    /// local-only edits (selection, sort) must not, or the panel would be
    /// re-synced on every click for no reason.
    #[test]
    fn session_side_changes_advance_the_generation_and_local_ones_do_not() {
        let mut l = SftpListing::default();
        let start = l.generation();

        l.set_path("/srv".into());
        assert_eq!(l.generation(), start + 1, "a path change is session news");

        l.load("/srv".into(), &[remote("a", 1)]);
        assert_eq!(l.generation(), start + 2, "a listing arrival is session news");

        l.touch();
        assert_eq!(l.generation(), start + 3, "an error or tree rebuild is session news");

        l.toggle_selected(0);
        l.advance_sort(crate::core::sftp::SftpColumn::Name);
        assert_eq!(
            l.generation(),
            start + 3,
            "the user's own clicks are not session news"
        );
    }

    #[test]
    fn load_applies_the_existing_sort_to_the_new_entries() {
        let mut l = SftpListing::default();
        l.set_sort(SftpSort::Size(SftpSortDir::Descending));

        l.load("/srv".into(), &[remote("small", 1), remote("big", 1000)]);

        assert_eq!(
            names(&l),
            ["big", "small"],
            "largest-first must persist across navigation"
        );
        assert_eq!(l.sort(), SftpSort::Size(SftpSortDir::Descending));
    }

    #[test]
    fn set_path_moves_the_directory_without_touching_what_is_shown() {
        let mut l = listing(vec![file("a", false), file("b", false)]);
        l.set_sort(SftpSort::Name(SftpSortDir::Descending));
        l.toggle_selected(1);

        l.set_path("/srv/next".into());

        assert_eq!(l.path(), "/srv/next");
        assert_eq!(l.parent(), "/srv", "the `..` row must follow immediately");
        assert_eq!(
            names(&l),
            ["b", "a"],
            "the previous rows stay on screen until the new listing lands"
        );
        assert_eq!(l.sort(), SftpSort::Name(SftpSortDir::Descending));
        assert_eq!(l.selected_count(), 1);
    }

    #[test]
    fn sorting_preserves_the_selection() {
        let mut l = listing(vec![file("b", false), file("a", false)]);
        l.toggle_selected(0); // "b"
        assert_eq!(l.selected_paths(), ["/b"]);

        l.set_sort(SftpSort::Name(SftpSortDir::Ascending));

        assert_eq!(names(&l), ["a", "b"]);
        assert_eq!(
            l.selected_paths(),
            ["/b"],
            "reordering must not drop checked rows"
        );
        assert_eq!(l.selected_count(), 1);
    }

    #[test]
    fn advance_sort_reorders_and_reports_the_new_state() {
        let mut l = listing(vec![file("file10", false), file("file2", false)]);

        assert_eq!(
            l.advance_sort(SftpColumn::Name),
            SftpSort::Name(SftpSortDir::Ascending)
        );
        assert_eq!(names(&l), ["file2", "file10"]);

        assert_eq!(
            l.advance_sort(SftpColumn::Name),
            SftpSort::Name(SftpSortDir::Descending)
        );
        assert_eq!(names(&l), ["file10", "file2"]);

        assert_eq!(l.advance_sort(SftpColumn::Name), SftpSort::Unsorted);
    }

    #[test]
    fn toggling_selection_keeps_the_count_exact() {
        let mut l = listing(vec![file("a", false), file("b", false), file("c", false)]);

        assert_eq!(l.toggle_selected(0), Some(true));
        assert_eq!(l.toggle_selected(2), Some(true));
        assert_eq!(l.selected_count(), 2);

        // Un-ticking must decrement, not just leave the old total behind.
        assert_eq!(l.toggle_selected(0), Some(false));
        assert_eq!(l.selected_count(), 1);
        assert_eq!(l.selected_paths(), ["/c"]);
    }

    #[test]
    fn toggling_an_out_of_range_row_changes_nothing() {
        let mut l = listing(vec![file("a", false)]);
        l.toggle_selected(0);

        // The panel dispatches a row index captured before the click; a reload
        // in between can make it stale.
        assert_eq!(l.toggle_selected(9), None);
        assert_eq!(
            l.selected_count(),
            1,
            "a stale index must not corrupt the count"
        );
    }

    #[test]
    fn clear_selection_unchecks_every_row() {
        let mut l = listing(vec![file("a", false), file("b", false)]);
        l.toggle_selected(0);
        l.toggle_selected(1);
        assert_eq!(l.selected_count(), 2);

        l.clear_selection();

        assert_eq!(l.selected_count(), 0);
        assert!(l.selected_paths().is_empty());
        assert!(l.files().iter().all(|f| !f.selected));
    }

    #[test]
    fn selected_paths_follow_display_order() {
        let mut l = listing(vec![file("b", false), file("a", false)]);
        l.toggle_selected(0); // "b"
        l.toggle_selected(1); // "a"

        // Display order is the stored order, which here is b then a.
        assert_eq!(l.selected_paths(), ["/b", "/a"]);
    }

    #[test]
    fn parent_climbs_from_the_current_path() {
        let mut l = SftpListing::default();
        l.load("/srv/data/logs".into(), &[]);
        assert_eq!(l.parent(), "/srv/data");

        l.load("/".into(), &[]);
        assert_eq!(l.parent(), "/");
    }

    #[test]
    fn an_empty_listing_is_in_server_order_at_the_root_of_nothing() {
        let l = SftpListing::default();
        assert_eq!(l.path(), "");
        assert!(l.files().is_empty());
        assert_eq!(l.sort(), SftpSort::Unsorted);
        assert_eq!(l.selected_count(), 0);
    }
}
