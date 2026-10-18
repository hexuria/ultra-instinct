//! Ordered CDP replay. Each [`CdpTransport::call`] consumes the next scripted
//! step. A live socket uses the same method names and JSON params.

use serde_json::Value;

use crate::{CdpError, CdpTransport};

#[derive(Clone, Debug)]
struct Step {
    method: String,
    params: Option<Value>,
    outcome: Outcome,
}

#[derive(Clone, Debug)]
enum Outcome {
    Result(Value),
    Protocol(String),
}

/// In-memory CDP peer. Tests and `--fixture` use this. Browser sessions use
/// whichever [`CdpTransport`] implementation they are given.
#[derive(Clone, Debug)]
pub struct ReplayTransport {
    steps: Vec<Step>,
    cursor: usize,
    log: Vec<String>,
    params_log: Vec<(String, String)>,
}

impl ReplayTransport {
    pub fn parse(text: &str) -> Result<Self, CdpError> {
        let mut transport = Self {
            steps: Vec::new(),
            cursor: 0,
            log: Vec::new(),
            params_log: Vec::new(),
        };
        transport.append(text)?;
        Ok(transport)
    }

    pub fn append(&mut self, text: &str) -> Result<(), CdpError> {
        let root: Value = serde_json::from_str(text).map_err(|err| CdpError::BadScript {
            message: err.to_string(),
        })?;
        let calls =
            root.get("calls")
                .and_then(Value::as_array)
                .ok_or_else(|| CdpError::BadScript {
                    message: "script must be an object with a calls array".into(),
                })?;
        for call in calls {
            let method = call
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| CdpError::BadScript {
                    message: "call is missing method".into(),
                })?
                .to_owned();
            let params = call.get("params").cloned();
            if let Some(params) = &params {
                if !params.is_object() {
                    return Err(CdpError::BadScript {
                        message: format!("params for `{method}` must be a JSON object"),
                    });
                }
            }
            let has_result = call.get("result").is_some();
            let has_error = call.get("error").is_some();
            if has_result == has_error {
                return Err(CdpError::BadScript {
                    message: format!("`{method}` must set exactly one of result or error"),
                });
            }
            let outcome = if has_error {
                let message = match call.get("error") {
                    Some(Value::String(message)) => message.clone(),
                    Some(other) => other.to_string(),
                    None => unreachable!("error presence checked"),
                };
                Outcome::Protocol(message)
            } else {
                Outcome::Result(call.get("result").cloned().unwrap_or(Value::Null))
            };
            self.steps.push(Step {
                method,
                params,
                outcome,
            });
        }
        Ok(())
    }

    pub fn logged_methods(&self) -> &[String] {
        &self.log
    }

    /// `(method, params_json)` of every consumed step, in call order.
    pub fn logged_calls(&self) -> &[(String, String)] {
        &self.params_log
    }

    /// Scripted steps not consumed yet.
    pub fn remaining(&self) -> usize {
        self.steps.len().saturating_sub(self.cursor)
    }
}

impl CdpTransport for ReplayTransport {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let Some(step) = self.steps.get(self.cursor) else {
            return Err(CdpError::NoScriptedResponse {
                method: method.to_owned(),
            });
        };
        if step.method != method {
            return Err(CdpError::NoScriptedResponse {
                method: method.to_owned(),
            });
        }
        if let Some(expected) = &step.params {
            let actual: Value =
                serde_json::from_str(params_json).map_err(|err| CdpError::BadJson {
                    message: err.to_string(),
                })?;
            if &actual != expected {
                return Err(CdpError::ParamsMismatch {
                    method: method.to_owned(),
                });
            }
        }
        let outcome = step.outcome.clone();
        self.cursor += 1;
        self.log.push(method.to_owned());
        self.params_log
            .push((method.to_owned(), params_json.to_owned()));
        match outcome {
            Outcome::Result(value) => Ok(value.to_string()),
            Outcome::Protocol(message) => Err(CdpError::Protocol { message }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_call_and_bad_script_are_exact_errors() {
        let err = ReplayTransport::parse("{}").unwrap_err();
        assert_eq!(
            err,
            CdpError::BadScript {
                message: "script must be an object with a calls array".into(),
            }
        );
        let err = ReplayTransport::parse(
            r#"{"calls":[{"method":"Page.getLayoutMetrics","result":{},"error":"x"}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, CdpError::BadScript { .. }), "{err}");

        let mut transport = ReplayTransport::parse(
            r#"{"calls":[{"method":"Page.getLayoutMetrics","result":{"ok":true}}]}"#,
        )
        .unwrap();
        let err = transport.call("DOM.getDocument", "{}").unwrap_err();
        assert_eq!(
            err,
            CdpError::NoScriptedResponse {
                method: "DOM.getDocument".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "no scripted CDP response for `DOM.getDocument`"
        );
        assert!(transport.logged_methods().is_empty());
    }

    #[test]
    fn bad_json_and_params_mismatch_are_exact() {
        let mut transport = ReplayTransport::parse(
            r#"{"calls":[{"method":"Page.getLayoutMetrics","params":{},"result":{}}]}"#,
        )
        .unwrap();
        let err = transport
            .call("Page.getLayoutMetrics", "not-json")
            .unwrap_err();
        let CdpError::BadJson { message } = &err else {
            panic!("expected BadJson, got {err}");
        };
        assert!(!message.is_empty());
        assert_eq!(err.to_string(), format!("invalid CDP JSON: {message}"));
        assert!(transport.logged_methods().is_empty());

        let err = transport
            .call("Page.getLayoutMetrics", r#"{"extra":1}"#)
            .unwrap_err();
        assert_eq!(
            err,
            CdpError::ParamsMismatch {
                method: "Page.getLayoutMetrics".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "scripted CDP params do not match the call to `Page.getLayoutMetrics`"
        );
        assert!(transport.logged_methods().is_empty());
    }

    #[test]
    fn proptest_parser_does_not_panic() {
        // Owned here rather than a second grammar. Garbage is an error or a script.
        let samples = [
            "",
            "{",
            "[]",
            "null",
            "{\"calls\":[]}",
            "@@@",
            "{\"calls\":[1]}",
        ];
        for sample in samples {
            let _ = ReplayTransport::parse(sample);
        }
    }
}
