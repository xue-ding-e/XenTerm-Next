//! What importing `~/.ssh/config` would add, as a rule rather than as a loop in a view.
//!
//! The welcome page and the session list both offer the import, and the interesting part
//! is not the parsing but the decision about which hosts are *new*. Two copies of that
//! decision would eventually disagree about whether a host already saved under a
//! different alias is a duplicate, and the failure is silent: an import that adds the
//! same machine twice.

use crate::config::{AuthMethod, Session};
use crate::ssh::ImportedHost;

/// The sessions an import would add: every parsed host that is not already saved.
///
/// A host counts as already saved when its alias matches a session's name, or when the
/// same user@host pair is saved under some other name. The second rule matters because a
/// user who renamed a session should not get the config's alias for it back on the next
/// import, and the first because renaming the *config's* alias is how a host is removed
/// from the list of things to import.
pub fn sessions_to_add(saved: &[Session], hosts: &[ImportedHost]) -> Vec<Session> {
    let mut added = Vec::new();
    for host in hosts {
        let already = saved.iter().any(|session| {
            session.name == host.alias
                || (session.host == host.hostname && session.user == host.user)
        });
        if already {
            continue;
        }
        // An identity file is what makes this key auth: the config naming one is the
        // user saying which key to use, and a host with no key is a password host. The
        // authentication method is a guess either way, and the editable one is key auth
        // with the path already filled in.
        let auth = if host.identity_file.is_empty() {
            AuthMethod::Password
        } else {
            AuthMethod::Key
        };
        added.push(Session {
            name: host.alias.clone(),
            host: host.hostname.clone(),
            port: host.port,
            // `ssh_config` leaves the user empty when the config does not name one, and
            // every SSH server needs something; `root` is the importer's long-standing
            // default.
            user: if host.user.is_empty() {
                "root".to_string()
            } else {
                host.user.clone()
            },
            auth,
            private_key_path: host.identity_file.clone(),
            ..Session::new_empty()
        });
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str, hostname: &str, user: &str, identity: &str) -> ImportedHost {
        ImportedHost {
            alias: alias.to_string(),
            hostname: hostname.to_string(),
            user: user.to_string(),
            port: 22,
            identity_file: identity.to_string(),
        }
    }

    fn saved(name: &str, host: &str, user: &str) -> Session {
        Session {
            name: name.to_string(),
            host: host.to_string(),
            user: user.to_string(),
            ..Session::new_empty()
        }
    }

    #[test]
    fn a_host_with_nothing_like_it_saved_is_added() {
        let added = sessions_to_add(&[], &[host("web", "web.example.com", "deploy", "")]);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].name, "web");
        assert_eq!(added[0].host, "web.example.com");
        assert_eq!(added[0].user, "deploy");
        assert_eq!(added[0].port, 22);
    }

    #[test]
    fn a_host_already_saved_under_the_same_alias_is_skipped() {
        let saved = [saved("web", "other.example.com", "root")];
        let added = sessions_to_add(&saved, &[host("web", "web.example.com", "root", "")]);
        assert!(added.is_empty(), "the alias is the session's name");
    }

    #[test]
    fn a_host_already_saved_under_another_name_is_skipped_too() {
        // Renaming a session must not bring the config's alias back on the next import:
        // it is the same machine, and the user has already decided what to call it.
        let saved = [saved("production", "web.example.com", "deploy")];
        let added = sessions_to_add(
            &saved,
            &[host(
                "web",
                "web.example.com",
                "deploy",
                "/home/x/.ssh/id_ed25519",
            )],
        );
        assert!(added.is_empty());
    }

    #[test]
    fn the_same_host_with_another_user_is_a_different_session() {
        let saved = [saved("web-root", "web.example.com", "root")];
        let added = sessions_to_add(&saved, &[host("web", "web.example.com", "deploy", "")]);
        assert_eq!(added.len(), 1, "user@host is the identity, not host alone");
    }

    #[test]
    fn a_config_with_an_identity_file_imports_as_key_auth() {
        let added = sessions_to_add(
            &[],
            &[host(
                "web",
                "web.example.com",
                "deploy",
                "/home/x/.ssh/id_ed25519",
            )],
        );
        assert_eq!(added[0].auth, AuthMethod::Key);
        assert_eq!(added[0].private_key_path, "/home/x/.ssh/id_ed25519");
    }

    #[test]
    fn a_user_the_config_does_not_name_becomes_root() {
        let added = sessions_to_add(&[], &[host("web", "web.example.com", "", "")]);
        assert_eq!(added[0].user, "root");
        assert_eq!(added[0].auth, AuthMethod::Password);
    }
}
