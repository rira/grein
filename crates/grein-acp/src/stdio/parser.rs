use super::protocol::{
    AcpConfigOption, AcpInitializeResult, AcpPermissionOption, AcpSessionInfo, AcpUsage,
    DriverEvent, JsonRpcNotification, JsonRpcRequest, StdioRunnerError,
};

pub(super) fn format_rpc_error(err: &serde_json::Value) -> String {
    if let Some(msg) = err.get("message").and_then(|v| v.as_str()) {
        if let Some(code) = err.get("code").and_then(|v| v.as_i64()) {
            return format!("JSON-RPC error {code}: {msg}");
        }
        return msg.to_string();
    }
    err.to_string()
}

pub(super) fn parse_initialize_result(
    result: &serde_json::Value,
) -> Result<AcpInitializeResult, StdioRunnerError> {
    let protocol_version = result
        .get("protocolVersion")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            StdioRunnerError::CommunicationError(
                "initialize response missing protocolVersion".into(),
            )
        })?;
    let agent_name = result
        .pointer("/agentInfo/name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let agent_version = result
        .pointer("/agentInfo/version")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let supports_system_prompt_append = result
        .pointer("/agentCapabilities/_meta/antigravity/systemPrompt/append")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let auth_method_ids = result
        .get("authMethods")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Ok(AcpInitializeResult {
        protocol_version,
        agent_name,
        agent_version,
        supports_system_prompt_append,
        auth_method_ids,
    })
}

fn parse_json_vec<T: serde::de::DeserializeOwned>(value: Option<&serde_json::Value>) -> Vec<T> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| serde_json::from_value::<T>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn parse_config_options_result(
    result: &serde_json::Value,
) -> Result<Vec<AcpConfigOption>, StdioRunnerError> {
    match result.get("configOptions") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(v) if v.is_array() => Ok(parse_json_vec(Some(v))),
        Some(_) => Err(StdioRunnerError::CommunicationError(
            "session/set_config_option response configOptions must be an array".into(),
        )),
    }
}

pub(super) fn parse_session_info(
    result: &serde_json::Value,
) -> Result<AcpSessionInfo, StdioRunnerError> {
    let session_id = result
        .get("sessionId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            StdioRunnerError::CommunicationError("session/new response missing sessionId".into())
        })?
        .to_string();
    let current_model_id = result
        .pointer("/models/currentModelId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let current_mode_id = result
        .pointer("/modes/currentModeId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let available_models = parse_json_vec(result.pointer("/models/availableModels"));
    let available_modes = parse_json_vec(result.pointer("/modes/availableModes"));
    let config_options = parse_json_vec(result.get("configOptions"));

    Ok(AcpSessionInfo {
        session_id,
        current_model_id,
        current_mode_id,
        available_models,
        available_modes,
        config_options,
    })
}

pub(super) fn build_prompt_completed(session_id: &str, result: &serde_json::Value) -> DriverEvent {
    let stop_reason = result
        .get("stopReason")
        .and_then(|v| v.as_str())
        .unwrap_or("end_turn")
        .to_string();
    let usage = result
        .get("usage")
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value::<AcpUsage>(v.clone()).ok());
    DriverEvent::PromptCompleted {
        session_id: session_id.to_string(),
        stop_reason,
        usage,
    }
}

fn str_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_default()
}

fn extract_tool_call_update_output(update: &serde_json::Value) -> String {
    if let Some(s) = update.pointer("/rawOutput/output").and_then(|v| v.as_str()) {
        if !s.is_empty() {
            return s.to_string();
        }
    }
    if let Some(s) = update
        .pointer("/content/0/content/text")
        .and_then(|v| v.as_str())
    {
        return s.to_string();
    }
    if let Some(s) = update.pointer("/content/0/text").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(v) = update.pointer("/rawOutput/output") {
        if let Some(s) = v.as_str() {
            return s.to_string();
        }
        if !v.is_null() {
            return v.to_string();
        }
    }
    String::new()
}

pub(super) fn translate_session_update(n: &JsonRpcNotification) -> Option<(String, DriverEvent)> {
    if n.method != "session/update" {
        tracing::debug!("ignoring unmapped notification: {}", n.method);
        return None;
    }
    let params = n.params.as_ref()?;
    let session_id = params
        .get("sessionId")
        .and_then(|v| v.as_str())?
        .to_string();
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate").and_then(|v| v.as_str())?;

    let event = match kind {
        "agent_message_chunk" => DriverEvent::StreamChunk {
            session_id: session_id.clone(),
            text: update
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "agent_thought_chunk" => DriverEvent::ThoughtChunk {
            session_id: session_id.clone(),
            text: update
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "tool_call" => DriverEvent::ToolCall {
            session_id: session_id.clone(),
            tool_call_id: str_field(update, "toolCallId"),
            name: str_field(update, "name"),
            title: str_field(update, "title"),
            args: update
                .get("args")
                .or_else(|| update.get("rawInput"))
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            status: str_field(update, "status"),
        },
        "tool_call_update" => DriverEvent::ToolCallUpdate {
            session_id: session_id.clone(),
            tool_call_id: str_field(update, "toolCallId"),
            name: str_field(update, "name"),
            status: str_field(update, "status"),
            output: extract_tool_call_update_output(update),
        },
        "session_info_update" => DriverEvent::SessionInfoUpdate {
            session_id: session_id.clone(),
            title: str_field(update, "title"),
        },
        "current_mode_update" => DriverEvent::CurrentModeUpdate {
            session_id: session_id.clone(),
            current_mode_id: update
                .get("currentModeId")
                .and_then(|v| v.as_str())
                .or_else(|| update.get("modeId").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string(),
        },
        "config_option_update" => DriverEvent::ConfigOptionUpdate {
            session_id: session_id.clone(),
            config_options: parse_json_vec(update.get("configOptions")),
        },
        "available_commands_update" => DriverEvent::AvailableCommandsUpdate {
            session_id: session_id.clone(),
            commands: parse_json_vec(update.get("availableCommands")),
        },
        "usage_update" => DriverEvent::UsageUpdate {
            session_id: session_id.clone(),
            used: update.get("used").and_then(|v| v.as_u64()).unwrap_or(0),
            size: update.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
        },
        other => {
            tracing::debug!("ignoring unknown session/update variant: {other}");
            return None;
        }
    };

    Some((session_id, event))
}

pub(super) fn parse_permission_request(req: &JsonRpcRequest) -> Option<(String, DriverEvent)> {
    if req.method != "session/request_permission" {
        return None;
    }
    let params = req.params.as_ref()?;
    let session_id = params
        .get("sessionId")
        .and_then(|v| v.as_str())?
        .to_string();
    let tool_call = params.get("toolCall");
    let tool_call_id = tool_call
        .map(|tc| str_field(tc, "toolCallId"))
        .unwrap_or_default();
    let tool_name = tool_call
        .map(|tc| str_field(tc, "name"))
        .unwrap_or_default();
    let title = tool_call
        .map(|tc| str_field(tc, "title"))
        .unwrap_or_default();
    let args = tool_call
        .and_then(|tc| tc.get("args").or_else(|| tc.get("rawInput")))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let options = params
        .get("options")
        .and_then(|v| serde_json::from_value::<Vec<AcpPermissionOption>>(v.clone()).ok())
        .unwrap_or_default();

    Some((
        session_id.clone(),
        DriverEvent::PermissionRequested {
            request_id: req.id.clone(),
            session_id,
            tool_call_id,
            tool_name,
            title,
            args,
            options,
        },
    ))
}
