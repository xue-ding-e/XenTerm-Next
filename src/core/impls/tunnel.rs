//! Tunnels: the shape of a forwarding rule and the rules for a valid one.
//!
//! ## Why this is here and not in either frontend
//!
//! Both windows edit the same list of forwards, and a rule that one accepts and the
//! other refuses would be a rule that works until the window is reopened. The
//! validation is the same three sentences in both — a listen port, and for everything
//! but a SOCKS proxy a target host and port — so it lives once, with its tests.
//!
//! ## The drafts are strings
//!
//! A form holds text, including text that is not a port yet. `TunnelDraft` keeps what
//! was typed and [`validate`] is what turns it into the typed `PortForward` the SSH
//! layer takes, reporting the first thing that is wrong in the words the user needs.

use crate::config::PortForward;

/// One row of the tunnel form, as it is being typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunnelDraft {
    /// `"local"`, `"remote"` or `"dynamic"`.
    pub kind: String,
    pub name: String,
    /// Empty means `127.0.0.1`.
    pub bind_addr: String,
    pub bind_port: String,
    /// Empty for a dynamic (SOCKS) forward, which has no single target.
    pub host: String,
    pub host_port: String,
}

/// A new rule's starting values: a local forward on the loopback address.
pub fn blank_draft() -> TunnelDraft {
    TunnelDraft {
        kind: "local".into(),
        name: String::new(),
        bind_addr: "127.0.0.1".into(),
        bind_port: String::new(),
        host: String::new(),
        host_port: String::new(),
    }
}

/// The form rows for a saved list.
pub fn drafts(forwards: &[PortForward]) -> Vec<TunnelDraft> {
    forwards
        .iter()
        .map(|forward| TunnelDraft {
            kind: forward.kind.clone(),
            name: forward.name.clone(),
            bind_addr: if forward.bind_addr.trim().is_empty() {
                "127.0.0.1".into()
            } else {
                forward.bind_addr.trim().into()
            },
            bind_port: forward.bind_port.to_string(),
            host: forward.host.clone(),
            // A dynamic rule has no target, and a `0` left in the field would be read as
            // one: the original clears it, and so does this.
            host_port: if forward.kind == "dynamic" {
                String::new()
            } else {
                forward.host_port.to_string()
            },
        })
        .collect()
}

/// Turn the form rows into rules, or say what is wrong with the first bad one.
///
/// A row with nothing in it at all is skipped rather than refused: an empty row is the
/// form's own spare line, not a rule the user meant to write.
pub fn validate(drafts: &[TunnelDraft]) -> Result<Vec<PortForward>, String> {
    let mut forwards = Vec::new();
    for draft in drafts {
        let is_blank = draft.name.trim().is_empty()
            && draft.bind_port.trim().is_empty()
            && draft.host.trim().is_empty()
            && draft.host_port.trim().is_empty();
        if is_blank {
            continue;
        }

        let bind_port = parse_port(&draft.bind_port).ok_or_else(|| {
            crate::i18n::t(
                "请输入有效的监听端口（1-65535）",
                "Enter a valid listen port (1-65535).",
            )
            .to_string()
        })?;
        let kind = draft.kind.as_str();
        let (host, host_port) = if kind == "dynamic" {
            (String::new(), 0)
        } else {
            let host = draft.host.trim();
            let port = parse_port(&draft.host_port);
            match (host.is_empty(), port) {
                (false, Some(port)) => (host.to_string(), port),
                _ => {
                    return Err(crate::i18n::t(
                        "请输入目标主机和有效的目标端口（1-65535）",
                        "Enter a target host and a valid target port (1-65535).",
                    )
                    .to_string());
                }
            }
        };

        forwards.push(PortForward {
            kind: kind.to_string(),
            name: draft.name.trim().to_string(),
            bind_addr: if draft.bind_addr.trim().is_empty() {
                "127.0.0.1".to_string()
            } else {
                draft.bind_addr.trim().to_string()
            },
            bind_port,
            host,
            host_port,
        });
    }
    Ok(forwards)
}

