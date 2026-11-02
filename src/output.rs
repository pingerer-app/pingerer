//! What commands print: aligned tables and status words in color for
//! people, or JSON with --json. Color only on a terminal, and never with
//! NO_COLOR set (https://no-color.org) or --no-color.

use serde_json::Value;
use std::io::Write;

pub struct Output {
    out: Box<dyn Write>,
    err: Box<dyn Write>,
    pub json: bool,
    /// None: decide from out_tty and NO_COLOR.
    pub color: Option<bool>,
    default_color: bool,
}

fn code(color: &str) -> Option<u8> {
    Some(match color {
        "red" => 31,
        "green" => 32,
        "yellow" => 33,
        "blue" => 34,
        "gray" => 90,
        "bold" => 1,
        _ => return None,
    })
}

fn status_color(status: &str) -> &'static str {
    match status {
        "up" | "ok" | "resolved" => "green",
        "degraded" | "acknowledged" => "yellow",
        "pending" | "paused" => "gray",
        "down" | "open" | "failed" => "red",
        _ => "bold",
    }
}

impl Output {
    pub fn new(out: Box<dyn Write>, err: Box<dyn Write>, out_tty: bool, no_color_env: bool) -> Self {
        Output { out, err, json: false, color: None, default_color: out_tty && !no_color_env }
    }

    pub fn color(&self) -> bool {
        self.color.unwrap_or(self.default_color)
    }

    pub fn puts(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.out, "{}", text.as_ref());
    }

    pub fn print(&mut self, text: impl AsRef<str>) {
        let _ = write!(self.out, "{}", text.as_ref());
        let _ = self.out.flush();
    }

    pub fn warn(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.err, "{}", text.as_ref());
    }

    pub fn json_value(&mut self, data: &Value) {
        let text = serde_json::to_string_pretty(data).unwrap_or_default();
        self.puts(text);
    }

    pub fn paint(&self, text: impl AsRef<str>, color: &str) -> String {
        match code(color) {
            Some(code) if self.color() => format!("\x1b[{code}m{}\x1b[0m", text.as_ref()),
            _ => text.as_ref().to_string(),
        }
    }

    pub fn status(&self, value: impl AsRef<str>) -> String {
        self.paint(value.as_ref(), status_color(value.as_ref()))
    }

    pub fn bold(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "bold")
    }

    pub fn muted(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "gray")
    }

    /// rows: cells that may already be painted. Columns are padded to the
    /// widest visible cell; the last one isn't.
    pub fn table(&mut self, headers: &[&str], rows: Vec<Vec<String>>, empty: &str) {
        if rows.is_empty() {
            let text = self.muted(empty);
            return self.puts(text);
        }
        let mut all: Vec<Vec<String>> = vec![headers.iter().map(|it| self.bold(it)).collect()];
        all.extend(rows);
        let widths: Vec<usize> = (0..headers.len()).map(|index| all.iter().map(|row| visible_length(row.get(index).map_or("", String::as_str))).max().unwrap_or(0)).collect();
        for row in &all {
            let line: Vec<String> = row.iter().enumerate().map(|(index, cell)| {
                if index == row.len() - 1 { cell.clone() } else { format!("{cell}{}", " ".repeat(widths[index].saturating_sub(visible_length(cell)))) }
            }).collect();
            let line = line.join("  ");
            self.puts(line.trim_end());
        }
    }

    /// "Label  value" lines.
    pub fn fields(&mut self, pairs: Vec<(String, String)>) {
        let width = pairs.iter().map(|(label, _)| label.chars().count()).max().unwrap_or(0);
        for (label, value) in pairs {
            let padded = format!("{label}{}", " ".repeat(width - label.chars().count()));
            let line = format!("{}  {value}", self.muted(padded));
            self.puts(line);
        }
    }
}

/// Characters, not counting color codes.
pub fn visible_length(text: &str) -> usize {
    let mut length = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next == 'm' {
                    break;
                }
            }
        } else {
            length += 1;
        }
    }
    length
}
