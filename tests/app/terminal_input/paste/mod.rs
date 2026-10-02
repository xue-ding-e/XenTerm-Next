use super::super::*;

#[test]
fn paste_normalizes_newlines_to_cr() {
    assert_eq!(
        normalize_pasted_newlines("sudo apt install \\\r\n  docker-ce"),
        "sudo apt install \\\r  docker-ce"
    );
    assert_eq!(normalize_pasted_newlines("a\nb\nc"), "a\rb\rc");
    assert_eq!(normalize_pasted_newlines("a\rb"), "a\rb");
    assert_eq!(normalize_pasted_newlines("echo hi"), "echo hi");
}

#[test]
fn command_bar_preserves_multiline_heredoc() {
    let command = "cat <<'EOF'\nHEREDOC-1\n中文-HEREDOC-2\nEOF\n";
    let (history, bytes) = encode_command_bar_input(command);
    assert_eq!(history.as_deref(), Some(command.trim_end()));
    // The heredoc's own newlines survive — only the submission's terminator is rewritten,
    // because the terminator is the Enter key and a terminal's Enter is CR (a local
    // Windows shell treats LF as "insert a newline" and would sit at its continuation
    // prompt instead of running the command).
    assert_eq!(
        bytes,
        "cat <<'EOF'\nHEREDOC-1\n中文-HEREDOC-2\nEOF\r".as_bytes()
    );
    assert!(!history.unwrap().lines().any(|line| line.starts_with(' ')));
}

#[test]
fn empty_command_bar_submission_sends_enter_without_history() {
    for input in ["", "   ", "\t"] {
        let (history, bytes) = encode_command_bar_input(input);
        assert_eq!(history, None);
        assert_eq!(bytes, b"\r", "a bare submission is one Enter key press");
    }
}

#[test]
fn paste_uses_remote_bracketed_paste_mode() {
    assert_eq!(
        encode_pasted_text("first\r\n  second", true),
        b"\x1b[200~first\r  second\x1b[201~"
    );
    assert_eq!(
        encode_pasted_text("safe\x1b[201~\x03text", true),
        b"\x1b[200~safe[201~text\x1b[201~"
    );
    assert_eq!(
        encode_pasted_text("first\r\nsecond", false),
        b"first\rsecond"
    );
}

#[test]
fn long_pastes_switch_to_large_review() {
    assert!(!paste_requires_large_review("short prompt\nsecond line"));
    assert!(!paste_requires_large_review(&"a".repeat(600)));
    assert!(paste_requires_large_review(&"a".repeat(601)));
    assert!(!paste_requires_large_review(&vec!["line"; 12].join("\r\n")));
    assert!(paste_requires_large_review(&vec!["line"; 13].join("\r\n")));
}
