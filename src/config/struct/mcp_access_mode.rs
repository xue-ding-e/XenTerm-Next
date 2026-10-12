use serde::{Deserialize, Serialize};

/// The scope granted by XenTerm to an MCP client. SSH account permissions
/// and the MCP server's own enable switch remain independent.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpAccessMode {
    #[default]
    Restricted,
    Full,
}

impl McpAccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restricted => "restricted",
            Self::Full => "full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "restricted" => Some(Self::Restricted),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}
