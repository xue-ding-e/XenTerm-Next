//! Quick commands, grouped the way the panel shows them.
//!
//! The grouping is a rule rather than a drawing: the implicit "default" group first,
//! then named groups alphabetically, with each group's entries keeping their saved order.
//! The command bar's dock and manage dialog both show that list, and deriving it twice
//! would mean two answers to "which group does an entry with an empty name belong to".
//!
//! The functions take the pieces rather than the store, so the rule can be tested without
//! a configuration file and without the loading that reading one implies.
//!
//! What is *not* here is which groups are folded. That is a view's own state, it is not
//! persisted, and folding a group in one window should not fold it in another.

use crate::config::QuickCommand;

/// The name the implicit group is displayed under: entries whose `group` is empty.
///
/// A name rather than an empty header, because a header that draws nothing is a group
/// nobody can fold, rename or see the extent of.
pub const DEFAULT_GROUP: &str = "default";

/// One row of the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct QuickRow {
    pub name: String,
    /// What clicking it sends (or drops into the command bar, when `send_enter` is off).
    pub command: String,
    /// The group this row belongs to, as displayed.
    pub group: String,
    /// Whether this row is the first of its group, and therefore draws the header.
    pub header: bool,
    /// Whether clicking it also sends Return.
    pub send_enter: bool,
    /// Where this entry sits in the store's own vec.
    ///
    /// `None` for the placeholder row of a group with no entries: such a group still
    /// needs a header — so it can be renamed or deleted, and so an empty group is not
    /// indistinguishable from a group that does not exist — and there is no entry for a
    /// click to edit.
    pub index: Option<usize>,
}

/// The group names, in display order: the implicit one first, the rest alphabetically.
///
/// An explicit group with no entries keeps its header. A group is only "gone" when it is
/// neither registered nor referenced by an entry.
pub fn group_names(commands: &[QuickCommand], groups: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // Only when something actually uses it: an empty "default" header on a machine with
    // no default entries would be a group the user never made.
    if commands
        .iter()
        .any(|command| command.group.trim().is_empty())
    {
        names.push(DEFAULT_GROUP.to_string());
    }
    let mut named: Vec<String> = groups
        .iter()
        .cloned()
        .chain(
            commands
                .iter()
                .map(|command| command.group.trim().to_string())
                .filter(|group| !group.is_empty()),
        )
        .collect();
    // Case-insensitive, because "Build" and "build" are two spellings a user reads as one
    // word and would not expect to find in different places.
    named.sort_by_key(|group| group.to_lowercase());
    named.dedup();
    names.extend(named);
    names
}

/// The panel's rows, in display order.
pub fn rows(commands: &[QuickCommand], groups: &[String]) -> Vec<QuickRow> {
    let mut rows = Vec::new();
    for group in group_names(commands, groups) {
        let members: Vec<(usize, &QuickCommand)> = commands
            .iter()
            .enumerate()
            .filter(|(_, command)| {
                let own = command.group.trim();
                if group == DEFAULT_GROUP {
                    own.is_empty()
                } else {
                    own == group
                }
            })
            .collect();

        if members.is_empty() {
            rows.push(QuickRow {
                name: String::new(),
                command: String::new(),
                group: group.clone(),
                header: true,
                send_enter: true,
                index: None,
            });
            continue;
        }
        for (position, (index, command)) in members.into_iter().enumerate() {
            rows.push(QuickRow {
                name: command.name.clone(),
                command: command.command.clone(),
                group: group.clone(),
                header: position == 0,
                send_enter: command.send_enter,
                index: Some(index),
            });
        }
    }
    rows
}

/// Move an entry one place within its own group, and return its new index.
///
/// Within its own group, not within the store: the panel shows groups, so "up" means up
/// among the entries the user can see, and swapping with an entry that is displayed in a
/// different group would look like nothing happening. The entries live in one vec in
/// display-group order, so a move is a swap with the nearest member of the same group.
pub fn reorder(commands: &mut [QuickCommand], index: usize, move_up: bool) -> Option<usize> {
    let current = commands.get(index)?;
    let group = current.group.trim().to_string();
    let target = if move_up {
        (0..index)
            .rev()
            .find(|&candidate| commands[candidate].group.trim() == group)
    } else {
        (index + 1..commands.len()).find(|&candidate| commands[candidate].group.trim() == group)
    };
    if let Some(target) = target {
        commands.swap(index, target);
        Some(target)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, group: &str) -> QuickCommand {
        QuickCommand {
            name: name.to_string(),
            command: format!("run {name}"),
            group: group.to_string(),
            send_enter: true,
        }
    }

    #[test]
    fn the_implicit_group_comes_first_and_the_rest_alphabetically() {
        let commands = [
            command("ls", ""),
            command("deploy", "zeta"),
            command("build", "alpha"),
        ];
        assert_eq!(
            group_names(&commands, &[]),
            ["default", "alpha", "zeta"],
            "default is not a name to sort, it is the group with no name"
        );
    }

    #[test]
    fn a_registered_group_with_no_entries_still_has_a_name() {
        let names = group_names(&[], &["empty".to_string()]);
        assert_eq!(names, ["empty"]);
    }

    #[test]
    fn entries_keep_their_saved_order_within_a_group() {
        let commands = [command("second", "g"), command("first", "g")];
        let shown = rows(&commands, &[]);
        let names: Vec<&str> = shown.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(
            names,
            ["second", "first"],
            "the panel is a list the user arranged, not an alphabetical index"
        );
    }

    #[test]
    fn a_header_is_drawn_once_per_group_and_carries_the_group_name() {
        let commands = [command("a", "g"), command("b", "g")];
        let rows = rows(&commands, &[]);
        assert!(rows[0].header, "the first row of a group draws its header");
        assert!(!rows[1].header, "the second row must not draw it again");
        assert!(rows.iter().all(|row| row.group == "g"));
    }

    #[test]
    fn an_empty_group_still_gets_a_row_for_its_header() {
        let rows = rows(&[], &["empty".to_string()]);
        assert_eq!(rows.len(), 1, "the header is the group's only row");
        assert!(rows[0].header);
        assert_eq!(
            rows[0].index, None,
            "there is no entry for a click to edit or send"
        );
    }

    #[test]
    fn an_entry_remembers_where_it_sits_in_the_store() {
        // The display order is derived; an edit or a delete acts on the stored order, so
        // the row has to carry that index rather than its own position.
        let commands = [command("later", "zzz"), command("earlier", "aaa")];
        let rows = rows(&commands, &[]);
        let earlier = rows
            .iter()
            .find(|row| row.name == "earlier")
            .expect("both rows are shown");
        assert_eq!(earlier.index, Some(1));
    }

    #[test]
    fn a_move_stays_inside_the_group_it_is_displayed_in() {
        let mut commands = [
            command("a", "ops"),
            command("x", "other"),
            command("b", "ops"),
        ];
        assert_eq!(reorder(&mut commands, 2, true), Some(0));
        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["b", "x", "a"],
            "b swapped with a, not with the entry displayed in another group"
        );
        assert!(
            reorder(&mut commands, 0, true).is_none(),
            "the first entry of a group has nowhere to go"
        );
        assert_eq!(reorder(&mut commands, 0, false), Some(2));
        assert_eq!(reorder(&mut commands, 2, false), None);
        assert_eq!(reorder(&mut commands, 3, true), None);
    }
}
