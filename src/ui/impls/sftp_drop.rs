//! Cheap drop admission only; the SFTP worker validates file type and I/O.
//! Never inspect a local file merely because a drag passed over the window.
use std::collections::HashSet;
use std::path::PathBuf;

pub(crate) const MAX_DROPPED_FILES: usize = 128;

pub(crate) fn dropped_paths(paths: &[PathBuf]) -> Option<Vec<PathBuf>> {
    if paths.is_empty() || paths.len() > MAX_DROPPED_FILES || paths.iter().any(|path| !path.is_absolute()) {
        return None;
    }
    let mut seen = HashSet::new();
    Some(paths.iter().filter(|path| seen.insert((*path).clone())).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_preserves_unicode_and_order_while_deduplicating_the_same_path() {
        let a = std::env::temp_dir().join("中文 文件.txt");
        let b = std::env::temp_dir().join("other.txt");
        assert_eq!(dropped_paths(&[a.clone(), b.clone(), a.clone()]), Some(vec![a, b]));
    }

    #[test]
    fn drop_rejects_empty_relative_and_over_limit_payloads_as_a_whole() {
        assert_eq!(dropped_paths(&[]), None);
        assert_eq!(dropped_paths(&[PathBuf::from("relative.txt")]), None);
        let path = std::env::temp_dir().join("synthetic.txt");
        assert_eq!(dropped_paths(&[path.clone(), PathBuf::from("relative.txt")]), None);
        assert_eq!(dropped_paths(&vec![path.clone(); MAX_DROPPED_FILES + 1]), None);
        assert_eq!(dropped_paths(&vec![path.clone(); MAX_DROPPED_FILES]), Some(vec![path]));
    }
}
