//! Ordered CDP replay. Each [`CdpTransport::call`] consumes the next scripted
//! step. A live socket uses the same method names and JSON params.

use std::collections::BTreeMap;

use serde_json::value::RawValue;
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
    /// Raw `result` JSON, returned verbatim. A recorded page's
    /// `DOM.getDocument` tree nests past `serde_json`'s 128-level `Value`
    /// cap, so a scripted result keeps its text rather than materializing —
    /// the same split the websocket transport makes in `ws::frame_fields`.
    Result(String),
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
        let root: BTreeMap<String, Box<RawValue>> =
            serde_json::from_str(text).map_err(|err| CdpError::BadScript {
                message: err.to_string(),
            })?;
        let calls = match root.get("calls") {
            Some(calls) if calls.get().trim_start().starts_with('[') => calls,
            _ => {
                return Err(CdpError::BadScript {
                    message: "script must be an object with a calls array".into(),
                })
            }
        };
        let calls: Vec<Box<RawValue>> =
            serde_json::from_str(calls.get()).map_err(|err| CdpError::BadScript {
                message: err.to_string(),
            })?;
        for raw_call in calls {
            // A non-object element decodes to an empty map and reports a
            // missing-method error.
            let call: BTreeMap<String, Box<RawValue>> =
                serde_json::from_str(raw_call.get()).unwrap_or_default();
            let method = call
                .get("method")
                .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
                .ok_or_else(|| CdpError::BadScript {
                    message: "call is missing method".into(),
                })?;
            let params = call
                .get("params")
                .map(|raw| serde_json::from_str::<Value>(raw.get()))
                .transpose()
                .map_err(|err| CdpError::BadScript {
                    message: err.to_string(),
                })?;
            if let Some(params) = &params {
                if !params.is_object() {
                    return Err(CdpError::BadScript {
                        message: format!("params for `{method}` must be a JSON object"),
                    });
                }
            }
            let has_result = call.contains_key("result");
            let has_error = call.contains_key("error");
            if has_result == has_error {
                return Err(CdpError::BadScript {
                    message: format!("`{method}` must set exactly one of result or error"),
                });
            }
            let outcome = if has_error {
                let error: Value =
                    serde_json::from_str(call.get("error").expect("error presence checked").get())
                        .map_err(|err| CdpError::BadScript {
                            message: err.to_string(),
                        })?;
                let message = match error {
                    Value::String(message) => message,
                    other => other.to_string(),
                };
                Outcome::Protocol(message)
            } else {
                Outcome::Result(
                    call.get("result")
                        .expect("result presence checked")
                        .get()
                        .to_owned(),
                )
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
            Outcome::Result(text) => Ok(text),
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
    fn deep_scripted_result_round_trips_verbatim() {
        // A recorded DOM.getDocument result nests past Value's 128-level
        // cap; the step keeps its raw text and call() hands it back
        // untouched so the extract layer's deep parser sees it whole.
        let mut tree = String::from("null");
        for _ in 0..150 {
            tree = format!("{{\"children\":[{tree}]}}");
        }
        let result_text = format!("{{\"root\":{tree}}}");
        assert!(serde_json::from_str::<Value>(&result_text).is_err());
        let text =
            format!("{{\"calls\":[{{\"method\":\"DOM.getDocument\",\"result\":{result_text}}}]}}");
        let mut transport = ReplayTransport::parse(&text).unwrap();
        assert_eq!(
            transport.call("DOM.getDocument", "{}").unwrap(),
            result_text
        );

        // `result: null` is a present result and answers "null".
        let mut transport =
            ReplayTransport::parse(r#"{"calls":[{"method":"Runtime.evaluate","result":null}]}"#)
                .unwrap();
        assert_eq!(transport.call("Runtime.evaluate", "{}").unwrap(), "null");
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
