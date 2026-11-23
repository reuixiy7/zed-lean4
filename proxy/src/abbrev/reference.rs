//! An on-demand Markdown reference, opened through an LSP command.

use super::table::ABBREVIATIONS;
use crate::shared::Shared;
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

const FILE_NAME: &str = "AbbreviationView.md";

pub fn prepare(shared: &Shared) -> io::Result<PathBuf> {
    let path = shared.config.goals_file.with_file_name(FILE_NAME);
    write(shared, &path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Cannot write {}: {error}", path.display()),
        )
    })?;
    Ok(path)
}

fn write(shared: &Shared, path: &std::path::Path) -> io::Result<()> {
    // A custom infoview filename must not be overwritten by the reference.
    if shared
        .config
        .goals_file
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(FILE_NAME))
    {
        return Err(io::Error::other(
            "the abbreviation reference and infoview have the same filename; change infoview.file",
        ));
    }
    let text = markdown();
    match std::fs::read(path) {
        Ok(previous) if previous == text.as_bytes() => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

fn markdown() -> &'static str {
    // The embedded data cannot change during this process. Render only on the
    // first explicit request, including when automatic input is disabled.
    static MARKDOWN: OnceLock<String> = OnceLock::new();
    MARKDOWN.get_or_init(|| {
        render(
            &serde_json::from_str(ABBREVIATIONS).expect("invalid embedded Lean abbreviation table"),
        )
    })
}

fn render(table: &BTreeMap<String, String>) -> String {
    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (abbreviation, symbol) in table {
        grouped.entry(symbol).or_default().push(abbreviation);
    }
    for abbreviations in grouped.values_mut() {
        abbreviations.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    }
    let mut out = String::from("| Abbreviation | Unicode symbol |\n| --- | --- |\n");
    for (symbol, abbreviations) in grouped {
        let aliases = abbreviations
            .iter()
            .map(|abbreviation| cell(&format!("\\{abbreviation}")))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("| {aliases} | {} |\n", cell(symbol)));
    }
    out
}

/// Pipes split Markdown table cells even inside code spans; backticks need a
/// longer fence. Padding also preserves abbreviations that start with a backtick.
fn cell(text: &str) -> String {
    let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    format!("{fence} {} {fence}", text.replace('|', "\\|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_is_one_table_with_every_alias_and_original_cursor_markers() {
        let table: BTreeMap<String, String> = serde_json::from_str(ABBREVIATIONS).unwrap();
        let text = markdown();
        assert!(text.starts_with("| Abbreviation | Unicode symbol |\n| --- | --- |\n"));
        assert_eq!(text.matches("| Abbreviation | Unicode symbol |").count(), 1);
        assert!(
            text.lines()
                .all(|line| line.starts_with("| ") && line.ends_with(" |"))
        );
        for (abbreviation, symbol) in &table {
            let row = text.lines().find(|line| {
                line.ends_with(&format!(" | {} |", cell(symbol)))
                    && line.contains(&cell(&format!("\\{abbreviation}")))
            });
            assert!(row.is_some(), "missing {abbreviation} → {symbol}");
        }
        assert!(text.contains("` \\R `, ` \\br `, ` \\bbR `, ` \\Real `, ` \\real `"));
        assert!(text.contains(" | ` ⟨$CURSOR⟩ ` |"));
        assert!(text.contains(" | ` {$CURSOR} ` |"));
        assert_eq!(cell("\\|"), "` \\\\| `");
        assert_eq!(cell("\\`a"), "`` \\`a ``");
    }
}
