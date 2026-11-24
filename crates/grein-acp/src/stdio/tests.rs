use std::path::Path;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use super::parser::{
    build_prompt_completed, parse_config_options_result, parse_initialize_result,
    parse_permission_request, parse_session_info, translate_session_update,
};
use super::*;

#[test]
fn parses_initialize_result_with_capabilities_and_auth_methods() {
    let val = json!({
        "protocolVersion": 1,
        "agentInfo": { "name": "agy", "version": "1.5.14" },
        "authMethods": [
            { "id": "oauth-personal", "name": "OAuth", "description": "Google" },
            { "id": "vertex", "name": "Vertex", "description": "GCP" }
        ],
        "agentCapabilities": {
            "_meta": {
                "antigravity": {
                    "systemPrompt": { "append": true }
                }
            }
        }
    });
    let parsed = parse_initialize_result(&val).expect("should parse");
    assert_eq!(
        parsed,
        AcpInitializeResult {
            protocol_version: 1,
            agent_name: Some("agy".into()),
            agent_version: Some("1.5.14".into()),
            supports_system_prompt_append: true,
            auth_method_ids: vec!["oauth-personal".into(), "vertex".into()],
        }
    );

    let missing_ver = json!({ "agentInfo": {} });
    assert!(parse_initialize_result(&missing_ver).is_err());
}

#[test]
fn parses_session_info_with_and_without_models_modes() {
    let full = json!({
        "sessionId": "sid-123",
        "models": {
            "currentModelId": "gemini-2.5-pro",
            "availableModels": [
                {
                    "modelId": "gemini-2.5-pro",
                    "name": "Gemini 2.5 Pro",
                    "description": "Deep reasoning"
                },
                {
                    "modelId": "gemini-2.5-flash",
                    "name": "Gemini 2.5 Flash"
                }
            ]
        },
        "modes": {
            "currentModeId": "architect",
            "availableModes": [
                {
                    "id": "architect",
                    "name": "Architect",
                    "description": "Plan only"
                },
                {
                    "id": "code",
                    "name": "Code",
                    "description": null
                }
            ]
        },
        "configOptions": [
            {
                "id": "thought_level",
                "name": "Thinking",
                "category": "thought_level",
                "type": "select",
                "currentValue": "high",
                "options": [
                    { "value": "low", "name": "Low", "description": "Fast" },
                    { "value": "high", "name": "High" }
                ]
            }
        ]
    });
    assert_eq!(
        parse_session_info(&full).expect("full session info"),
        AcpSessionInfo {
            session_id: "sid-123".into(),
            current_model_id: Some("gemini-2.5-pro".into()),
            current_mode_id: Some("architect".into()),
            available_models: vec![
                AcpModelOption {
                    model_id: "gemini-2.5-pro".into(),
                    name: "Gemini 2.5 Pro".into(),
                    description: Some("Deep reasoning".into()),
                },
                AcpModelOption {
                    model_id: "gemini-2.5-flash".into(),
                    name: "Gemini 2.5 Flash".into(),
                    description: None,
                },
            ],
            available_modes: vec![
                AcpModeOption {
                    id: "architect".into(),
                    name: "Architect".into(),
                    description: Some("Plan only".into()),
                },
                AcpModeOption {
                    id: "code".into(),
                    name: "Code".into(),
                    description: None,
                },
            ],
            config_options: vec![AcpConfigOption {
                id: "thought_level".into(),
                name: "Thinking".into(),
                category: Some("thought_level".into()),
                current_value: "high".into(),
                options: vec![
                    AcpConfigChoice {
                        value: "low".into(),
                        name: "Low".into(),
                        description: Some("Fast".into()),
                    },
                    AcpConfigChoice {
                        value: "high".into(),
                        name: "High".into(),
                        description: None,
                    },
                ],
            }],
        }
    );

    let minimal = json!({ "sessionId": "sid-456" });
    assert_eq!(
        parse_session_info(&minimal).expect("minimal session info"),
        AcpSessionInfo {
            session_id: "sid-456".into(),
            ..Default::default()
        }
    );

    assert!(parse_session_info(&json!({ "sessionId": "" })).is_err());
}

