//! Structural guards, asserted instead of assumed.
//!
//! The layering this crate is written in used to live only in prose: `src/core`
//! said "nothing in here names a toolkit", and nothing checked it. A claim that
//! is never checked is a fact about the day it was written — the same argument
//! that turned "the interface uses no emoji" from a habit into a test in
//! `crate::ui`.
//!
//! Each guard below is a negative claim about where a name may appear. They are
//! deliberately cheap: they read the source tree as text, so they need no
//! compiler, no toolkit and no running window, and they fail with the offending
//! `file:line` rather than a type error somewhere far away.
//!
//! These are guards, not a design. When one fires, the fix is either to move the
//! code back to its own side of the line or to narrow the guard and say in the
//! comment why the exception is real.

#![cfg(test)]

use std::path::{Path, PathBuf};

/// One source file, with its path relative to the crate root.
struct Source {
    /// Path as the assertion messages should print it: `src/core/mod.rs`.
    display: String,
    text: String,
}

impl Source {
    /// Every line of the file that contains `needle`, as `path:line: text`.
    ///
    /// A line that is entirely a comment is skipped: prose is allowed to *mention*
    /// a layer ("`crate::core::ssh_import` owns that rule") without depending on
    /// it, and a guard that fires on documentation is a guard people learn to
    /// ignore. Nothing else is filtered — a name in a signature, a body or an
    /// attribute all count.
    fn hits(&self, needle: &str) -> Vec<String> {
        self.text
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains(needle))
            .filter(|(_, line)| !line.trim_start().starts_with("//"))
            .map(|(index, line)| format!("{}:{}: {}", self.display, index + 1, line.trim()))
            .collect()
    }

    /// Every line containing `needle`, comments included.
    fn all_hits(&self, needle: &str) -> Vec<String> {
        self.text
            .lines()
            .enumerate()
            .filter(|(_, line)| contains_word(line, needle))
            .map(|(index, line)| format!("{}:{}: {}", self.display, index + 1, line.trim()))
            .collect()
    }
}

/// Does `line` contain `needle` as a word, ignoring case?
///
/// Ignoring case is the point: the first version of this guard used a
/// case-sensitive `contains`, so prose that said "Slint" or "GPUI" never failed it
/// and a whole tree's worth of comments went on describing a toolkit that had been
/// deleted. A guard that only catches the lowercase spelling catches the spelling
/// nobody writes.
///
/// The word boundaries are the other half. `LocalGpuInfo` contains "gpu" but is an
/// application type, not a toolkit type, and the guard is about to be trusted to
/// tell those apart rather than be narrowed until it stops looking. So a hit must
/// start and end at an identifier boundary.
fn contains_word(line: &str, needle: &str) -> bool {
    let line = line.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut from = 0;
    while let Some(offset) = line[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        let free = |c: Option<char>| c.map_or(true, |c| !c.is_alphanumeric() && c != '_');
        if free(before) && free(after) {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Read every `.rs` file under `src/`, in a stable order.
fn sources() -> Vec<Source> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<Source>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|error| {
            panic!("the source tree is readable: {}: {error}", dir.display())
        });
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let display = path
                    .strip_prefix(root)
                    .expect("a path under the crate root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("{display} is readable: {error}"));
                out.push(Source { display, text });
            }
        }
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("src");
    let mut out = Vec::new();
    walk(&src, &root, &mut out);
    assert!(!out.is_empty(), "no sources found under {}", src.display());
    out.sort_by(|a, b| a.display.cmp(&b.display));
    out
}

/// The files whose path starts with `src/<module>/` or is `src/<module>.rs`.
///
/// This mirrors how the crate is wired: a module's files live either in its own
/// directory or beside the module root, which is why the guard has to know both
/// spellings rather than only looking inside the directory.
fn module_files<'a>(sources: &'a [Source], module: &str) -> Vec<&'a Source> {
    let directory = format!("src/{module}/");
    let root = format!("src/{module}.rs");
    sources
        .iter()
        .filter(|source| source.display.starts_with(&directory) || source.display == root)
        .collect()
}

