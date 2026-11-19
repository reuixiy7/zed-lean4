//! The LSP shapes the proxy works with: request ids, positions, diagnostics, and
//! JSON-RPC envelopes. Everything else is forwarded without being looked at.

use serde_json::{Value, json};
use std::fmt;

/// Prefix of ids of requests sent by the proxy itself, so they never collide with
/// ids chosen by Zed or by the Lean server.
pub const OWN_ID_PREFIX: &str = "infoview-";

/// A JSON-RPC request id (number or string), usable as a map key.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    pub fn of(id: &Value) -> Self {
        Self(id.to_string())
    }

    pub fn own(n: u64) -> Self {
        Self::of(&Value::String(format!("{OWN_ID_PREFIX}{n}")))
    }

    pub fn to_value(&self) -> Value {
        serde_json::from_str(&self.0).unwrap_or(Value::Null)
    }
}

impl fmt::Debug for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A zero-based position; `character` counts UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u64,
    pub character: u64,
}

impl Position {
    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            line: value.get("line")?.as_u64()?,
            character: value.get("character")?.as_u64()?,
        })
    }

    pub fn to_json(self) -> Value {
        json!({"line": self.line, "character": self.character})
    }
}

/// A document and a position in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub uri: String,
    pub pos: Position,
}

impl Cursor {
    /// `TextDocumentPositionParams`.
    pub fn params(&self) -> Value {
        json!({"textDocument": {"uri": self.uri}, "position": self.pos.to_json()})
    }
}

/// The part of a diagnostic the infoview shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub start: Position,
    pub end_line: u64,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl Diagnostic {
    pub fn from_json(value: &mut Value) -> Option<Self> {
        let start = Position::from_json(value.pointer("/range/start")?)?;
        let end_line = value.pointer("/range/end/line")?.as_u64()?;
        let severity = match value.get("severity").and_then(Value::as_u64) {
            Some(1) => Severity::Error,
            Some(2) => Severity::Warning,
            Some(4) => Severity::Hint,
            _ => Severity::Information,
        };
        let message = match value.get_mut("message")?.take() {
            Value::String(message) => message,
            _ => return None,
        };
        Some(Self {
            severity,
            start,
            end_line,
            message,
        })
    }

    pub fn covers_line(&self, line: u64) -> bool {
        self.start.line <= line && line <= self.end_line
    }
}

pub fn range(start: Position, end: Position) -> Value {
    json!({"start": start.to_json(), "end": end.to_json()})
}

/// `params.textDocument.uri`.
pub fn text_document_uri(params: &Value) -> Option<&str> {
    params.pointer("/textDocument/uri")?.as_str()
}

pub fn request(id: &RequestId, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id.to_value(), "method": method, "params": params})
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_distinguish_numbers_and_strings() {
        assert_ne!(RequestId::of(&json!(1)), RequestId::of(&json!("1")));
        assert_eq!(RequestId::own(7).to_value(), json!("infoview-7"));
        assert_eq!(RequestId::of(&json!(3)).to_value(), json!(3));
    }

    #[test]
    fn diagnostic_from_json() {
        let mut d = json!({"severity": 2, "message": "m",
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 3, "character": 0}}});
        let d = Diagnostic::from_json(&mut d).unwrap();
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(
            d.start,
            Position {
                line: 1,
                character: 2
            }
        );
        assert!(d.covers_line(3) && !d.covers_line(4));
        assert_eq!(d.message, "m");
    }
}
