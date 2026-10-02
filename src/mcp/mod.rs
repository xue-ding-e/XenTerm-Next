//! MCP adapters over the shared automation policy.
#[cfg(feature = "remote-mcp")]
#[path = "impls/http.rs"]
mod http;
#[cfg(feature = "remote-mcp")]
#[path = "impls/oauth.rs"]
mod oauth;
#[path = "impls/server.rs"]
mod server;
#[path = "impls/tools.rs"]
mod tools;

pub(crate) fn run(args: &[String]) -> anyhow::Result<()> {
    let mut allow_config_import = false;
    let mut http_config = None;
    let mut arguments = args.iter().skip(3);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--allow-config-import" if !allow_config_import => allow_config_import = true,
            "--http-config" if http_config.is_none() => {
                http_config = Some(
                    arguments
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--http-config requires a path"))?,
                );
            }
            _ => anyhow::bail!("unknown or duplicate MCP argument: {argument}"),
        }
    }
    if let Some(path) = http_config {
        anyhow::ensure!(
            crate::config::has_explicit_data_dir(),
            "HTTP service requires an explicitly selected --data-dir or XENTERM_DATA_DIR profile"
        );
        #[cfg(feature = "remote-mcp")]
        {
            http::run(path, allow_config_import)
        }
        #[cfg(not(feature = "remote-mcp"))]
        {
            let _ = path;
            anyhow::bail!("HTTP transport requires the remote-mcp build feature")
        }
    } else {
        server::run_stdio(allow_config_import)
    }
}
