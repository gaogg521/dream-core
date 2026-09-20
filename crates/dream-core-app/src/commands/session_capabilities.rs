use serde_json::{Value, json};

pub(crate) fn data() -> Value {
    json!({
        "schema_version": 1,
        "contract": "agent-facing-session-cli",
        "commands": {
            "capabilities": { "runtime_env_required": [] },
            "list": {
                "runtime_env_required": ["ONE_BASE_URL", "ONE_USER_ID", "ONE_CONVERSATION_ID", "ONE_RUNTIME_TOKEN"],
                "description": "List conversations owned by the current user (id, title, workspace, updated_at)."
            }
        },
        "output_envelope": {
            "success": "boolean",
            "data": "object when success=true",
            "error": "object when success=false"
        }
    })
}
