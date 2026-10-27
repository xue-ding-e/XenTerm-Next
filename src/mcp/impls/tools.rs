use anyhow::Result;
use serde_json::{json, Value};

pub(super) fn definitions() -> Value {
    json!([
        {
            "name": "get_access_policy",
            "description": "Read the configured MCP access mode and effective permissions without changing them. Full access permits SSH commands and file operations; restricted access follows configured capability rules. Execution history is optional and off by default. SSH account and server permissions always apply. Contains no credentials. Access mode can only be changed locally in Settings > MCP & permissions or with xenterm cli permissions.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "import_sessions",
            "description": "Preview or append sessions from a local MeatShell/XenTerm v1 portable export, native sessions.json, or FinalShell JSON. Defaults to dry_run=true. Applying requires --allow-config-import; all calls require the MCP file-transfer permission. Existing sessions are never overwritten: equivalent complete profiles are skipped and distinct aliases get fresh IDs. Returns counts and fixed compatibility warnings without source field values. Export files contain reversible credential obfuscation and must be kept private.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "local_path": {"type": "string", "minLength": 1, "description": "JSON file on the server host, at most 16 MiB."},
                    "dry_run": {"type": "boolean", "default": true}
                },
                "required": ["local_path"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "export_sessions",
            "description": "Write every saved session to a new private portable export on the server host. Requires both MCP saved-credentials and file-transfer permissions. An export contains reversible credential obfuscation, so keep it private. Pass include_credentials=true explicitly to acknowledge the credential export. The destination must not already exist and must be outside the source profile directory. Only saved sessions are exported; global settings, host trust and external key files are not included.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "local_path": {"type": "string", "minLength": 1, "description": "New file path on the server host. Must not exist and must be outside the XenTerm profile directory."},
                    "include_credentials": {"type": "boolean", "default": false, "description": "Must be true; acknowledged that the export contains recoverable credentials."}
                },
                "required": ["local_path", "include_credentials"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        },
        {
            "name": "list_sessions",
            "description": "List saved XenTerm sessions without exposing passwords, private keys, or other secrets. Requires the MCP saved-credentials permission.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "group": { "type": "string", "description": "Optional exact session group filter." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "get_session",
            "description": "Get non-secret connection metadata for one saved XenTerm session. Requires the MCP saved-credentials permission.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Stable session id returned by list_sessions." }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "run_command",
            "description": "Execute one non-interactive command on a saved SSH session. Requires the MCP saved-credentials and arbitrary-command permissions. Use get_access_policy to inspect the configured capabilities and optional execution history. SSH account and server permissions apply. XenTerm does not request per-operation risk approval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Stable session id returned by list_sessions." },
                    "command": { "type": "string", "minLength": 1 },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 30 },
                    "max_output_bytes": { "type": "integer", "minimum": 1024, "maximum": 4194304, "default": 1048576 }
                },
                "required": ["session_id", "command"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_remote_files",
            "description": "List a remote directory over XenTerm SFTP without exposing credentials. Requires the MCP file-transfer permission.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "path": { "type": "string", "default": "." },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 30 }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "read_remote_text_file",
            "description": "Read a bounded UTF-8 text file over XenTerm SFTP. Binary, oversized, or excessively long files are rejected. Requires the MCP file-transfer permission.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "path": { "type": "string", "minLength": 1 },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 30 }
                },
                "required": ["session_id", "path"],
                "additionalProperties": false
            }
        },
        {
            "name": "upload_file",
            "description": "Upload one local file to a remote directory over XenTerm SFTP. Requires the MCP file-transfer permission.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "local_path": { "type": "string", "minLength": 1 },
                    "remote_directory": { "type": "string", "minLength": 1 },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 120 }
                },
                "required": ["session_id", "local_path", "remote_directory"],
                "additionalProperties": false
            }
        },
        {
            "name": "download_file",
            "description": "Download one remote file into an existing local directory over XenTerm SFTP. Existing files are not overwritten. Requires the MCP file-transfer permission.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "remote_path": { "type": "string", "minLength": 1 },
                    "local_directory": { "type": "string", "minLength": 1 },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 120 }
                },
                "required": ["session_id", "remote_path", "local_directory"],
                "additionalProperties": false
            }
        }
    ])
}

pub(super) async fn call_mcp(
    name: &str,
    arguments: &Value,
    allow_config_import: bool,
) -> Result<Value> {
    crate::automation::call(
        name,
        arguments,
        crate::automation::Frontend::Mcp {
            allow_config_import,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_do_not_expose_secret_arguments() {
        let text = definitions().to_string();
        assert!(!text.contains("\"password\":"));
        assert!(!text.contains("\"private_key_inline\":"));
        assert!(text.contains("run_command"));
    }

    /// Export must stay reachable from the protocol adapter.
    ///
    /// It used to exist only in the CLI, which meant an MCP caller allowed to
    /// run arbitrary commands still could not read the profile it was acting
    /// on. Both adapters now route through `automation::call`, so the tool has
    /// to be advertised here or the parity silently comes back.
    #[test]
    fn export_sessions_is_advertised() {
        let tools: Value = definitions();
        let tools = tools.as_array().expect("tool definitions");
        let export = tools
            .iter()
            .find(|tool| tool["name"] == "export_sessions")
            .expect("export_sessions must be advertised");

        let required: Vec<&str> = export["inputSchema"]["required"]
            .as_array()
            .expect("required list")
            .iter()
            .filter_map(|value| value.as_str())
            .collect();
        assert!(required.contains(&"local_path"));
        assert!(
            required.contains(&"include_credentials"),
            "export must require an explicit credential acknowledgement"
        );
    }

    /// Every advertised tool must have a dispatch arm.
    ///
    /// A definition with no matching arm only fails when a client calls it, so
    /// assert the pairing here instead. The dispatch table lives in the shared
    /// automation layer because CLI and MCP both call through it.
    #[test]
    fn advertised_tools_are_dispatched() {
        let source = include_str!("../../automation/impls/tools.rs");
        let tools: Value = definitions();
        let tools = tools.as_array().expect("tool definitions");
        for tool in tools {
            let name = tool["name"].as_str().expect("tool name");
            let expected = format!("\"{name}\" =>");
            assert!(
                source.contains(&expected),
                "tool {name} is advertised but has no dispatch arm"
            );
        }
    }

    /// The export must stay behind the saved-credentials and file-transfer gates.
    ///
    /// The description promises the gate, so the implementation has to apply
    /// it. Only checking the file-transfer switch would let an MCP caller with
    /// transfers enabled but credential use disabled read every password in
    /// the profile -- the exact bypass the gate exists to prevent.
    #[test]
    fn export_checks_the_saved_credentials_gate() {
        let source = include_str!("../../automation/impls/tools.rs");
        let start = source
            .find("fn export_sessions(")
            .expect("export_sessions implementation");
        let body = &source[start..];
        let end = body.find("\nasync fn ").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("mcp_use_saved_credentials"),
            "export_sessions must check the saved-credentials permission"
        );
        assert!(
            body.contains("is_unattended"),
            "the saved-credentials gate must only constrain unattended callers"
        );
        assert!(
            body.contains("enforce_export_permissions"),
            "export_sessions must also require file-transfer permission"
        );
        assert!(
            body.contains("export_to_new"),
            "export_sessions must publish through the no-clobber writer"
        );
    }
}
