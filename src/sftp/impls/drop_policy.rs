//! Drop-only path and queue policy. No filesystem, transport or shell operations.
use std::collections::HashSet;
use std::path::{Component, Path};

pub(super) const MAX_DROPPED: usize = 128;

pub(super) fn destination(local: &Path, directory: &str) -> Result<String, &'static str> {
    if !local.is_absolute()
        || local
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Dropped files must have absolute paths without parent components");
    }
    if !directory.starts_with('/') || directory.contains(['\0', '\r', '\n']) {
        return Err("Invalid remote drop directory");
    }
    let name = local
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid UTF-8 file name")?;
    if name.is_empty() || name.contains(['/', '\\', '\0', '\r', '\n']) {
        return Err("Unsupported dropped file name");
    }
    Ok(format!("{}/{name}", directory.trim_end_matches('/')))
}

#[derive(Default)]
pub(super) struct Reservations(HashSet<String>);

impl Reservations {
    pub(super) fn reserve(&mut self, path: &str) -> Result<(), &'static str> {
        if self.0.contains(path) {
            return Err("A dropped upload to this destination is already queued or active");
        }
        if self.0.len() >= MAX_DROPPED {
            return Err("The dropped-upload queue is full (128 files)");
        }
        self.0.insert(path.into());
        Ok(())
    }

    pub(super) fn release(&mut self, path: &str) {
        self.0.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_spaces_and_shell_syntax_stay_literal() {
        let base = std::env::temp_dir();
        assert_eq!(
            destination(&base.join("中文 $(id).txt"), "/目标/"),
            Ok("/目标/中文 $(id).txt".into())
        );
    }

    #[test]
    fn malformed_sources_and_destinations_are_rejected() {
        assert!(destination(Path::new("relative.txt"), "/").is_err());
        let base = std::env::temp_dir();
        assert!(destination(&base.join("../file"), "/").is_err());
        for dir in ["relative", "", "/bad\0", "/bad\n"] {
            assert!(destination(&base.join("file"), dir).is_err());
        }
        for name in ["a\nb", "a\rb"] {
            assert!(
                destination(&base.join(name), "/").is_err(),
                "name: {name:?}"
            );
        }
        // Backslash is a literal filename character on Unix, but a native
        // directory separator on Windows. Do not confuse a path with its basename.
        #[cfg(unix)]
        assert!(
            destination(&base.join("a\\b"), "/").is_err(),
            "Unix literal backslash name"
        );
        #[cfg(windows)]
        assert_eq!(
            destination(&base.join("a\\b"), "/"),
            Ok("/b".into()),
            "Windows native path separator"
        );
        assert_eq!(
            destination(&base.join("nested").join("file.txt"), "/"),
            Ok("/file.txt".into()),
            "native nested path uses its basename"
        );
    }

    #[test]
    fn duplicate_remote_names_remain_reserved_until_completion() {
        let mut queue = Reservations::default();
        assert!(queue.reserve("/same/name").is_ok());
        assert!(queue.reserve("/same/name").is_err());
        assert!(queue.reserve("/other/name").is_ok());
        queue.release("/same/name");
        assert!(queue.reserve("/same/name").is_ok());
    }

    #[test]
    fn queue_capacity_is_global_across_batches_and_reusable() {
        let mut queue = Reservations::default();
        for n in 0..MAX_DROPPED {
            assert!(queue.reserve(&format!("/{n}")).is_ok());
        }
        assert!(queue.reserve("/overflow").is_err());
        queue.release("/0");
        assert!(queue.reserve("/overflow").is_ok());
    }
}
