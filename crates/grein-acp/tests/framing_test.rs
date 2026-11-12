use grein_acp::framing::{
    AcpError, FramingError, JsonRpcId, JsonRpcMessage, JsonRpcRequest, MessageDecoder,
    MessageEncoder, StdioFrameCodec,
};
use serde_json::json;

#[test]
fn test_serialize_and_frame_request_message() {
    let codec = StdioFrameCodec::new();
    let request = JsonRpcRequest::new(
        JsonRpcId::Number(1),
        "initialize".to_string(),
        Some(json!({"protocolVersion": "2024-11-05"})),
    );
    let message = JsonRpcMessage::Request(request);

    let framed_bytes = codec
        .encode_message(&message)
        .expect("encoding must succeed");
    let framed_str = std::str::from_utf8(&framed_bytes).expect("framed output must be valid utf8");

    assert!(
        framed_str.ends_with("\n"),
        "framed output must terminate with newline"
    );
    assert!(framed_str.contains("\"jsonrpc\":\"2.0\""));
    assert!(framed_str.contains("\"method\":\"initialize\""));
}

#[test]
fn test_decode_valid_framed_request_and_notification() {
    let codec = StdioFrameCodec::new();
    let raw_payload = b"{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"ping\",\"params\":null}\n";

    let decoded = codec
        .decode_message(raw_payload)
        .expect("decoding valid request must succeed");

    match decoded {
        JsonRpcMessage::Request(req) => {
            assert_eq!(req.id(), &JsonRpcId::Number(42));
            assert_eq!(req.method(), "ping");
            assert!(req.params().is_none() || req.params() == Some(&serde_json::Value::Null));
        }
        _ => panic!("expected JsonRpcMessage::Request"),
    }

    let notification_payload =
        b"{\"jsonrpc\":\"2.0\",\"method\":\"notify\",\"params\":{\"status\":\"ready\"}}\n";
    let decoded_notify = codec
        .decode_message(notification_payload)
        .expect("decoding valid notification must succeed");

    match decoded_notify {
        JsonRpcMessage::Notification(notif) => {
            assert_eq!(notif.method(), "notify");
            assert!(notif.params().is_some());
        }
        _ => panic!("expected JsonRpcMessage::Notification"),
    }
}

#[test]
fn test_decode_response_with_result_and_error() {
    let codec = StdioFrameCodec::new();
    let response_success = b"{\"jsonrpc\":\"2.0\",\"id\":\"req-1\",\"result\":{\"pong\":true}}\n";

    let decoded = codec
        .decode_message(response_success)
        .expect("decoding success response must succeed");

    match decoded {
        JsonRpcMessage::Response(res) => {
            assert_eq!(res.id(), &JsonRpcId::String("req-1".to_string()));
            assert!(res.is_success());
            assert_eq!(res.result(), Some(&json!({"pong": true})));
            assert!(res.error().is_none());
        }
        _ => panic!("expected JsonRpcMessage::Response"),
    }

    let response_err = b"{\"jsonrpc\":\"2.0\",\"id\":10,\"error\":{\"code\":-32601,\"message\":\"Method not found\"}}\n";
    let decoded_err = codec
        .decode_message(response_err)
        .expect("decoding error response must succeed");

    match decoded_err {
        JsonRpcMessage::Response(res) => {
            assert_eq!(res.id(), &JsonRpcId::Number(10));
            assert!(!res.is_success());
            let err = res.error().expect("response must contain jsonrpc error");
            assert_eq!(err.code(), -32601);
            assert_eq!(err.message(), "Method not found");
        }
        _ => panic!("expected JsonRpcMessage::Response"),
    }
}

#[test]
fn test_framing_error_on_invalid_or_incomplete_payload() {
    let codec = StdioFrameCodec::new();

    let malformed_json = b"{\"jsonrpc\":\"2.0\",\"id\": incomplete\n";
    let err = codec
        .decode_message(malformed_json)
        .expect_err("malformed json must return framing error");

    match err {
        AcpError::Framing(FramingError::InvalidPayload(_)) => {}
        other => panic!("expected AcpError::Framing(InvalidPayload), got: {other:?}"),
    }

    let non_jsonrpc = b"{\"foo\":\"bar\"}\n";
    let non_rpc_err = codec
        .decode_message(non_jsonrpc)
        .expect_err("missing jsonrpc envelope must return framing error");

    match non_rpc_err {
        AcpError::Framing(FramingError::ProtocolViolation(_)) => {}
        other => panic!("expected AcpError::Framing(ProtocolViolation), got: {other:?}"),
    }
}
