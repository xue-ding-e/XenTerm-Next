// Entry point. Wires the UI shell to the config store, system sampler and
// SSH session manager.

#![cfg_attr(
    all(not(debug_assertions), feature = "desktop"),
    windows_subsystem = "windows"
)]

mod allocator;

#[global_allocator]
static GLOBAL: allocator::Allocator = allocator::Allocator;
#[cfg(feature = "desktop")]
mod app;
// The layering rules, asserted rather than assumed. Test-only: the module is
// nothing but `#[cfg(test)]` readers of this source tree.
#[cfg(test)]
mod arch_guards;
mod automation;
mod cli;
mod config;
mod core;
mod i18n;
#[cfg(feature = "desktop")]
mod layout;
mod logging;
mod mcp;
mod resource;
mod session;
mod sftp;
mod ssh;
mod terminal;
mod tunnel;
#[cfg(feature = "desktop")]
mod ui;
mod webdav;

enum StartMode {
    Mcp,
    Cli,
    /// The UI shell, which is the whole UI. The explicit `gpui` argument from
    /// the migration window is still accepted so existing shortcuts, jump-list
    /// tasks and scripts keep working.
    Ui,
    Version,
}

impl StartMode {
    fn detect(args: &[String]) -> Self {
        match args.get(1).map(String::as_str) {
            Some("mcp") if args.get(2).is_some_and(|arg| arg == "serve") => Self::Mcp,
            Some("cli") => Self::Cli,
            Some("gpui") => Self::Ui,
            _ if args.iter().any(|arg| arg == "--version" || arg == "-V") => Self::Version,
            _ => Self::Ui,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    config::configure_profile(&mut args)?;
    let mode = StartMode::detect(&args);
    if matches!(mode, StartMode::Version) {
        println!("xenterm {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    #[cfg(not(feature = "desktop"))]
    anyhow::ensure!(config::has_explicit_data_dir(),
        "headless CLI/MCP requires --data-dir <independent-service-profile> or XENTERM_DATA_DIR; import an export into that profile instead of opening the default desktop profile");

    if args.get(1).is_some_and(|a| a == "--config-info") {
        anyhow::ensure!(
            args.len() == 2,
            "--config-info does not accept command arguments"
        );
        let store = config::ConfigStore::load()?;
        println!(
            "{}",
            serde_json::json!({
                "executable": std::env::current_exe()?, "version": env!("CARGO_PKG_VERSION"),
                "data_dir": config::data_dir(), "session_count": store.sessions().len()
            })
        );
        return Ok(());
    }

    // Refuse before initializing file logging: a service launch must not even
    // touch the default desktop profile when an explicit profile is missing.
    if matches!(mode, StartMode::Mcp) && args.iter().any(|arg| arg == "--http-config") {
        anyhow::ensure!(
            config::has_explicit_data_dir(),
            "HTTP service requires an explicitly selected --data-dir or XENTERM_DATA_DIR profile"
        );
    }
    init_tracing();

    match mode {
        StartMode::Mcp => mcp::run(&args),
        StartMode::Cli => cli::run(&args),
        #[cfg(feature = "desktop")]
        StartMode::Ui => ui::run(),
        #[cfg(not(feature = "desktop"))]
        StartMode::Ui => anyhow::bail!("headless build: use xenterm cli help or xenterm mcp serve"),
        StartMode::Version => unreachable!("handled above"),
    }
}

/// Set up tracing: stderr (honours RUST_LOG, default info) **plus** a capped
/// `error.log` file at WARN and above so users can send diagnostics — e.g. a
/// bastion disconnect reason — without setting RUST_LOG (#86).
fn init_tracing() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    // Third-party noise routed through `log` → tracing: ICU4X data-error warnings
    // (icu_provider dependency) and fontdb's "malformed font" warning for fonts it
    // can't parse but harmlessly skips (e.g. Windows' mstmc.ttf). Silence on every
    // layer; keep fontdb at `error` so genuine failures still surface.
    fn quiet_noise(mut f: EnvFilter) -> EnvFilter {
        for d in [
            "icu_provider=off",
            "icu_segmenter=off",
            "icu_normalizer=off",
            "fontdb=error",
        ] {
            if let Ok(dir) = d.parse() {
                f = f.add_directive(dir);
            }
        }
        f
    }

    let env_filter =
        quiet_noise(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")));
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(env_filter);

    // One file, capped at 50 MiB, auto-overwriting when full (5 MiB was too
    // small to diagnose anything useful).
    let file_layer = logging::path()
        .and_then(|p| logging::CappedFile::open(p, 50 * 1024 * 1024).ok())
        .map(|cf| {
            fmt::layer()
                .with_ansi(false)
                .with_writer(logging::CappedWriter::new(cf))
                .with_filter(quiet_noise(EnvFilter::new("warn")))
        });

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_start_mode() {
        let mcp = vec![
            "xenterm".to_string(),
            "mcp".to_string(),
            "serve".to_string(),
        ];
        assert!(matches!(StartMode::detect(&mcp), StartMode::Mcp));

        let cli = vec![
            "xenterm".to_string(),
            "cli".to_string(),
            "sessions".to_string(),
        ];
        assert!(matches!(StartMode::detect(&cli), StartMode::Cli));

        let version = vec!["xenterm".to_string(), "--version".to_string()];
        assert!(matches!(StartMode::detect(&version), StartMode::Version));

        // Both the bare launch and the migration-era `gpui` argument open the
        // UI shell.
        let app = vec!["xenterm".to_string()];
        assert!(matches!(StartMode::detect(&app), StartMode::Ui));
        let gpui = vec!["xenterm".to_string(), "gpui".to_string()];
        assert!(matches!(StartMode::detect(&gpui), StartMode::Ui));
    }
}
