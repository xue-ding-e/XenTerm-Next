//! The command box's history, as the dropdown shows it.
//!
//! History is stored oldest-first and deduplicated by the config (#113): using a command
//! again moves it to the end rather than adding a second copy. The dropdown shows that
//! order, filtered — oldest at the top, the command you just ran at the bottom nearest the
//! input box, which is where the original puts it.
//!
//! Filtering keeps each row's storage index, because deleting a row acts on the store
//! rather than on the displayed position: the two differ as soon as a filter is typed.

/// One row of the dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRow {
    pub command: String,
    /// Where the entry sits in the stored history, for a delete to act on.
    pub index: usize,
}

/// The rows to show for `query`, in storage order.
///
/// A case-insensitive substring match, and an empty query is every entry: the dropdown is
/// opened to *find* a command you ran, and a filter that needed exact text would be a
/// filter nobody uses.
pub fn filtered(history: &[String], query: &str) -> Vec<HistoryRow> {
    let needle = query.trim().to_lowercase();
    history
        .iter()
        .enumerate()
        .filter(|(_, command)| needle.is_empty() || command.to_lowercase().contains(&needle))
        .map(|(index, command)| HistoryRow {
            command: command.clone(),
            index,
        })
        .collect()
}

/// Local history suggestions, not claims about commands available on a server.
/// Match the exact prefix (shells are case-sensitive), newest first. Bound both
/// work and presentation; keep the original string intact for recall/copying.
pub fn suggestions(history: &[String], query: &str) -> Vec<HistoryRow> {
    const LIMIT: usize = 5;
    if query.chars().take(513).count() > 512 || query.trim().is_empty() {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    history.iter().enumerate().rev().take(500)
        .filter(|(_, command)| {
            command.as_str() != query
                && command.starts_with(query)
                && command.chars().take(4097).count() <= 4096
                && !command.chars().any(char::is_control)
                && seen.insert(command.as_str())
        })
        .take(LIMIT)
        .map(|(index, command)| HistoryRow { index, command: command.clone() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> Vec<String> {
        vec![
            "dir".to_string(),
            "Get-ChildItem -Force".to_string(),
            "echo hello".to_string(),
        ]
    }

    #[test]
    fn an_empty_query_shows_everything_in_storage_order() {
        let rows = filtered(&history(), "");
        let commands: Vec<&str> = rows.iter().map(|row| row.command.as_str()).collect();
        assert_eq!(commands, ["dir", "Get-ChildItem -Force", "echo hello"]);
        assert_eq!(
            rows.iter().map(|row| row.index).collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn whitespace_is_not_a_search_term() {
        // A query of spaces is what a half-typed search looks like, and showing nothing
        // there would read as an empty history.
        assert_eq!(filtered(&history(), "   ").len(), 3);
    }

    #[test]
    fn the_match_ignores_case_and_needle_position() {
        let rows = filtered(&history(), "CHILDITEM");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "Get-ChildItem -Force");
    }

    #[test]
    fn a_row_remembers_its_place_in_the_store() {
        // Filtering changes what is displayed, not what a delete acts on: the row has to
        // carry the stored index or deleting a filtered row would remove the wrong one.
        let rows = filtered(&history(), "echo");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 2);
    }

    #[test]
    fn nothing_matches_is_an_empty_list_rather_than_everything() {
        assert!(filtered(&history(), "no-such-command").is_empty());
    }
}

#[cfg(test)]
mod suggestion_tests {
    use super::*;

    #[test]
    fn suggestions_are_recent_deduplicated_exact_prefixes() {
        let history = ["git old", "git new", "git old", "Git other", "echo git", "git"]
            .map(str::to_owned);
        let rows = suggestions(&history, "git");
        assert_eq!(rows.iter().map(|r| r.command.as_str()).collect::<Vec<_>>(),
            ["git old", "git new"]);
        assert_eq!(rows[0].index, 2);
    }

    #[test]
    fn suggestions_preserve_unicode_and_spaces_without_byte_slicing() {
        let command = "打印 文件😀 e\u{301}".to_owned();
        assert_eq!(suggestions(&[command.clone()], "打印 文")[0].command, command);
        assert!(suggestions(&[" echo hi".into()], "echo").is_empty());
        assert!(suggestions(&["echo hi".into()], " ").is_empty());
    }

    #[test]
    fn suggestions_are_bounded_and_reject_control_characters() {
        let history = (0..20).map(|n| format!("echo {n}")).collect::<Vec<_>>();
        let rows = suggestions(&history, "echo");
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].command, "echo 19");
        assert!(suggestions(&["echo\nunsafe".into(), "echo\u{1b}[31m".into()], "echo").is_empty());
        assert!(suggestions(&["a".repeat(4097)], "a").is_empty());
        assert!(suggestions(&["a".repeat(514)], &"a".repeat(513)).is_empty());
        assert!(suggestions(&["界".repeat(1_000_000)], "界").is_empty());
        assert!(suggestions(&[], &"界".repeat(1_000_000)).is_empty());
        assert!(suggestions(&[], &"\u{3000}".repeat(1_000_000)).is_empty());
    }
}
