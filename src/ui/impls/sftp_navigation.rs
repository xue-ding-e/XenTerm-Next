//! Remote navigation is an SFTP path, never a local path or a shell command.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectoryTarget {
    pub tab: String,
    pub path: String,
    pub listing_generation: u64,
    pub connection_generation: u64,
}

/// Preserve POSIX spelling (including spaces and symlink-sensitive `..`).
/// Relative paths resolve against the displayed directory, not the server's
/// login directory. Tildes and shell metacharacters remain literal filenames.
pub(crate) fn entered_path(current: &str, entered: &str) -> Option<String> {
    if entered.trim().is_empty() || entered.contains(['\0', '\r', '\n']) {
        return None;
    }
    if entered.starts_with('/') {
        return Some(entered.to_string());
    }
    if !current.starts_with('/') || current.contains(['\0', '\r', '\n']) {
        return None;
    }
    Some(format!("{}/{}", current.trim_end_matches('/'), entered))
}

#[cfg(test)]
mod tests {
    use super::entered_path;

    #[test]
    fn absolute_and_unicode_paths_are_not_reinterpreted() {
        assert_eq!(entered_path("/old", "/共享/带 空格/"), Some("/共享/带 空格/".into()));
        assert_eq!(entered_path("", "/"), Some("/".into()));
    }

    #[test]
    fn relative_paths_use_the_visible_directory_and_keep_symlink_semantics() {
        assert_eq!(entered_path("/srv/current/", "../中文"), Some("/srv/current/../中文".into()));
        assert_eq!(entered_path("/", "./logs"), Some("/./logs".into()));
        assert_eq!(entered_path("/srv", " name "), Some("/srv/ name ".into()));
    }

    #[test]
    fn blank_and_control_inputs_do_not_navigate() {
        for value in ["", "   ", "\t", "/a\0b", "a\nb", "a\rb"] {
            assert_eq!(entered_path("/srv", value), None);
        }
        assert_eq!(entered_path("", "relative"), None);
        assert_eq!(entered_path("relative", "child"), None);
    }

    #[test]
    fn shell_syntax_is_literal_and_backslash_is_not_a_local_separator() {
        for value in ["~", "$(touch bad)", "a;b", "a\\b"] {
            assert_eq!(entered_path("/srv", value), Some(format!("/srv/{value}")));
        }
    }
}
