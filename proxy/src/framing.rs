//! LSP base protocol: `Content-Length`-framed JSON messages.
//!
//! A [`Message`] keeps its raw bytes and only decodes the envelope (`method`, `id`);
//! the full JSON is parsed on demand, so messages the proxy merely forwards are
//! written out unchanged without being decoded or re-encoded.

use crate::lsp::RequestId;
use serde::Deserialize;
use serde_json::Value;
use std::io::{self, BufRead, Write};

pub struct Message {
    raw: Vec<u8>,
    method: Option<String>,
    id: Option<Value>,
}

#[derive(Deserialize)]
struct Envelope {
    method: Option<String>,
    id: Option<Value>,
}

impl Message {
    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }

    /// The id of a response (a message with an id and no method).
    pub fn response_id(&self) -> Option<RequestId> {
        self.method.is_none().then_some(())?;
        self.id.as_ref().map(RequestId::of)
    }

    /// The request id of a request.
    pub fn request_id(&self) -> Option<&Value> {
        self.method.as_ref().and(self.id.as_ref())
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Parses the whole message.
    pub fn value(&self) -> Value {
        serde_json::from_slice(&self.raw).unwrap_or_default()
    }
}

/// Reads one message. Returns `Ok(None)` on clean EOF before a header.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Message>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    let mut saw_header = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return if saw_header {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF inside LSP header",
                ))
            } else {
                Ok(None)
            };
        }
        saw_header = true;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length")
            })?);
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut raw = vec![0; len];
    reader.read_exact(&mut raw)?;
    let envelope: Envelope =
        serde_json::from_slice(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(Message {
        raw,
        method: envelope.method,
        id: envelope.id,
    }))
}

/// Writes a body with its header in a single write.
pub fn write_raw(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut frame = Vec::with_capacity(header.len() + body.len());
    frame.extend_from_slice(header.as_bytes());
    frame.extend_from_slice(body);
    writer.write_all(&frame)?;
    writer.flush()
}

pub fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    write_raw(writer, &serde_json::to_vec(message)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn read(reader: &mut impl BufRead) -> Option<Value> {
        read_message(reader).unwrap().map(|m| m.value())
    }

    #[test]
    fn roundtrip_multiple_messages() {
        let mut buf = Vec::new();
        let a = json!({"jsonrpc": "2.0", "id": 1, "method": "a", "params": {"s": "∀ x, x = x"}});
        let b = json!({"jsonrpc": "2.0", "method": "b"});
        write_message(&mut buf, &a).unwrap();
        write_message(&mut buf, &b).unwrap();
        let mut reader = Cursor::new(buf);
        assert_eq!(read(&mut reader), Some(a));
        assert_eq!(read(&mut reader), Some(b));
        assert_eq!(read(&mut reader), None);
    }

    #[test]
    fn envelope_without_full_parse() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"id": 5, "result": {"big": [1, 2, 3]}})).unwrap();
        write_message(&mut buf, &json!({"id": 6, "method": "m", "params": {}})).unwrap();
        let mut reader = Cursor::new(buf);
        let response = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(response.response_id(), Some(RequestId::of(&json!(5))));
        assert_eq!(response.request_id(), None);
        let request = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(request.response_id(), None);
        assert_eq!(request.request_id(), Some(&json!(6)));
        assert_eq!(request.method(), Some("m"));
    }

    #[test]
    fn accepts_extra_headers_and_case() {
        let body = r#"{"x":1}"#;
        let raw = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{body}",
            body.len()
        );
        assert_eq!(
            read(&mut Cursor::new(raw.into_bytes())),
            Some(json!({"x": 1}))
        );
    }

    #[test]
    fn missing_length_is_error() {
        let mut reader = Cursor::new(b"Foo: bar\r\n\r\n{}".to_vec());
        assert!(read_message(&mut reader).is_err());
    }
}
