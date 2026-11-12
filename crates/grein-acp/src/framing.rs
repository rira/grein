use serde::{Deserialize, Serialize};
use std::fmt;

/// Error hierarchy for ACP domain operations.
#[derive(Debug)]
pub enum AcpError {
    Framing(FramingError),
    Io(std::io::Error),
    Serialization(serde_json::Error),
    Subprocess(String),
}

impl fmt::Display for AcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Framing(err) => write!(f, "Framing error: {err}"),
            Self::Io(err) => write!(f, "IO error: {err}"),
            Self::Serialization(err) => write!(f, "Serialization error: {err}"),
            Self::Subprocess(msg) => write!(f, "Subprocess error: {msg}"),
        }
    }
}

impl std::error::Error for AcpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Framing(err) => Some(err),
            Self::Io(err) => Some(err),
            Self::Serialization(err) => Some(err),
            Self::Subprocess(_) => None,
        }
    }
}

impl From<FramingError> for AcpError {
    fn from(err: FramingError) -> Self {
        Self::Framing(err)
    }
}

impl From<std::io::Error> for AcpError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for AcpError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err)
    }
}

/// Specific errors encountered while framing or unframing JSON-RPC streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramingError {
    InvalidPayload(String),
    ProtocolViolation(String),
}

impl fmt::Display for FramingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPayload(msg) => write!(f, "Invalid payload: {msg}"),
            Self::ProtocolViolation(msg) => write!(f, "Protocol violation: {msg}"),
        }
    }
}

impl std::error::Error for FramingError {}

/// JSON-RPC Identifier representing numeric or string identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
}

/// JSON-RPC Error object structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl JsonRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(code: i64, message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    pub fn code(&self) -> i64 {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn data(&self) -> Option<&serde_json::Value> {
        self.data.as_ref()
    }
}

/// JSON-RPC Request message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    id: JsonRpcId,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: JsonRpcId, method: String, params: Option<serde_json::Value>) -> Self {
        Self { id, method, params }
    }

    pub fn id(&self) -> &JsonRpcId {
        &self.id
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn params(&self) -> Option<&serde_json::Value> {
        self.params.as_ref()
    }
}

/// JSON-RPC Notification message (no identifier).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: String, params: Option<serde_json::Value>) -> Self {
        Self { method, params }
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn params(&self) -> Option<&serde_json::Value> {
        self.params.as_ref()
    }
}

/// JSON-RPC Response message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    id: JsonRpcId,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn new_result(id: JsonRpcId, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn new_error(id: JsonRpcId, error: JsonRpcError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }

    pub fn id(&self) -> &JsonRpcId {
        &self.id
    }

    pub fn result(&self) -> Option<&serde_json::Value> {
        self.result.as_ref()
    }

    pub fn error(&self) -> Option<&JsonRpcError> {
        self.error.as_ref()
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }
}

/// Generic JSON-RPC Message enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
}

/// Trait for encoding JSON-RPC messages into byte frames.
pub trait MessageEncoder {
    fn encode_message(&self, message: &JsonRpcMessage) -> Result<Vec<u8>, AcpError>;
}

/// Trait for decoding JSON-RPC messages from byte frames.
pub trait MessageDecoder {
    fn decode_message(&self, bytes: &[u8]) -> Result<JsonRpcMessage, AcpError>;
}

/// Codec implementation for newline-delimited JSON-RPC framing over stdio streams.
#[derive(Debug, Default, Clone)]
pub struct StdioFrameCodec;

impl StdioFrameCodec {
    pub fn new() -> Self {
        Self
    }
}

