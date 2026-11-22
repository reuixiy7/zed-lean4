//! Mirrors of the open documents, kept in sync from `didOpen` / `didChange` / `didClose`.

use crate::lsp::{Position, text_document_uri};
use serde_json::Value;
use std::collections::HashMap;
use std::ops::Range;

pub struct Document {
    pub text: String,
    pub version: i64,
}

/// The last change of a `didChange`, if it inserted text without replacing anything.
pub struct Insertion {
    pub start: Position,
    pub text: String,
}

#[derive(Default)]
pub struct Documents {
    docs: HashMap<String, Document>,
}

impl Documents {
    pub fn get(&self, uri: &str) -> Option<&Document> {
        self.docs.get(uri)
    }

    pub fn open(&mut self, params: &Value) -> Option<()> {
        let doc = params.get("textDocument")?;
        self.docs.insert(
            doc.get("uri")?.as_str()?.to_owned(),
            Document {
                text: doc.get("text")?.as_str()?.to_owned(),
                version: doc.get("version").and_then(Value::as_i64).unwrap_or(0),
            },
        );
        Some(())
    }

    pub fn close(&mut self, params: &Value) {
        if let Some(uri) = text_document_uri(params) {
            self.docs.remove(uri);
        }
    }

    /// Applies a `didChange` and returns the last change if it was a pure insertion.
    pub fn change(&mut self, params: &Value) -> Option<Insertion> {
        let doc = self.docs.get_mut(text_document_uri(params)?)?;
        if let Some(version) = params
            .pointer("/textDocument/version")
            .and_then(Value::as_i64)
        {
            doc.version = version;
        }
        let mut last = None;
        for change in params.get("contentChanges")?.as_array()? {
            let text = change.get("text").and_then(Value::as_str).unwrap_or("");
            let Some(range) = change.get("range") else {
                doc.text = text.to_owned();
                last = None;
                continue;
            };
            let start = range.get("start").and_then(Position::from_json);
            let end = range.get("end").and_then(Position::from_json);
            let (Some(start), Some(end)) = (start, end) else {
                last = None;
                continue;
            };
            let from = offset(&doc.text, start);
            let to = if end == start {
                from
            } else {
                offset(&doc.text, end).max(from)
            };
            doc.text.replace_range(from..to, text);
            last = (start == end && !text.is_empty()).then(|| Insertion {
                start,
                text: text.to_owned(),
            });
        }
        last
    }
}

/// Byte range of a line without its newline; clamped to the end of the text.
pub fn line_range(text: &str, line: u64) -> Range<usize> {
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len()..text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    start..end
}

/// Byte offset of a UTF-16 column within a line; clamped to the line end.
pub fn column_offset(line: &str, character: u64) -> usize {
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units >= character {
            return i;
        }
        units += c.len_utf16() as u64;
    }
    line.len()
}

/// Byte offset of an LSP position.
pub fn offset(text: &str, pos: Position) -> usize {
    let line = line_range(text, pos.line);
    line.start + column_offset(&text[line], pos.character)
}

/// UTF-16 length of a string, for computing LSP characters.
pub fn utf16_len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pos(line: u64, character: u64) -> Position {
        Position { line, character }
    }

    #[test]
    fn offsets_count_utf16() {
        let text = "a𝔸b\nℝx";
        assert_eq!(offset(text, pos(0, 0)), 0);
        assert_eq!(offset(text, pos(0, 1)), 1);
        // 𝔸 is two UTF-16 units and four bytes.
        assert_eq!(offset(text, pos(0, 3)), 5);
        assert_eq!(offset(text, pos(0, 99)), 6);
        assert_eq!(offset(text, pos(1, 1)), 7 + 3);
        assert_eq!(offset(text, pos(9, 0)), text.len());
        assert_eq!(line_range(text, 1), 7..11);
    }

    #[test]
    fn applies_incremental_changes() {
        let mut docs = Documents::default();
        let uri = "file:///A.lean";
        docs.open(&json!({"textDocument": {"uri": uri, "text": "x : ℝ\n", "version": 1}}));
        let change = |version: i64, start: u64, end: u64, text: &str| {
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"range": {"start": {"line": 0, "character": start},
                                              "end": {"line": 0, "character": end}}, "text": text}]
            })
        };
        let ins = docs.change(&change(2, 5, 5, " \\R")).unwrap();
        assert_eq!(ins.start, pos(0, 5));
        assert_eq!(ins.text, " \\R");
        let doc = docs.get(uri).unwrap();
        assert_eq!(doc.text, "x : ℝ \\R\n");
        assert_eq!(doc.version, 2);

        // A replacement is not an insertion.
        assert!(docs.change(&change(3, 6, 8, "ℝ")).is_none());
        assert_eq!(docs.get(uri).unwrap().text, "x : ℝ ℝ\n");
    }
}
