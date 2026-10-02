use anyhow::Result;
use serde_json::{json, Value};

pub(super) fn definitions() -> Value {
    json!([
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
            "description": "Execute one non-interactive command on a saved SSH session. Requires the MCP saved-credentials and arbitrary-command permissions. Commands flagged as risky (destructive patterns or system directories) are held for human approval in the XenTerm window: the call returns an error [denied-by-user] when the user refuses or [denied-by-timeout]/[denied-by-limit] when nobody answers. On any of those errors, stop and report the refusal to the user - do NOT retry the command, do NOT try to route around the approval, do NOT approve on the user's behalf, and do NOT assume a later call will be treated differently.",
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

pub(super) async fn call_mcp(name: &str, arguments: &Value) -> Result<Value> {
    crate::automation::call(name, arguments, crate::automation::Frontend::Mcp).await
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
}
