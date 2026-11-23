//! The `ok`-shaped JSON envelope every tool answers with.

use crate::validation;
use rmcp::model::{CallToolResponse, ContentBlock};
use serde_json::{Value, json};

/// Build an `ok:false` result JSON (the standard error envelope).
pub(super) fn error_result(error_code: &str, message: String) -> Value {
    json!({ "ok": false, "error_code": error_code, "message": message })
}

/// Serialize a validation error into the `ok:false` content envelope.
pub(super) fn validation_error(v: &validation::ValidationError) -> String {
    error_result(v.error_code, v.message.clone()).to_string()
}

/// Fold the wall-clock duration of a tool call into its result payload, and
/// hold the payload to the `ok` envelope.
///
/// Every tool answers with a single JSON object, so the duration is added as an
/// `elapsed_ms` field on that object instead of a second content block: whoever
/// reads the payload sees how long the call took without special-casing a
/// second block. Text that is not a JSON object — rmcp's own "failed to
/// deserialize parameters" error — is wrapped as an `INVALID_ARGUMENT`
/// `ok:false` envelope, and an `ok:false` payload sets `isError`.
pub(super) fn with_elapsed_ms(response: CallToolResponse, elapsed_ms: u128) -> CallToolResponse {
    match response {
        CallToolResponse::Complete(mut result) => {
            if let Some(ContentBlock::Text(text)) = result.content.first_mut() {
                let mut payload = match serde_json::from_str::<Value>(&text.text) {
                    Ok(Value::Object(p)) => p,
                    _ if result.is_error == Some(true) => {
                        let Value::Object(p) =
                            error_result("INVALID_ARGUMENT", std::mem::take(&mut text.text))
                        else {
                            unreachable!("error_result builds an object")
                        };
                        p
                    }
                    _ => return CallToolResponse::Complete(result),
                };
                payload.insert("elapsed_ms".to_string(), json!(elapsed_ms));
                if payload.get("ok") == Some(&Value::Bool(false)) {
                    result.is_error = Some(true);
                }
                text.text = Value::Object(payload).to_string();
            }
            CallToolResponse::Complete(result)
        }
        // `CallToolResponse` is non-exhaustive; MRTR/task outcomes carry no
        // tool payload to annotate.
        other => other,
    }
}
