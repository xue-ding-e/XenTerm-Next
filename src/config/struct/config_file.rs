
use serde::{Deserialize, Serialize};

use super::{OutputHighlightRule, QuickCommand, Secret, Session};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WslProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub distribution: String,
    #[serde(default = "default_wsl_home")]
    pub directory: String,
}

fn default_wsl_home() -> String {
    "~".to_string()
}

// MCP capabilities default to OFF for configs that predate the fields: an
// external process initiates those calls, so every capability is opt-in. The
// preview builds once defaulted these to true; `migrate_defaults` rev 4 resets
// the stale `true` values those builds wrote (H-04/H-05).
fn default_mcp_preview_enabled() -> bool {
    false
}

fn default_true() -> bool {
    true
}

fn default_approval_timeout() -> u64 {
    120
}

/// The serde twin of [`default_approval_timeout`], used by fields that outlive
/// the config struct itself — the approval request files carry their asker's
/// deadline, and an old file without the field still parses.
pub fn default_approval_timeout_value() -> u64 {
    120
}

fn default_risky_patterns() -> Vec<String> {
    DEFAULT_RISKY_PATTERNS.iter().map(|s| s.to_string()).collect()
}

fn default_risky_dirs() -> Vec<String> {
    DEFAULT_RISKY_DIRS.iter().map(|s| s.to_string()).collect()
}

fn default_audit_retention() -> u64 {
    30
}

/// The risky-command patterns a fresh config ships with, matched
/// case-insensitively as substrings of the command. Published here because
/// this layer owns the defaults; the risk assessor reads them from the config.
pub const DEFAULT_RISKY_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "mkfs",
    "dd if=",
    "dd of=/dev/",
    ":(){:|:&};:",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
    "init 0",
    "init 6",
    "> /dev/sd",
    "> /dev/nvme",
    "chmod -r 777 /",
    "chown -r ",
    "userdel",
    "groupdel",
    "passwd root",
    "crontab -r",
    "iptables -f",
    "ufw disable",
    "systemctl disable",
];

/// The filesystem prefixes a fresh config treats as high risk. The trees the
/// system lives in; a recursive delete or an overwrite aimed here is the
/// classic automation accident.
pub const DEFAULT_RISKY_DIRS: &[&str] = &[
    "/etc", "/boot", "/bin", "/sbin", "/usr", "/lib", "/lib64", "/var", "/sys", "/proc", "/dev",
    "/root", r"C:\Windows", r"C:\Program Files",
];

/// Ships with the "幻想 3048" sci-fi wallpaper on by default (a dark theme). New
/// installs and users upgrading from before the wallpaper feature get it; once
/// the user picks anything (including "无"/none, stored as ""), their choice is
/// saved and sticks.
fn default_wallpaper() -> String {
    // Serde default for the `wallpaper` field: kept at the old "幻想 3048" so an
    // *existing* config that predates the field stays on tech — `migrate_defaults`
    // then advances default-following users through the migration chain. Brand-new
    // installs get the current default straight from `fresh_config`.
    "builtin:tech".to_string()
}

/// Bump when `migrate_defaults` gains a new one-time default-layout change.
pub const DEFAULTS_REV: u32 = 6;

/// The default font of the previous binary, which is no longer embedded. A
/// config naming it is migrated to the empty string — "the new default" — by
/// the defaults migration. The constant lives in this layer because the
/// migration here owns it and config depends on nothing above it.
pub const RETIRED_DEFAULT_FONT: &str = "Meatshell Mono";

/// The proportional faces a previous default made terminal-wide. The terminal
/// is monospace-only — a proportional face in a cell grid drifts against the
/// cursor — so a config naming any of these is reset to the default by the
/// rev 6 migration, regardless of whether the choice was deliberate: the
/// picker refuses the face anyway, and a stored name that renders nothing the
/// picker offers is a trap.
pub const RETIRED_PROPORTIONAL_DEFAULTS: &[&str] = &["MiSans", "HarmonyOS Sans SC"];

pub(crate) const PREVIOUS_DEFAULT_WALLPAPER_TRANSPARENCY: f32 = 0.38;
pub(crate) const PREVIOUS_DEFAULT_WALLPAPER_OVERLAY: f32 =
    1.0 - PREVIOUS_DEFAULT_WALLPAPER_TRANSPARENCY;
