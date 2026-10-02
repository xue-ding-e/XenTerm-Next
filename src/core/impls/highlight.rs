//! What makes a custom output-highlighting rule valid, as a rule of its own.
//!
//! The settings page lets a user add one, and the interesting part is the refusal: an
//! empty pattern matches everything, a 600-character one is a typo nobody can read, and
//! an invalid regular expression is a rule that would silently do nothing at all. The
//! messages are the interface here, so they live with the check rather than in a view.

use crate::i18n::t;

/// How many custom rules a machine may keep.
///
/// A cap rather than none, because every rule is run against every line the terminal
/// draws: a thousand of them is a terminal that stutters, and the user who wants a
/// thousand patterns wants a different feature.
pub const MAX_RULES: usize = 128;

/// How long a single pattern may be.
pub const MAX_PATTERN_CHARS: usize = 512;

/// Whether this rule can be saved, and what to say when it cannot.
pub fn validate(pattern: &str, is_regex: bool, case_sensitive: bool) -> Result<(), String> {
    if pattern.is_empty() {
        return Err(t(
            "请输入关键词或正则表达式",
            "Enter a keyword or regular expression",
        )
        .to_string());
    }
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(t(
            "规则不能超过 512 个字符",
            "Rules cannot exceed 512 characters",
        )
        .to_string());
    }
    if is_regex {
        // The same builder the matcher uses, so a pattern that validates is a pattern
        // that compiles — and with the same case-sensitivity flag, because `(?i)` in one
        // and not the other is a rule that matches something other than what was tested.
        regex::RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|error| {
                format!(
                    "{}: {error}",
                    t("无效的正则表达式", "Invalid regular expression")
                )
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_pattern_is_refused_with_a_message_that_says_what_to_do() {
        let error = validate("", false, false).expect_err("an empty pattern matches everything");
        assert!(!error.is_empty());
    }

    #[test]
    fn a_plain_keyword_needs_nothing_else() {
        assert!(validate("ERROR", false, false).is_ok());
        // Even one that would not compile as a regex, which is the point of the flag:
        // `(unclosed` is a perfectly good thing to look for in a line of output.
        assert!(validate("(unclosed", false, false).is_ok());
    }

    #[test]
    fn a_broken_regular_expression_is_refused() {
        let error = validate("(unclosed", true, false).expect_err("this cannot compile");
        assert!(!error.is_empty());
        assert!(validate("ready$", true, false).is_ok());
    }

    #[test]
    fn case_sensitivity_is_part_of_what_is_checked() {
        // Validated with the flag it will be compiled with: a pattern that only compiles
        // case-insensitively is not a pattern this rule can use.
        assert!(validate("[a-z]", true, true).is_ok());
        assert!(validate("(?i)error", true, false).is_ok());
    }

    #[test]
    fn an_over_long_pattern_is_refused() {
        let long = "x".repeat(MAX_PATTERN_CHARS + 1);
        assert!(validate(&long, false, false).is_err());
        let limit = "x".repeat(MAX_PATTERN_CHARS);
        assert!(validate(&limit, false, false).is_ok());
    }
}
