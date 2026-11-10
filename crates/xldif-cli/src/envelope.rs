//! The JSON process contract (version 1), shared in shape with `dsd`.
//!
//! Every JSON invocation prints one newline-terminated object:
//! `{ok, command, result, warnings, error, metadata}`. `ok` is true exactly
//! when the exit status is 0. `error.code` is a stable category; messages
//! and hints are for people. Success envelopes arrive with the first command.

use serde_json::{Value, json};

pub const CONTRACT_VERSION: &str = "1";

/// A failed invocation.
#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
    usage: bool,
}

impl Failure {
    /// The arguments could not be parsed: exit status 2.
    pub fn usage(message: String, hint: String) -> Self {
        Self { code: "invalid_invocation", message, hint: Some(hint), usage: true }
    }

    pub const fn exit_code(&self) -> u8 {
        if self.usage { 2 } else { 1 }
    }
}

pub struct Envelope(Value);

fn metadata() -> Value {
    json!({"tool": "xldif", "version": crate::VERSION, "contract": CONTRACT_VERSION})
}

impl Envelope {
    pub fn failure(command: &str, failure: &Failure) -> Self {
        let mut error = json!({"code": failure.code, "message": failure.message});
        if let Some(hint) = &failure.hint {
            error["hint"] = json!(hint);
        }
        Self(json!({
            "ok": false,
            "command": if command.is_empty() { Value::Null } else { json!(command) },
            "result": Value::Null,
            "warnings": [],
            "error": error,
            "metadata": metadata(),
        }))
    }

    pub fn render(&self, pretty: bool) -> String {
        let mut text = if pretty { serde_json::to_string_pretty(&self.0) } else { serde_json::to_string(&self.0) }
            .expect("JSON values always serialize");
        text.push('\n');
        text
    }
}
