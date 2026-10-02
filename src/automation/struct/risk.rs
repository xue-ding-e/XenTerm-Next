//! What makes a command risky, as data and as one pure function.
//!
//! The MCP's `run_command` is the one automation surface that reaches a live
//! shell, and its caller is a program that decided on its own to run
//! something. The judgement "is this the kind of thing a person should
//! confirm" lives here: pattern matches over the command text, nothing
//! cleverer. A parser that tried to be smart about shell syntax would be a
//! parser a bypass could be written against; substring rules with published
//! defaults are dumb, checkable, and editable by the user.

/// Why a command was flagged. Shown to the human whose approval is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RiskVerdict {
    pub(crate) reasons: Vec<String>,
}

impl RiskVerdict {
    pub(crate) fn is_risky(&self) -> bool {
        !self.reasons.is_empty()
    }

    /// One line per reason, for the approval dialog's body.
    pub(crate) fn summary(&self) -> String {
        self.reasons.join("；")
    }
}

/// Assess `command` against the user's pattern and directory lists.
///
/// The user's lists replace the defaults when present (`migrate` never fills
/// them in), so an empty pattern list means "this user wants no command
/// matching" and is honoured. Matching is case-insensitive throughout: shells
/// do not agree on case and neither does the person writing the pattern list.
pub(crate) fn assess(command: &str, patterns: &[String], dirs: &[String]) -> RiskVerdict {
    let lowered = command.to_lowercase();
    let mut reasons = Vec::new();

    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            continue;
        }
        if lowered.contains(&pattern.to_lowercase()) {
            reasons.push(format!(
                "{}: {pattern}",
                crate::i18n::t("命令命中高危模式", "matches a risky command pattern")
            ));
        }
    }

    // Tokens, so a path matched is a path *appearing* in the command rather
    // than a substring of some longer name (`/usr` inside `/usrlocal` — the
    // tokeniser is what keeps that from firing).
    for token in lowered.split_whitespace() {
        if let Some(dir) = dir_hit(&token, dirs) {
            reasons.push(format!(
                "{}: {dir}",
                crate::i18n::t("命令涉及高风险目录", "touches a high-risk directory")
            ));
        }
    }

    reasons.dedup();
    RiskVerdict { reasons }
}

/// Whether one whitespace token names or lives inside a watched directory,
/// and which one. The token is stripped of quote characters first, because a
/// shell glues `/e"t"c` back into `/etc` and a matcher that honours the quote
/// hands the attacker a one-character bypass.
fn dir_hit(token: &str, dirs: &[String]) -> Option<String> {
    let bare = token.replace(['\'', '"'], "");
    for dir in dirs {
        let dir = dir.trim().to_lowercase();
        if dir.is_empty() {
            continue;
        }
        let hit = bare == dir
            || bare.starts_with(&(dir.clone() + "/"))
            || bare.starts_with(&(dir.clone() + "\\"));
        if hit {
            return Some(dir);
        }
    }
    None
}

/// Assess a *filesystem path* against the watched directories — the file
/// tools' side of the same gate. The pattern list does not apply: a path is
/// not a command, and pattern substrings would fire on filenames that merely
/// mention one (`notes-on-mkfs.txt`).
pub(crate) fn assess_path(path: &str, dirs: &[String]) -> RiskVerdict {
    let mut reasons = Vec::new();
    for segment in path.split_whitespace() {
        if let Some(dir) = dir_hit(&segment.to_lowercase(), dirs) {
            reasons.push(format!(
                "{}: {dir}",
                crate::i18n::t("路径涉及高风险目录", "path touches a high-risk directory")
            ));
            break;
        }
    }
    RiskVerdict { reasons }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_RISKY_DIRS, DEFAULT_RISKY_PATTERNS};

    fn lists() -> (Vec<String>, Vec<String>) {
        (
            DEFAULT_RISKY_PATTERNS.iter().map(|s| s.to_string()).collect(),
            DEFAULT_RISKY_DIRS.iter().map(|s| s.to_string()).collect(),
        )
    }

    #[test]
    fn ordinary_commands_pass() {
        let (patterns, dirs) = lists();
        let _ = crate::config::DEFAULT_RISKY_PATTERNS;
        for command in ["ls -la", "cat /home/me/notes.txt", "docker ps", "echo hello"] {
            assert!(
                !assess(command, &patterns, &dirs).is_risky(),
                "{command} should not be flagged"
            );
        }
    }

    #[test]
    fn destructive_patterns_are_caught_case_insensitively() {
        let (patterns, dirs) = lists();
        for command in [
            "rm -rf /tmp/build",
            "RM -RF /",
            "sudo mkfs.ext4 /dev/sdb1",
            "shutdown -h now",
            "dd if=/dev/zero of=/dev/sda",
        ] {
            assert!(assess(command, &patterns, &dirs).is_risky(), "{command}");
        }
    }

    #[test]
    fn high_risk_directories_are_caught_by_token_not_substring() {
        let (patterns, dirs) = lists();
        assert!(assess("rm -r /etc/nginx", &patterns, &dirs).is_risky());
        assert!(assess("cat C:\\Windows\\system32\\config", &patterns, &dirs).is_risky());
        // `/usrlocal` is not `/usr`: the token boundary is the whole point.
        assert!(!assess("ls /usrlocal/share", &patterns, &dirs).is_risky());
    }

    #[test]
    fn quoted_gluing_does_not_bypass_the_directory_match() {
        let (patterns, dirs) = lists();
        // A shell reads /e"t"c as /etc; the matcher strips the quotes and
        // agrees.
        assert!(assess("cd /e\"t\"c && rm x", &patterns, &dirs).is_risky());
        assert!(assess("cat /e't'c/passwd", &patterns, &dirs).is_risky());
    }

    #[test]
    fn file_paths_are_judged_by_directory_only() {
        let dirs = DEFAULT_RISKY_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        assert!(assess_path("/etc/shadow", &dirs).is_risky());
        assert!(assess_path(r"C:\Windows\system32\config", &dirs).is_risky());
        assert!(
            !assess_path("/home/me/notes-on-mkfs.txt", &dirs).is_risky(),
            "a filename that mentions a pattern is not a command"
        );
        assert!(
            !assess_path("/usrlocal/share", &dirs).is_risky(),
            "token boundaries still apply"
        );
    }

    #[test]
    fn an_empty_list_disables_that_kind_of_matching() {
        assert!(!assess("rm -rf /", &[], &DEFAULT_RISKY_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>())
        .is_risky());
    }

    #[test]
    fn reasons_name_the_pattern_they_hit() {
        let (patterns, dirs) = lists();
        let verdict = assess("rm -rf /var/log", &patterns, &dirs);
        assert_eq!(verdict.reasons.len(), 2, "pattern and directory both fire");
        assert!(verdict.summary().contains("rm -rf"));
        assert!(verdict.summary().contains("/var"));
    }
}