#[test]
fn translates_all_session_update_variants_and_ignores_unknown() {
    let make_notif = |update: serde_json::Value| JsonRpcNotification {
        jsonrpc: "2.0".into(),
        method: "session/update".into(),
        params: Some(json!({ "sessionId": "s1", "update": update })),
    };

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "hi" }
        }))),
        Some((
            "s1".into(),
            DriverEvent::StreamChunk {
                session_id: "s1".into(),
                text: "hi".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": "hmm" }
        }))),
        Some((
            "s1".into(),
            DriverEvent::ThoughtChunk {
                session_id: "s1".into(),
                text: "hmm".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "name": "read_file",
            "title": "Reading",
            "args": { "Path": "src/lib.rs" },
            "status": "pending"
        }))),
        Some((
            "s1".into(),
            DriverEvent::ToolCall {
                session_id: "s1".into(),
                tool_call_id: "tc1".into(),
                name: "read_file".into(),
                title: "Reading".into(),
                args: json!({ "Path": "src/lib.rs" }),
                status: "pending".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc1",
            "name": "read_file",
            "status": "completed",
            "rawOutput": { "output": "file body" }
        }))),
        Some((
            "s1".into(),
            DriverEvent::ToolCallUpdate {
                session_id: "s1".into(),
                tool_call_id: "tc1".into(),
                name: "read_file".into(),
                status: "completed".into(),
                output: "file body".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc2",
            "name": "run_command",
            "status": "completed",
            "content": [{ "type": "content", "content": { "type": "text", "text": "stdout text" } }]
        }))),
        Some((
            "s1".into(),
            DriverEvent::ToolCallUpdate {
                session_id: "s1".into(),
                tool_call_id: "tc2".into(),
                name: "run_command".into(),
                status: "completed".into(),
                output: "stdout text".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "session_info_update",
            "title": "New Title"
        }))),
        Some((
            "s1".into(),
            DriverEvent::SessionInfoUpdate {
                session_id: "s1".into(),
                title: "New Title".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "usage_update",
            "used": 400,
            "size": 1000000
        }))),
        Some((
            "s1".into(),
            DriverEvent::UsageUpdate {
                session_id: "s1".into(),
                used: 400,
                size: 1000000,
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "current_mode_update",
            "currentModeId": "architect"
        }))),
        Some((
            "s1".into(),
            DriverEvent::CurrentModeUpdate {
                session_id: "s1".into(),
                current_mode_id: "architect".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "current_mode_update",
            "modeId": "code"
        }))),
        Some((
            "s1".into(),
            DriverEvent::CurrentModeUpdate {
                session_id: "s1".into(),
                current_mode_id: "code".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                { "name": "compact", "description": "Compact context" },
                { "name": "help", "description": null },
                { "name": "clear" }
            ]
        }))),
        Some((
            "s1".into(),
            DriverEvent::AvailableCommandsUpdate {
                session_id: "s1".into(),
                commands: vec![
                    AcpSlashCommand {
                        name: "compact".into(),
                        description: "Compact context".into(),
                    },
                    AcpSlashCommand {
                        name: "help".into(),
                        description: String::new(),
                    },
                    AcpSlashCommand {
                        name: "clear".into(),
                        description: String::new(),
                    },
                ],
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "config_option_update",
            "configOptions": [
                {
                    "id": "model",
                    "name": "Model",
                    "category": "model",
                    "currentValue": "gemini-2.5-flash",
                    "options": [
                        { "value": "gemini-2.5-flash", "name": "Flash" }
                    ]
                }
            ]
        }))),
        Some((
            "s1".into(),
            DriverEvent::ConfigOptionUpdate {
                session_id: "s1".into(),
                config_options: vec![AcpConfigOption {
                    id: "model".into(),
                    name: "Model".into(),
                    category: Some("model".into()),
                    current_value: "gemini-2.5-flash".into(),
                    options: vec![AcpConfigChoice {
                        value: "gemini-2.5-flash".into(),
                        name: "Flash".into(),
                        description: None,
                    }],
                }],
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "unknown_future_update",
            "payload": {}
        }))),
        None
    );
}

