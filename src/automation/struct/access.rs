/// Entry point invoking the shared XenTerm automation capabilities.
///
/// MCP and plugins apply persisted permission gates because an external process
/// initiates those calls and nobody is at the keyboard choosing to make them.
/// CLI commands are explicit local user actions and therefore do not depend on
/// whether the MCP server itself is enabled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frontend {
    Mcp { allow_config_import: bool },
    Cli,
}

impl Frontend {
    pub(crate) fn is_mcp(self) -> bool {
        matches!(self, Self::Mcp { .. })
    }

    pub(crate) fn allows_config_import(self) -> bool {
        matches!(
            self,
            Self::Cli
                | Self::Mcp {
                    allow_config_import: true
                }
        )
    }

    /// Whether this caller has to pass the persisted gates before reaching a
    /// session's credentials, running a command or transferring a file.
    ///
    /// A method rather than `== Frontend::Mcp` at each gate because four call
    /// sites each spelling the comparison is four chances to forget one of them
    /// the next time an unattended caller shows up.
    ///
    /// Deliberately does not cover `mcp_enabled`, which is the MCP server's own
    /// on-switch rather than a gate on a capability.
    pub(crate) fn is_unattended(self) -> bool {
        matches!(self, Self::Mcp { .. })
    }
}