/// The two sides of a rule as one line each, for a list that has no room for five
/// columns: `127.0.0.1:8080` and `db.internal:5432`, or `SOCKS5` for a dynamic rule.
///
/// Shared because both frontends write the same sentence in the same words, and a
/// tunnel list that described the same rule two ways would be two lists.
pub fn describe(forward: &PortForward) -> (String, String) {
    let bind = format!("{}:{}", forward.bind_addr, forward.bind_port);
    let target = if forward.kind == "dynamic" {
        "SOCKS5".to_string()
    } else if forward.host.is_empty() || forward.host_port == 0 {
        String::new()
    } else {
        format!("{}:{}", forward.host, forward.host_port)
    };
    (bind, target)
}

fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|port| *port > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(kind: &str, bind_port: &str, host: &str, host_port: &str) -> TunnelDraft {
        TunnelDraft {
            kind: kind.into(),
            name: "db".into(),
            bind_addr: "127.0.0.1".into(),
            bind_port: bind_port.into(),
            host: host.into(),
            host_port: host_port.into(),
        }
    }

    #[test]
    fn an_empty_row_is_skipped_and_a_filled_one_becomes_a_rule() {
        // Nothing at all in the row, name included: that is the form's spare line.
        let empty = TunnelDraft::default();
        let rules = validate(&[empty, draft("local", "8080", "db", "5432")])
            .expect("a rule with a listen port and a target is valid");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].bind_port, 8080);
        assert_eq!(rules[0].host, "db");
        assert_eq!(rules[0].host_port, 5432);
        assert_eq!(rules[0].bind_addr, "127.0.0.1");
    }

    #[test]
    fn a_listen_port_is_required_and_bounded() {
        for bad in ["", "0", "70000", "eighty"] {
            let error = validate(&[draft("local", bad, "db", "5432")])
                .expect_err("a rule without a usable listen port is refused");
            assert!(error.contains("监听端口"), "unexpected message: {error}");
        }
    }

    #[test]
    fn a_target_is_required_unless_the_rule_is_dynamic() {
        let error = validate(&[draft("local", "8080", "", "5432")])
            .expect_err("a local forward needs somewhere to forward to");
        assert!(error.contains("目标主机"), "unexpected message: {error}");

        // Dynamic needs neither, and its target is left empty rather than zeroed.
        let rules = validate(&[draft("dynamic", "1080", "", "")]).expect("a SOCKS rule is valid");
        assert_eq!(rules[0].kind, "dynamic");
        assert!(rules[0].host.is_empty());
        assert_eq!(rules[0].host_port, 0);
    }

    #[test]
    fn drafts_round_trip_through_the_form() {
        let rules = validate(&[draft("remote", "9000", "10.0.0.5", "22")]).unwrap();
        let back = drafts(&rules);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].kind, "remote");
        assert_eq!(back[0].bind_port, "9000");
        assert_eq!(back[0].host_port, "22");

        // A dynamic rule's target field comes back empty rather than as "0".
        let dynamic = drafts(&validate(&[draft("dynamic", "1080", "", "")]).unwrap());
        assert_eq!(dynamic[0].host_port, "");
    }

    #[test]
    fn a_rule_is_described_by_its_two_ends() {
        let local = validate(&[draft("local", "8080", "db.internal", "5432")]).unwrap();
        let (bind, target) = describe(&local[0]);
        assert_eq!(bind, "127.0.0.1:8080");
        assert_eq!(target, "db.internal:5432");

        let dynamic = validate(&[draft("dynamic", "1080", "", "")]).unwrap();
        let (bind, target) = describe(&dynamic[0]);
        assert_eq!(bind, "127.0.0.1:1080");
        assert_eq!(target, "SOCKS5");
    }
}