#[test]
fn parses_set_config_option_response() {
    let valid = json!({
        "configOptions": [
            {
                "id": "mode",
                "name": "Mode",
                "category": "mode",
                "currentValue": "plan",
                "options": [
                    { "value": "code", "name": "Code", "description": "Edit files" },
                    { "value": "plan", "name": "Plan" }
                ]
            }
        ]
    });
    assert_eq!(
        parse_config_options_result(&valid).expect("valid configOptions"),
        vec![AcpConfigOption {
            id: "mode".into(),
            name: "Mode".into(),
            category: Some("mode".into()),
            current_value: "plan".into(),
            options: vec![
                AcpConfigChoice {
                    value: "code".into(),
                    name: "Code".into(),
                    description: Some("Edit files".into()),
                },
                AcpConfigChoice {
                    value: "plan".into(),
                    name: "Plan".into(),
                    description: None,
                },
            ],
        }]
    );
    assert_eq!(
        parse_config_options_result(&json!({})).expect("empty"),
        vec![]
    );
    assert_eq!(
        parse_config_options_result(&json!({ "configOptions": null })).expect("null"),
        vec![]
    );
    assert!(parse_config_options_result(&json!({ "configOptions": "bad" })).is_err());
}

struct StubAgentDriver;

#[async_trait]
impl AgentDriver for StubAgentDriver {
    fn is_running(&self) -> bool {
        true
    }
    async fn initialize(&self) -> Result<AcpInitializeResult, AgentDriverError> {
        unreachable!()
    }
    async fn authenticate(&self, _method_id: &str) -> Result<(), AgentDriverError> {
        unreachable!()
    }
    async fn new_session(
        &self,
        _cwd: &Path,
        _system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, AgentDriverError> {
        unreachable!()
    }
    async fn send_prompt(
        &self,
        _session_id: &str,
        _prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, AgentDriverError> {
        unreachable!()
    }
    async fn respond_permission(
        &self,
        _request_id: serde_json::Value,
        _option_id: Option<&str>,
    ) -> Result<(), AgentDriverError> {
        unreachable!()
    }
    async fn cancel_session(&self, _session_id: &str) -> Result<(), AgentDriverError> {
        unreachable!()
    }
    async fn close_session(&self, _session_id: &str) -> Result<(), AgentDriverError> {
        unreachable!()
    }
    async fn shutdown(&self) -> Result<(), AgentDriverError> {
        unreachable!()
    }
}

#[tokio::test]
async fn default_agent_driver_config_setters_return_not_supported() {
    let driver = StubAgentDriver;
    assert!(driver.is_running());
    assert!(matches!(
        driver.set_session_model("s1", "m1").await,
        Err(AgentDriverError::Unsupported(msg)) if msg == "not supported"
    ));
    assert!(matches!(
        driver.set_session_mode("s1", "mode1").await,
        Err(AgentDriverError::Unsupported(msg)) if msg == "not supported"
    ));
    assert!(matches!(
        driver.set_config_option("s1", "cfg", "val").await,
        Err(AgentDriverError::Unsupported(msg)) if msg == "not supported"
    ));
}

#[test]
fn parses_permission_request_preserving_exact_id_value() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: json!("1"),
        method: "session/request_permission".into(),
        params: Some(json!({
            "sessionId": "sid-9",
            "toolCall": {
                "toolCallId": "tc-9",
                "name": "write_file",
                "title": "Write",
                "args": { "Path": "foo.rs" }
            },
            "options": [
                { "optionId": "allow_once", "name": "Allow", "kind": "allow_once" }
            ]
        })),
    };
    let parsed = parse_permission_request(&req).expect("should parse permission");
    assert_eq!(
        parsed,
        (
            "sid-9".into(),
            DriverEvent::PermissionRequested {
                request_id: json!("1"),
                session_id: "sid-9".into(),
                tool_call_id: "tc-9".into(),
                tool_name: "write_file".into(),
                title: "Write".into(),
                args: json!({ "Path": "foo.rs" }),
                options: vec![AcpPermissionOption {
                    option_id: "allow_once".into(),
                    name: "Allow".into(),
                    kind: "allow_once".into(),
                }],
            }
        )
    );
}