pub(crate) const DEFAULT_WALLPAPER_TRANSPARENCY: f32 = 0.15;
pub(crate) const DEFAULT_WALLPAPER_OVERLAY: f32 = 1.0 - DEFAULT_WALLPAPER_TRANSPARENCY;

pub(crate) fn default_sidebar_width() -> f32 {
    220.0
}
pub(crate) fn default_sidebar_height() -> f32 {
    240.0
}
pub(crate) fn default_sftp_width() -> f32 {
    380.0
}
pub(crate) fn default_sftp_height() -> f32 {
    220.0
}
pub(crate) fn default_sftp_tree_width() -> f32 {
    160.0
}

pub(crate) fn default_quick_panel_width() -> f32 {
    260.0
}

pub(crate) fn default_quick_panel_height() -> f32 {
    220.0
}

/// On-disk layout. Keep additive to ease forward-compat.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigFile {
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// User-managed WSL launch entries. An empty list keeps the implicit default
    /// WSL entry for backwards compatibility.
    #[serde(default)]
    pub wsl_profiles: Vec<WslProfile>,
    /// Preset SFTP download directory. Empty = ask each time.
    #[serde(default)]
    pub download_dir: String,
    /// UI language code: "zh" (default) or "en".
    #[serde(default)]
    pub language: String,
    /// Theme preference: "system" (default) | "dark" | "light".
    #[serde(default)]
    pub theme_pref: String,
    /// Platform renderer preference. Windows uses software/auto/gpu; macOS uses
    /// software/femtovg/skia. Missing or foreign-platform values use the platform default.
    #[serde(default)]
    pub renderer_mode: String,
    /// Terminal font family. Empty = the built-in default ("Meatshell Mono").
    #[serde(default)]
    pub font_family: String,
    /// Terminal font size in px. 0 = the built-in default.
    #[serde(default)]
    pub font_size: u32,
    /// Terminal line-height multiplier. 0 means the default 1.0.
    #[serde(default)]
    pub terminal_line_spacing: f32,
    /// Force regular terminal text to render with a bold face (#262).
    #[serde(default)]
    pub terminal_bold: bool,
    /// Inset the terminal grid from its pane's edge by a few pixels, so output
    /// does not sit flush against the frame. On by default; missing/legacy
    /// config keeps it on.
    #[serde(default = "default_true")]
    pub terminal_padding: bool,
    /// Terminal insertion cursor shape: block (default), bar, or underline (#275).
    #[serde(default)]
    pub terminal_cursor_style: String,
    /// Custom terminal cursor colour as #RRGGBB. Empty follows the theme (#275).
    #[serde(default)]
    pub terminal_cursor_color: String,
    /// Stored inverted so missing/legacy config keeps the automatic plain-text
    /// output highlighter enabled by default.
    #[serde(default)]
    pub output_highlight_disabled: bool,
    /// Built-in output highlight preset: "log" (default) or "devops".
    #[serde(default)]
    pub output_highlight_preset: String,
    /// User-defined rules applied before the selected built-in preset.
    #[serde(default)]
    pub output_highlight_rules: Vec<OutputHighlightRule>,
    /// Stored inverted so complete JSON lines are formatted and syntax-coloured
    /// by default while still allowing users to preserve byte-for-byte display.
    #[serde(default)]
    pub json_format_disabled: bool,
    /// Global UI scale in percent (#100). 0 = default (100%).
    #[serde(default)]
    pub ui_scale: u32,
    /// Immersive wallpaper id: "" = none, "builtin:light" / "builtin:dark" /
    /// "builtin:tech", or a filesystem path to a custom image. Drives the
    /// wallpaper + tinted theme. Defaults to the "幻想 3048" built-in.
    #[serde(default = "default_wallpaper")]
    pub wallpaper: String,
    /// Explicit session groups/folders (#41), including empty ones so a folder
    /// can exist before any session is moved into it. "default" is implicit and
    /// not stored here.
    #[serde(default)]
    pub groups: Vec<String>,
    /// Quick Connect folders that were collapsed when the UI was last used.
    /// `None` is a legacy/new config and starts with every folder collapsed;
    /// `Some([])` means the user explicitly expanded every folder.
    #[serde(default)]
    pub collapsed_session_groups: Option<Vec<String>>,
    /// Stored inverted ("don't follow") so both serde and the Default derive
    /// yield `false` = the feature defaults to ON: the SFTP panel follows the
    /// terminal's cd (OSC 7) unless the user opts out in Interface settings.
    #[serde(default)]
    pub sftp_no_follow_cd: bool,
    /// Always prompt for the save location on each download instead of using the
    /// preset download dir. Defaults to false (#87).
    #[serde(default)]
    pub download_always_ask: bool,
    /// Hide the quick-command bar under the terminal. Defaults to false.
    #[serde(default)]
    pub hide_cmd_bar: bool,
    /// Stored inverted so multiline paste confirmation remains enabled for
    /// existing configurations unless the user explicitly disables it (#300).
    #[serde(default)]
    pub paste_confirm_disabled: bool,
    /// Stored inverted so Ctrl+Alt+V, Shift+Insert, and middle-click paste stay
    /// enabled for existing users (#300).
    #[serde(default)]
    pub extra_paste_shortcuts_disabled: bool,
    /// Hide auxiliary panels and edge strips so the terminal fills the window.
    #[serde(default)]
    pub zen_mode: bool,
    /// Saved quick commands (#55).
    #[serde(default)]
    pub quick_commands: Vec<QuickCommand>,
    /// Explicit quick-command group names — mirrors `groups` for sessions so that
    /// empty quick-command groups survive and can be renamed/deleted (#55).
    #[serde(default)]
    pub quick_groups: Vec<String>,
    /// Opt-in docked quick-command sidebar (#215). The command-bar popup remains
    /// available until the user actually drags it into the main dock layer.
    #[serde(default)]
    pub quick_commands_as_sidebar: bool,
    #[serde(default)]
    pub quick_panel_open: bool,
    #[serde(default)]
    pub quick_panel_collapsed: bool,
    #[serde(default = "default_quick_panel_width")]
    pub quick_panel_width: f32,
    #[serde(default = "default_quick_panel_height")]
    pub quick_panel_height: f32,
    #[serde(default)]
    pub quick_panel_dock: String,
    /// Which edge the file panel is docked to: `bottom` (the default) or `right`.
    ///
    /// The panel belongs to the terminal rather than to the window, so those are the only two
    /// edges that mean anything: `left` or `top` would put it between the tab strip and the
    /// output it describes.
    #[serde(default)]
    pub sftp_panel_dock: String,
    /// Recent commands sent from the command box, oldest first, capped (#55).
    #[serde(default)]
    pub command_history: Vec<String>,
    /// Collapse the left resource sidebar on startup (#78).
    #[serde(default)]
    pub collapse_sidebar_default: bool,
    /// Last resource-sidebar collapsed state. None means fall back to
    /// `collapse_sidebar_default` for older configs.
    #[serde(default)]
    pub sidebar_collapsed: Option<bool>,
    /// User-adjustable width of the left resource sidebar, in logical pixels.
    /// Persisted across restarts so the drag-resized width sticks.
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
    /// Resource-panel docking: size when docked top/bottom, and which edge it is
    /// docked to (left|right|top|bottom). Persisted so the layout sticks (#dock).
    #[serde(default = "default_sidebar_height")]
    pub sidebar_height: f32,
    #[serde(default)]
    pub sidebar_dock: String,
    /// SFTP-panel docking: extents (px) and docked edge, persisted (#dock).
    #[serde(default = "default_sftp_width")]
    pub sftp_panel_width: f32,
    #[serde(default = "default_sftp_height")]
    pub sftp_panel_height: f32,
    #[serde(default = "default_sftp_tree_width")]
    pub sftp_tree_width: f32,
    #[serde(default)]
    pub sftp_dock: String,
    /// Last window size in logical px (0 = unset → use the built-in default).
    /// Lets users keep their preferred window size across restarts.
    #[serde(default)]
    pub window_width: f32,
    #[serde(default)]
    pub window_height: f32,
    /// Collapse the bottom SFTP panel on startup (#78).
    #[serde(default)]
    pub collapse_sftp_default: bool,
    /// When session-sync is on, also mirror SFTP uploads to the other online
    /// sessions (same path, falling back to each panel's current dir).
    #[serde(default)]
    pub sync_upload: bool,
    /// WebDAV sync settings (#185). Password is encrypted at rest like session
    /// passwords; remote_path is the JSON export object path under the endpoint.
    #[serde(default)]
    pub webdav_enabled: bool,
    #[serde(default)]
    pub webdav_url: String,
    #[serde(default)]
    pub webdav_username: String,
    #[serde(default)]
    pub webdav_password: Secret,
    #[serde(default)]
    pub webdav_remote_path: String,
    #[serde(default)]
    pub webdav_accept_invalid_certs: bool,
    /// Render the welcome page (session list) as a docked left sidebar instead of
    /// a "New tab" tab (v0.5). Persisted so the layout choice sticks.
    #[serde(default)]
    pub welcome_as_sidebar: bool,
    /// Width (logical px) of the welcome/session sidebar when docked (v0.5).
    #[serde(default)]
    pub welcome_sidebar_width: f32,
    /// Welcome/session sidebar dock edge (left|right|top|bottom).
    #[serde(default)]
    pub welcome_sidebar_dock: String,
    /// Welcome sidebar collapsed to the edge icon strip (IDEA-style) (v0.5).
    /// None means the user has not explicitly collapsed/expanded it yet.
    #[serde(default)]
    pub welcome_collapsed: Option<bool>,
    /// Frosted-panel opacity over a wallpaper (0.30–1.00); user-adjustable via the
    /// Interface › Wallpaper opacity slider. 0 = use the current default.
    #[serde(default)]
    pub wallpaper_overlay: f32,
    /// Settings-panel font scale, percent (80–160). 0 = 100% default (v0.5).
    #[serde(default)]
    pub panel_font: u32,
    /// Disable the startup "new version available" check (#184). Default false =
    /// keep checking (preserves existing behaviour for upgrading users); turning
    /// it on stops the GitHub releases query and the banner.
    #[serde(default)]
    pub update_check_disabled: bool,
    /// Enable the local stdio MCP server (`xenterm mcp serve`).
    #[serde(default = "default_mcp_preview_enabled")]
    pub mcp_enabled: bool,
    /// Allow MCP tools to use credentials already stored by XenTerm. Secrets
    /// remain internal and are never included in protocol responses.
    #[serde(default = "default_mcp_preview_enabled")]
    pub mcp_use_saved_credentials: bool,
    /// Allow MCP clients to execute arbitrary commands on saved SSH sessions.
    #[serde(default = "default_mcp_preview_enabled")]
    pub mcp_allow_commands: bool,
    /// Allow MCP clients to upload local files and download remote files.
    #[serde(default = "default_mcp_preview_enabled")]
    pub mcp_allow_file_transfers: bool,
    /// Require a human at the main window to approve risky commands before an
    /// MCP client may run them. On by default: approval is the safe default
    /// and the whole point of the feature.
    #[serde(default = "default_true")]
    pub mcp_approval_enabled: bool,
    /// Seconds an approval request waits for a human before the command is
    /// refused. A window that is closed counts the same as a timeout.
    #[serde(default = "default_approval_timeout")]
    pub mcp_approval_timeout_secs: u64,
    /// Command patterns (case-insensitive substrings) that trip the approval
    /// gate. Empty disables pattern matching entirely; an old config without
    /// the field deserializes to the published defaults.
    #[serde(default = "default_risky_patterns")]
    pub mcp_risky_patterns: Vec<String>,
    /// Path prefixes whose appearance in a command trips the approval gate.
    /// Empty disables directory matching; old configs default as above.
    #[serde(default = "default_risky_dirs")]
    pub mcp_risky_dirs: Vec<String>,
    /// Audit-journal retention in whole days. The sweep drops entire day
    /// files past this age; there is no single-record deletion.
    #[serde(default = "default_audit_retention")]
    pub mcp_audit_retention_days: u64,
    /// One-time default-layout migration marker (#new-user-defaults). 0 = config
    /// predates the migration. `migrate_defaults` bumps it to `DEFAULTS_REV` after
    /// pushing the new look (default wallpaper / welcome-as-sidebar / right-docked
    /// resource panel / wallpaper overlay) to users still sitting on old defaults.
    #[serde(default)]
    pub defaults_rev: u32,
}

/// Portable export file (issue #46): sessions with everything in plaintext
/// **except** the password, which is encrypted with a fixed key baked into the
/// binary so the file opens on *any* machine running xenterm.
///
/// Security note: a built-in key in open-source code is **obfuscation, not real
/// security** — anyone with the source can derive it. It only stops a casual
/// over-the-shoulder read of the file, same level as FinalShell's export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExportFile {
    /// Format marker / version so the schema can evolve later.
    // The JSON key keeps its pre-rename name: portable export files in the
    // wild carry it, and serde matches fields by name when importing.
    pub(crate) meatshell_export: u32,
    pub(crate) sessions: Vec<Session>,
}
