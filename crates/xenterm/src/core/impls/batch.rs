//! Batch import: a pasted list of connections, one per line.
//!
//! ## Why this is here and not in the UI
//!
//! The shape of the text — `host|port|user|password|name`, with defaults for everything
//! but the host — is a rule, not a drawing decision, and every window pastes into the same
//! list of sessions. A line one window accepted and another refused would be a line that
//! works until the other window is opened.
//!
//! ## What is deliberately forgiving
//!
//! This is for text that came from somewhere else — a wiki page, a spreadsheet column, a
//! chat message — so it takes what those produce: blank lines and `#` comments are
//! skipped, a header row beginning `host|` is recognised and dropped, everything but the
//! host is optional, and the name field may itself contain `|` because it is last.

use crate::config::{AuthMethod, Secret, Session};

/// Turn pasted text into sessions, in the order they appeared.
///
/// A line with no host is not an error worth reporting: it is a blank line, a comment or
/// a header, and the caller shows the count of what it made rather than a complaint about
/// what it skipped.
pub fn parse(text: &str) -> Vec<Session> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `splitn(5)` so the last field, the name, may itself contain '|'.
        let parts: Vec<&str> = line.splitn(5, '|').map(str::trim).collect();
        let host = parts.first().copied().unwrap_or("");
        // Blank hosts, and a header row like `host|port|username|...`. The
        // hostname gate rejects metacharacters/whitespace so a pasted list
        // cannot smuggle them into saved sessions (audit N-低8).
        if host.is_empty()
            || host.eq_ignore_ascii_case("host")
            || !crate::config::validation::is_valid_hostname(host)
        {
            continue;
        }
        let port = parts
            .get(1)
            .and_then(|part| part.parse::<u16>().ok())
            .filter(|port| *port > 0)
            .unwrap_or(22);
        let user = parts
            .get(2)
            .copied()
            .filter(|part| !part.is_empty())
            .unwrap_or("root");
        let password = parts.get(3).copied().unwrap_or("");
        let name = parts
            .get(4)
            .copied()
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{user}@{host}"));

        let mut session = Session {
            name,
            host: host.to_string(),
            port,
            user: user.to_string(),
            auth: AuthMethod::Password,
            ..Session::new_empty()
        };
        if !password.is_empty() {
            session.password = Secret::new(password.to_string());
        }
        out.push(session);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_line_becomes_a_session() {
        let sessions = parse("db.internal|2222|deploy|hunter2|prod db");
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.host, "db.internal");
        assert_eq!(session.port, 2222);
        assert_eq!(session.user, "deploy");
        assert_eq!(session.password.as_str(), "hunter2");
        assert_eq!(session.name, "prod db");
        assert_eq!(session.auth, AuthMethod::Password);
    }

    #[test]
    fn a_host_alone_is_enough() {
        let sessions = parse("example.com");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].port, 22);
        assert_eq!(sessions[0].user, "root");
        // The name is derived from what there is, so the list is never blank.
        assert_eq!(sessions[0].name, "root@example.com");
        assert!(sessions[0].password.as_str().is_empty());
    }

    #[test]
    fn blanks_comments_and_a_header_are_skipped() {
        let text = "\n# production\ndb|22|root\n\nhost|port|user|password|name\nweb|80|www\n   \n";
        let sessions = parse(text);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].host, "db");
        assert_eq!(sessions[1].host, "web");
    }

    #[test]
    fn a_name_may_contain_the_separator_because_it_is_last() {
        let sessions = parse("db|22|root||prod|primary");
        assert_eq!(sessions[0].name, "prod|primary");
    }

    #[test]
    fn a_port_that_is_not_one_falls_back_rather_than_dropping_the_line() {
        let sessions = parse("db|notaport|root");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].port, 22);
        let zero = parse("db|0|root");
        assert_eq!(zero[0].port, 22);
    }

    #[test]
    fn fields_are_trimmed_and_an_empty_user_is_the_default() {
        let sessions = parse("  db.internal  |  2222  |   |  |  ");
        assert_eq!(sessions[0].host, "db.internal");
        assert_eq!(sessions[0].port, 2222);
        assert_eq!(sessions[0].user, "root");
        assert_eq!(sessions[0].name, "root@db.internal");
    }
}
