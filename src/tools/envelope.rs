//! The `ok`-shaped JSON envelope every tool answers with.

use crate::validation;
use rmcp::model::{CallToolResponse, ContentBlock};
use serde_json::{Value, json};
use std::path::Path;

/// Build an `ok:false` result JSON (the standard error envelope).
pub(super) fn error_result(error_code: &str, message: String) -> Value {
    json!({ "ok": false, "error_code": error_code, "message": message })
}

/// Serialize a validation error into the `ok:false` content envelope.
pub(super) fn validation_error(v: &validation::ValidationError) -> String {
    error_result(v.error_code, v.message.clone()).to_string()
}

pub(super) fn running_job(
    id: u64,
    command: &str,
    build_dir: Option<&str>,
    run_ms: u128,
    log: Option<&Path>,
    log_bytes: Option<u64>,
) -> String {
    let mut payload = json!({"ok":true,"status":"running","job_id":id,"run_ms":run_ms});
    let object = payload.as_object_mut().expect("object envelope");
    if let Some(build_dir) = build_dir {
        object.insert("command".into(), json!(command));
        object.insert("build_dir".into(), json!(build_dir));
    }
    if let Some(log) = log {
        object.insert("log".into(), json!(log.to_string_lossy()));
        if let Some(bytes) = log_bytes {
            object.insert("log_bytes".into(), json!(bytes));
        }
    }
    Value::Object(payload.as_object().cloned().unwrap_or_default()).to_string()
}

pub(super) fn finished_job(id: u64, done: &crate::jobs::Done) -> String {
    if done.cancelled {
        return json!({"ok":true,"status":"cancelled","job_id":id,
            "command":done.command,"run_ms":done.run_ms})
        .to_string();
    }
    let mut payload = serde_json::from_str::<Value>(&done.json).unwrap_or_else(|_| {
        error_result("BUILD_FAILURE", "job result could not be decoded".into())
    });
    if let Value::Object(object) = &mut payload {
        object.insert("status".into(), json!("finished"));
        object.insert("job_id".into(), json!(id));
        object.insert("run_ms".into(), json!(done.run_ms));
    }
    payload.to_string()
}

pub(super) fn cancelled_job_wait(id: u64, done: &crate::jobs::Done) -> String {
    json!({"ok":false,"error_code":"CANCELLED",
        "message":format!("job {id} was cancelled"),"status":"cancelled",
        "job_id":id,"command":done.command,"run_ms":done.run_ms})
    .to_string()
}

pub(super) fn busy_job(build_dir: &str, id: u64) -> String {
    error_result(
        "BUSY",
        format!(
            "build dir {build_dir} has running background job {id}; use job_wait or job_cancel"
        ),
    )
    .as_object()
    .map(|object| {
        let mut object = object.clone();
        object.insert("job_id".into(), json!(id));
        Value::Object(object).to_string()
    })
    .unwrap_or_default()
}

pub(super) fn missing_job(id: u64) -> String {
    error_result(
        "NOT_FOUND",
        format!("no job {id} (unknown, or evicted after 16 newer finished jobs)"),
    )
    .to_string()
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
