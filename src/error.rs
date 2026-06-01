//! Diagnostic types and error-reporting infrastructure.
//!
//! Every compiler stage produces `Diagnostic`s on failure. They are printed in
//! a Rust-like format with a caret pointing at the offending column.

use std::fmt::Write as _;

/// A single compiler error.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: String,        // e.g. "E201"
    pub message: String,     // human-readable explanation
    pub file: String,        // source file path
    pub line: usize,         // 1-based line number
    pub col: usize,          // 1-based column number
    pub source_line: String, // the full text of `line`
    /// Short message shown next to the caret (defaults to the main message if empty).
    pub label: String,
}

impl Diagnostic {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        line: usize,
        col: usize,
        source_line: impl Into<String>,
    ) -> Diagnostic {
        Diagnostic {
            code: code.into(),
            message: message.into(),
            file: file.into(),
            line,
            col,
            source_line: source_line.into(),
            label: String::new(),
        }
    }

    /// Set the short caret label (builder style).
    pub fn with_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label = label.into();
        self
    }

    /// Render the diagnostic to a printable string.
    ///
    /// ```text
    /// error[E201]: expected return type after ')'
    ///   --> src/main.jo:5:12
    ///    |
    ///  5 | fn foo(x: int) {
    ///    |               ^ expected return type
    /// ```
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "error[{}]: {}", self.code, self.message);
        let _ = writeln!(out, "  --> {}:{}:{}", self.file, self.line, self.col);

        // The gutter width is determined by the line number's digit count.
        let line_str = self.line.to_string();
        let gutter = line_str.len();
        let pad = " ".repeat(gutter);

        let _ = writeln!(out, "{} |", pad);
        let _ = writeln!(out, "{} | {}", line_str, self.source_line);

        // Build the caret line. Columns are 1-based; expand tabs in the prefix
        // to single spaces so the caret roughly lines up.
        let col = self.col.max(1);
        let mut caret_prefix = String::new();
        for (i, ch) in self.source_line.chars().enumerate() {
            if i + 1 >= col {
                break;
            }
            caret_prefix.push(if ch == '\t' { '\t' } else { ' ' });
        }
        let label = if self.label.is_empty() {
            &self.message
        } else {
            &self.label
        };
        let _ = writeln!(out, "{} | {}^ {}", pad, caret_prefix, label);
        out
    }
}

/// Print every diagnostic to stderr, separated by blank lines.
pub fn report_all(diags: &[Diagnostic]) {
    for d in diags {
        eprintln!("{}", d.render());
    }
    let n = diags.len();
    if n == 1 {
        eprintln!("error: aborting due to previous error");
    } else if n > 1 {
        eprintln!("error: aborting due to {} previous errors", n);
    }
}

/// Helper: extract the 1-based `line`'s text from `src`, or "" if out of range.
pub fn source_line_of(src: &str, line: usize) -> String {
    if line == 0 {
        return String::new();
    }
    src.lines().nth(line - 1).unwrap_or("").to_string()
}