impl MessageEncoder for StdioFrameCodec {
    fn encode_message(&self, message: &JsonRpcMessage) -> Result<Vec<u8>, AcpError> {
        let mut bytes = match message {
            JsonRpcMessage::Request(req) => {
                let mut map = serde_json::Map::new();
                map.insert(
                    "jsonrpc".to_string(),
                    serde_json::Value::String("2.0".to_string()),
                );
                map.insert("id".to_string(), serde_json::to_value(req.id())?);
                map.insert(
                    "method".to_string(),
                    serde_json::Value::String(req.method().to_string()),
                );
                if let Some(params) = req.params() {
                    map.insert("params".to_string(), params.clone());
                }
                serde_json::to_vec(&serde_json::Value::Object(map))?
            }
            JsonRpcMessage::Notification(notif) => {
                let mut map = serde_json::Map::new();
                map.insert(
                    "jsonrpc".to_string(),
                    serde_json::Value::String("2.0".to_string()),
                );
                map.insert(
                    "method".to_string(),
                    serde_json::Value::String(notif.method().to_string()),
                );
                if let Some(params) = notif.params() {
                    map.insert("params".to_string(), params.clone());
                }
                serde_json::to_vec(&serde_json::Value::Object(map))?
            }
            JsonRpcMessage::Response(res) => {
                let mut map = serde_json::Map::new();
                map.insert(
                    "jsonrpc".to_string(),
                    serde_json::Value::String("2.0".to_string()),
                );
                map.insert("id".to_string(), serde_json::to_value(res.id())?);
                if let Some(result) = res.result() {
                    map.insert("result".to_string(), result.clone());
                }
                if let Some(err) = res.error() {
                    map.insert("error".to_string(), serde_json::to_value(err)?);
                }
                serde_json::to_vec(&serde_json::Value::Object(map))?
            }
        };
        bytes.push(b'\n');
        Ok(bytes)
    }
}

impl MessageDecoder for StdioFrameCodec {
    fn decode_message(&self, bytes: &[u8]) -> Result<JsonRpcMessage, AcpError> {
        let val: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| AcpError::Framing(FramingError::InvalidPayload(e.to_string())))?;

        let obj = val.as_object().ok_or_else(|| {
            AcpError::Framing(FramingError::ProtocolViolation(
                "Expected JSON object in frame payload".to_string(),
            ))
        })?;

        match obj.get("jsonrpc").and_then(|v| v.as_str()) {
            Some("2.0") => {}
            _ => {
                return Err(AcpError::Framing(FramingError::ProtocolViolation(
                    "Missing or invalid 'jsonrpc' envelope version (expected '2.0')".to_string(),
                )));
            }
        }

        if let Some(id_val) = obj.get("id") {
            let id: JsonRpcId = serde_json::from_value(id_val.clone()).map_err(|e| {
                AcpError::Framing(FramingError::ProtocolViolation(format!(
                    "Invalid 'id' field in message: {e}"
                )))
            })?;

            if let Some(method_val) = obj.get("method") {
                let method = method_val
                    .as_str()
                    .ok_or_else(|| {
                        AcpError::Framing(FramingError::ProtocolViolation(
                            "Field 'method' must be a valid string".to_string(),
                        ))
                    })?
                    .to_string();
                let params = obj.get("params").cloned();
                Ok(JsonRpcMessage::Request(JsonRpcRequest::new(
                    id, method, params,
                )))
            } else if obj.contains_key("result") || obj.contains_key("error") {
                let result = obj.get("result").cloned();
                let error = if let Some(err_val) = obj.get("error") {
                    let err: JsonRpcError =
                        serde_json::from_value(err_val.clone()).map_err(|e| {
                            AcpError::Framing(FramingError::ProtocolViolation(format!(
                                "Invalid 'error' object structure: {e}"
                            )))
                        })?;
                    Some(err)
                } else {
                    None
                };
                Ok(JsonRpcMessage::Response(JsonRpcResponse {
                    id,
                    result,
                    error,
                }))
            } else {
                Err(AcpError::Framing(FramingError::ProtocolViolation(
                    "Message with 'id' must contain either 'method', 'result', or 'error'"
                        .to_string(),
                )))
            }
        } else if let Some(method_val) = obj.get("method") {
            let method = method_val
                .as_str()
                .ok_or_else(|| {
                    AcpError::Framing(FramingError::ProtocolViolation(
                        "Notification 'method' must be a valid string".to_string(),
                    ))
                })?
                .to_string();
            let params = obj.get("params").cloned();
            Ok(JsonRpcMessage::Notification(JsonRpcNotification::new(
                method, params,
            )))
        } else {
            Err(AcpError::Framing(FramingError::ProtocolViolation(
                "Malformed JSON-RPC envelope: missing both 'id' and 'method'".to_string(),
            )))
        }
    }
}
