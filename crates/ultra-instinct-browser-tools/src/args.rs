//! Argument readers every `browser_*` tool shares: one wording for a
//! missing or mistyped argument.

use serde_json::Value;

use crate::error::ToolError;

pub(crate) fn opt_str<'a>(arguments: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "`{key}` must be a string, got {other}"
        ))),
    }
}

pub(crate) fn req_str<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    opt_str(arguments, key)?
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing required `{key}`")))
}

/// A pointer press (`Input::Click`) unless `mouse: false` asks for a script click.
pub(crate) fn pointer_press(arguments: &Value) -> Result<bool, ToolError> {
    Ok(opt_bool(arguments, "mouse")?.unwrap_or(true))
}

/// Schema of a press's `mouse` argument.
pub(crate) fn mouse_prop() -> Value {
    serde_json::json!({"type": "boolean", "description": "Default true: a real pointer press at the element's center (moves focus, fires pointerdown / mousedown). false: a DOM script click (element.click())."})
}

pub(crate) fn opt_bool(arguments: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "`{key}` must be a boolean, got {other}"
        ))),
    }
}

pub(crate) fn opt_usize(arguments: &Value, key: &str) -> Result<Option<usize>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|v| Some(usize::try_from(v).unwrap_or(usize::MAX)))
            .ok_or_else(|| {
                ToolError::InvalidArguments(format!("`{key}` must be a non-negative integer"))
            }),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "`{key}` must be a non-negative integer, got {other}"
        ))),
    }
}

pub(crate) fn browser_err(err: impl ToString) -> ToolError {
    ToolError::Browser(err.to_string())
}