/// Assert that no file in `module` contains `needle`.
fn assert_absent(module: &str, needle: &str, why: &str) {
    let sources = sources();
    let files = module_files(&sources, module);
    assert!(
        !files.is_empty(),
        "guard for `{module}` is scanning nothing — the module moved or was renamed, \
         so this guard is now checking an empty set"
    );

    let hits: Vec<String> = files
        .iter()
        .flat_map(|source| source.hits(needle))
        .collect();
    assert!(
        hits.is_empty(),
        "{why}\n`{needle}` found in src/{module}:\n{}",
        hits.join("\n")
    );
}

/// The names of whatever draws the window, which must be absent everywhere below
/// the shell — including in prose.
///
/// This is the one guard that scans comments too, and for the opposite reason the
/// others skip them: a comment in `src/terminal` that says "the view takes its
/// colour from its own theme" is a reader being told to look at a file that does
/// not exist, which is how a tree teaches its own history as if it were the
/// present.
///
/// The list names a dependency rather than a concept because that is what the
/// layers are actually free of: they may describe a window, a view or a repaint
/// in their own words, so long as no line makes the crate above them a
/// compile-time dependency. Keeping the entry current when the toolkit is
/// replaced is the price of the guard being checkable at all; a guard that named
/// no dependency could not fail.
fn assert_no_ui_dependency_mention(module: &str) {
    let sources = sources();
    let files = module_files(&sources, module);
    let hits: Vec<String> = files
        .iter()
        .flat_map(|source| {
            ["gpui", "slint"]
                .iter()
                .flat_map(|needle| source.all_hits(needle))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        hits.is_empty(),
        "src/{module} is below the shell and must not name the UI toolkit, in code or in \
         a comment that points a reader at the toolkit's side of the tree:\n{}",
        hits.join("\n")
    );
}

/// The modules that must not name the toolkit that draws the window.
///
/// `core` and `terminal` are the two layers a toolkit swap has to leave alone:
/// the first is the application's own state, the second is the screen model the
/// renderer paints. `config`, `session`, `ssh`, `sftp`, `tunnel`, `resource`,
/// `layout` and `logging` sit under both, and the CLI and the MCP server build
/// against them with no window in existence — so a toolkit name in any of them
/// is a build that needs a UI to compile.
const TOOLKIT_FREE: &[&str] = &[
    "config", "core", "layout", "logging", "resource", "session", "sftp", "ssh", "terminal",
    "tunnel",
];

/// No toolkit in the framework-agnostic layers.
///
/// This is the guard for `src/core`'s own module doc: everything below the shell
/// has to compile and be testable without a UI toolkit in scope.
#[test]
fn the_lower_layers_name_no_ui_toolkit() {
    for module in TOOLKIT_FREE {
        assert_no_ui_dependency_mention(module);
    }
}

/// The session layer must not reach up into the UI, and must not depend on the
/// pump side either.
///
/// `crate::session` owns the connect path: the four session kinds, the prompt
/// queues and the policy over them. It is the layer both a window and a
/// windowless entry point (CLI, MCP) use, so a `crate::ui` name in it would be a
/// session that cannot be started without a window.
///
/// `crate::app` was banned after `ConnectCtx` stopped naming
/// `crate::app::core::TabRoutes`: the tab route moved to `crate::session::protocol`
/// with the rest of the session vocabulary, which is what turned
/// `session → app → session` into two downward edges into one leaf.
#[test]
fn the_session_layer_does_not_reach_up_into_the_shell_or_the_pump() {
    for module in ["crate::ui", "crate::app"] {
        assert_absent(
            "session",
            module,
            "the session layer names a layer above it; a session must be startable \
             with no window and without the pump's plumbing",
        );
    }
}

/// The pump side must not reach up into the UI.
///
/// `crate::app` is what a session's pump threads run on top of. It delivers
/// through `crate::core::EventSink`, which is the whole point of that trait: the
/// pump says "here is a batch" and never learns who is drawing it.
#[test]
fn the_pump_side_does_not_reach_into_the_ui() {
    assert_absent(
        "app",
        "crate::ui",
        "the pump side names the UI shell; delivery goes through core::EventSink",
    );
}

/// The session protocol is a leaf, and that is the whole point of it.
///
/// It is the vocabulary every session implementation speaks — SSH, local PTY,
/// telnet, serial, the SFTP client, the tunnel forwarder — so it cannot be owned
/// by any one of them. When `SessionEvent` and friends lived in `crate::ssh`,
/// every one of those layers had to depend on the SSH module to name a message.
///
/// The list below is what the module is allowed to reach *down* to. Reaching back
/// up to an implementation would recreate the cycle the move removed, so it is
/// asserted rather than trusted.
#[test]
fn the_session_protocol_is_a_leaf() {
    for module in [
        "crate::ssh",
        "crate::ui",
        "crate::app",
        "crate::session",
        "crate::automation",
        "crate::cli",
        "crate::mcp",
        "crate::resource",
    ] {
        assert_absent(
            "session/protocol",
            module,
            "the session protocol reaches up into an implementation or a consumer; \
             it is the vocabulary they share and must stay a leaf",
        );
    }
}

/// `crate::config` is the bottom of the tree.
///
/// It is read by both entry points and by the session pump, and it is the one
/// module whose types appear in the on-disk format. Anything above it can change
/// without touching a saved file; a dependency out of it would put that the other
/// way round.
#[test]
fn the_config_layer_depends_on_nothing_above_it() {
    for module in ["crate::core", "crate::ui", "crate::app", "crate::ssh", "crate::sftp"] {
        assert_absent(
            "config",
            module,
            "config is the bottom layer and must not depend on a layer above it",
        );
    }
}

/// Nobody reaches the session protocol through the SSH module any more.
///
/// The protocol used to live in `src/ssh/struct`, so every consumer — the
/// terminal kinds, the SFTP client, the tunnel forwarder, the pumps, the views —
/// named `crate::ssh::…` to talk about a session. It now lives in
/// `crate::session::protocol`, and for one commit a re-export kept the old
/// spellings working while the call sites were repointed. That re-export is gone;
/// this test is what keeps it gone, because a convenience alias is exactly the
/// kind of thing that grows back.
///
/// The names are assembled at run time rather than written out, so the guard does
/// not find itself.
#[test]
fn the_session_protocol_is_not_reached_through_the_ssh_module() {
    const NAMES: &[&str] = &[
        "SessionEvent",
        "SessionCommand",
        "SessionHandle",
        "ProcessKillResult",
        "HostKeyResponder",
        "CredentialResponder",
        "MfaResponder",
        "CredentialReply",
        "RemoteEntry",
        "RemoteTreeNode",
        "ProcInfo",
        "SystemDetails",
        "RuntimeTunnelInfo",
    ];

    let sources = sources();
    let mut hits = Vec::new();
    for name in NAMES {
        let needle = format!("{}::{}::{}", "crate", "ssh", name);
        for source in &sources {
            hits.extend(source.hits(&needle));
        }
    }
    assert!(
        hits.is_empty(),
        "the session protocol is reached through `crate::ssh`, which owns it no longer:\n{}",
        hits.join("\n")
    );
}

/// The toolkit guard has to catch both spellings, and only the spellings.
///
/// This is the test for the guard's own rule, because that rule is the one thing
/// that makes all ten of its assertions mean anything: the case-sensitive version
/// it replaced passed over every capitalized mention, which is the spelling prose
/// actually uses.
#[test]
fn the_toolkit_guard_sees_both_cases_and_only_whole_words() {
    // Caught: the spellings a comment would use.
    assert!(contains_word("// the Slint window drew it", "slint"));
    assert!(contains_word("// GPUI's hit test", "gpui"));
    assert!(contains_word("SLINT", "slint"));
    assert!(contains_word("// a gpui-kit type", "gpui"));

    // Not caught: an application type that merely contains the letters.
    assert!(!contains_word("pub(crate) gpus: Vec<LocalGpuInfo>,", "gpui"));
    assert!(!contains_word("struct LocalGpuInfo {", "gpui"));
    assert!(!contains_word("// the toolkit is not named here", "slint"));
}