#[test]
fn decodes_null_result_and_string_id_request_messages() {
    let null_res =
        serde_json::from_str::<JsonRpcMessage>(r#"{"jsonrpc":"2.0","id":2,"result":null}"#)
            .expect("null result response");
    assert!(matches!(
        null_res,
        JsonRpcMessage::Response(JsonRpcResponse {
            result: serde_json::Value::Null,
            ..
        })
    ));

    let str_req = serde_json::from_str::<JsonRpcMessage>(
        r#"{"jsonrpc":"2.0","id":"1","method":"session/request_permission","params":{}}"#,
    )
    .expect("string id request");
    assert!(matches!(
        str_req,
        JsonRpcMessage::Request(JsonRpcRequest {
            id: serde_json::Value::String(_),
            ..
        })
    ));
}

#[test]
fn prompt_completed_extracts_camel_case_usage() {
    let ev = build_prompt_completed(
        "sid-1",
        &json!({
            "stopReason": "end_turn",
            "usage": {
                "inputTokens": 11,
                "outputTokens": 22,
                "cachedReadTokens": 33,
                "cachedWriteTokens": 44,
                "thoughtTokens": 55,
                "totalTokens": 165
            }
        }),
    );
    assert_eq!(
        ev,
        DriverEvent::PromptCompleted {
            session_id: "sid-1".into(),
            stop_reason: "end_turn".into(),
            usage: Some(AcpUsage {
                input_tokens: 11,
                output_tokens: 22,
                cached_read_tokens: 33,
                cached_write_tokens: 44,
                thought_tokens: 55,
                total_tokens: 165,
            }),
        }
    );
}

#[test]
fn parser_tool_call_update_and_fallback_edge_cases() {
    use super::parser::format_rpc_error;

    assert_eq!(format_rpc_error(&json!({"message": "plain"})), "plain");
    assert_eq!(format_rpc_error(&json!(404)), "404");

    let make_notif = |update: serde_json::Value| JsonRpcNotification {
        jsonrpc: "2.0".into(),
        method: "session/update".into(),
        params: Some(json!({ "sessionId": "s1", "update": update })),
    };

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-content-text",
            "name": "bash",
            "status": "completed",
            "rawOutput": { "output": "" },
            "content": [{ "type": "text", "text": "direct content text" }]
        }))),
        Some((
            "s1".into(),
            DriverEvent::ToolCallUpdate {
                session_id: "s1".into(),
                tool_call_id: "tc-content-text".into(),
                name: "bash".into(),
                status: "completed".into(),
                output: "direct content text".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-nonstring-raw",
            "name": "bash",
            "status": "completed",
            "rawOutput": { "output": { "exitCode": 0 } }
        }))),
        Some((
            "s1".into(),
            DriverEvent::ToolCallUpdate {
                session_id: "s1".into(),
                tool_call_id: "tc-nonstring-raw".into(),
                name: "bash".into(),
                status: "completed".into(),
                output: "{\"exitCode\":0}".into(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-empty-raw-str",
            "name": "bash",
            "status": "completed",
            "rawOutput": { "output": "" }
        }))),
        Some((
            "s1".into(),
            DriverEvent::ToolCallUpdate {
                session_id: "s1".into(),
                tool_call_id: "tc-empty-raw-str".into(),
                name: "bash".into(),
                status: "completed".into(),
                output: String::new(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&make_notif(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-null-raw",
            "name": "bash",
            "status": "completed",
            "rawOutput": { "output": null }
        }))),
        Some((
            "s1".into(),
            DriverEvent::ToolCallUpdate {
                session_id: "s1".into(),
                tool_call_id: "tc-null-raw".into(),
                name: "bash".into(),
                status: "completed".into(),
                output: String::new(),
            }
        ))
    );

    assert_eq!(
        translate_session_update(&JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "other/method".into(),
            params: None,
        }),
        None
    );

    assert_eq!(
        parse_permission_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: "other/req".into(),
            params: None,
        }),
        None
    );
}

#[tokio::test]
async fn default_agent_driver_trait_methods_covered() {
    let driver = StubAgentDriver;
    assert!(driver.recent_stderr().is_empty());
    let mut rx = driver.subscribe_session("s1");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        driver.submit_prompt("s1", "hi").await,
        Err(AgentDriverError::Unsupported(msg)) if msg.contains("submit_prompt")
    ));
}
