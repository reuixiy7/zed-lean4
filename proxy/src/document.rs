//! Mirrors of the open documents, kept in sync from `didOpen` / `didChange` / `didClose`.

use crate::lsp::{Position, text_document_uri};
use serde_json::Value;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

mod lex;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    start: usize,
    context: lex::Context,
}

#[derive(Clone)]
pub struct Document {
    pub text: String,
    pub version: i64,
    lines: Vec<Line>,
}

impl Document {
    pub fn new(text: String, version: i64) -> Self {
        let lines = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .map(|start| Line {
                start,
                context: Default::default(),
            })
            .collect();
        let mut doc = Self {
            text,
            version,
            lines,
        };
        doc.refresh_context(0, doc.lines.len());
        doc
    }

    pub fn line_range(&self, line: u64) -> Range<usize> {
        let Ok(line) = usize::try_from(line) else {
            return self.text.len()..self.text.len();
        };
        let Some(start) = self.lines.get(line).map(|l| l.start) else {
            return self.text.len()..self.text.len();
        };
        let end = self
            .lines
            .get(line + 1)
            .map_or(self.text.len(), |l| l.start - 1);
        start..end
    }

    pub fn line(&self, line: u64) -> &str {
        &self.text[self.line_range(line)]
    }

    pub fn offset(&self, pos: Position) -> usize {
        let range = self.line_range(pos.line);
        range.start + column_offset(&self.text[range], pos.character)
    }

    pub fn allows_abbreviation(&self, line: u64, byte: usize) -> bool {
        let Some(checkpoint) = usize::try_from(line).ok().and_then(|i| self.lines.get(i)) else {
            return false;
        };
        let mut context = checkpoint.context.clone();
        context.scan(&self.line(line)[..byte]);
        context.allows_abbreviation()
    }

    fn replace(&mut self, range: Range<usize>, text: &str) {
        let first = self.lines.partition_point(|l| l.start <= range.start) - 1;
        let last = self.lines.partition_point(|l| l.start <= range.end) - 1;
        let inserted: Vec<_> = text
            .match_indices('\n')
            .map(|(i, _)| Line {
                start: range.start + i + 1,
                context: Default::default(),
            })
            .collect();
        let changed_last = first + inserted.len();
        let removed = range.end - range.start;
        self.lines.splice(first + 1..last + 1, inserted);
        for line in &mut self.lines[changed_last + 1..] {
            line.start = line.start - removed + text.len();
        }
        self.text.replace_range(range, text);
        self.refresh_context(first, changed_last);
    }

    /// Re-lex changed lines, then stop as soon as an unchanged checkpoint agrees.
    fn refresh_context(&mut self, first: usize, changed_last: usize) {
        let mut context = self.lines[first].context.clone();
        for i in first..self.lines.len().saturating_sub(1) {
            context.scan(&self.text[self.lines[i].start..self.lines[i + 1].start]);
            if i >= changed_last && self.lines[i + 1].context == context {
                break;
            }
            self.lines[i + 1].context = context.clone();
        }
    }
}

/// The last change of a `didChange`, if it inserted text without replacing anything.
pub struct Insertion {
    pub start: Position,
    pub text: String,
}

#[derive(Default)]
pub struct Documents {
    docs: HashMap<String, Arc<Document>>,
}

impl Documents {
    pub fn get(&self, uri: &str) -> Option<&Arc<Document>> {
        self.docs.get(uri)
    }

    pub fn open(&mut self, params: &Value) -> Option<()> {
        let doc = params.get("textDocument")?;
        self.docs.insert(
            doc.get("uri")?.as_str()?.to_owned(),
            Arc::new(Document::new(
                doc.get("text")?.as_str()?.to_owned(),
                doc.get("version").and_then(Value::as_i64).unwrap_or(0),
            )),
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
        let doc = Arc::make_mut(self.docs.get_mut(text_document_uri(params)?)?);
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
                *doc = Document::new(text.to_owned(), doc.version);
                last = None;
                continue;
            };
            let start = range.get("start").and_then(Position::from_json);
            let end = range.get("end").and_then(Position::from_json);
            let (Some(start), Some(end)) = (start, end) else {
                last = None;
                continue;
            };
            let from = doc.offset(start);
            let to = if end == start {
                from
            } else {
                doc.offset(end).max(from)
            };
            doc.replace(from..to, text);
            last = (start == end && !text.is_empty()).then(|| Insertion {
                start,
                text: text.to_owned(),
            });
        }
        last
    }
}

/// Byte range of a line without its newline; clamped to the end of the text.
#[cfg(test)]
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
#[cfg(test)]
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

    #[test]
    fn incremental_index_and_context_match_a_fresh_document() {
        for text in [
            "a𝔸b\r\nℝx\n",
            "def s := \"one\ntwo\"\nx\n",
            "/- outer\n/- inner -/\n-/\nx\n",
            "r##\"one\n\"#\ntwo\"##\nx",
            "s!\"{\"inner\"}\nouter\"\nx",
        ] {
            let boundaries: Vec<_> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect();
            for (i, &start) in boundaries.iter().enumerate() {
                for &end in &boundaries[i..] {
                    for inserted in [
                        "", "x", "\n", "\n\n", "\"", "/-", "-/", "r#\"", "}\"", "𝔸\r\n",
                    ] {
                        let mut doc = Document::new(text.to_owned(), 1);
                        doc.replace(start..end, inserted);
                        let fresh = Document::new(doc.text.clone(), 1);
                        assert_eq!(
                            doc.lines, fresh.lines,
                            "{text:?}, {start}..{end}, {inserted:?}"
                        );
                        for line in 0..doc.lines.len() as u64 + 1 {
                            assert_eq!(doc.line_range(line), line_range(&doc.text, line));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn diagnostics_snapshots_survive_document_edits() {
        let uri = "file:///A.lean";
        let mut docs = Documents::default();
        docs.open(&json!({"textDocument": {"uri": uri, "text": "a\nb", "version": 1}}));
        let snapshot = docs.get(uri).unwrap().clone();
        docs.change(&json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": "x\ny\nz"}]}));
        assert_eq!(snapshot.text, "a\nb");
        assert_eq!(snapshot.version, 1);
        assert_eq!(docs.get(uri).unwrap().line(2), "z");
        assert_eq!(docs.get(uri).unwrap().version, 2);
    }
}
