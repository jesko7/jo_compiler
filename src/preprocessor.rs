//! The Jo preprocessor.
//!
//! Runs on raw source *text* before lexing. Responsibilities:
//!   * Expand `import` declarations (named / aliased / glob / module).
//!   * Apply `#define NAME: value#` constant substitutions.
//!   * Apply `#define NAME … #` macro substitutions.
//!
//! For `import module;` (the module form) nothing is pasted, but the module's
//! *own* preprocessed source is recorded in `PreprocessOutput::modules` so the
//! type checker can resolve `module::name` references against it.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Diagnostic;

pub struct PreprocessOutput {
    /// The fully-expanded source text for the main file.
    pub source: String,
    /// Loaded modules (from `import module;`): module name → preprocessed source.
    pub modules: HashMap<String, String>,
}

pub struct Preprocessor {
    file: String,
    include_paths: Vec<PathBuf>,
    diags: Vec<Diagnostic>,
    modules: HashMap<String, String>,
    /// Module-file source cache so we only read/parse each `module.jo` once.
    module_cache: HashMap<String, ModuleFile>,
}

/// A parsed module file: its raw text plus a name→definition-text index.
struct ModuleFile {
    /// Each top-level definition's source text, in file order.
    defs: Vec<(String, String)>,
}

impl Preprocessor {
    /// `src` is the raw text of `file`. `include_paths` are the directories in
    /// which `module.jo` files are searched, tried in order.
    pub fn run(
        src: &str,
        file: impl Into<String>,
        include_paths: &[PathBuf],
    ) -> Result<PreprocessOutput, Vec<Diagnostic>> {
        let mut pp = Preprocessor {
            file: file.into(),
            include_paths: include_paths.to_vec(),
            diags: Vec::new(),
            modules: HashMap::new(),
            module_cache: HashMap::new(),
        };
        let expanded = pp.expand(src);
        if !pp.diags.is_empty() {
            return Err(pp.diags);
        }
        // Apply #define substitutions on the expanded text.
        let substituted = pp.apply_defines(&expanded);
        if !pp.diags.is_empty() {
            return Err(pp.diags);
        }
        Ok(PreprocessOutput {
            source: substituted,
            modules: pp.modules,
        })
    }

    // -----------------------------------------------------------------------
    // Import expansion
    // -----------------------------------------------------------------------

    /// Expand all `import` lines in `src`, returning the new text.
    fn expand(&mut self, src: &str) -> String {
        let mut out = String::new();
        let mut pasted_imports = String::new();
        let mut line_no = 0usize;
        for raw_line in src.lines() {
            line_no += 1;
            let trimmed = raw_line.trim();
            if let Some(rest) = trimmed.strip_prefix("import ") {
                self.handle_import(rest, line_no, raw_line, &mut pasted_imports);
                out.push('\n');
            } else if trimmed == "import" || trimmed.starts_with("import\t") {
                // `import` with non-space whitespace.
                let rest = trimmed["import".len()..].trim();
                self.handle_import(rest, line_no, raw_line, &mut pasted_imports);
                out.push('\n');
            } else {
                out.push_str(raw_line);
                out.push('\n');
            }
        }
        if !pasted_imports.is_empty() {
            out.push('\n');
            out.push_str(&pasted_imports);
        }
        out
    }

    fn handle_import(&mut self, rest: &str, line_no: usize, raw_line: &str, out: &mut String) {
        // Strip trailing ';'
        let spec = match rest.trim().strip_suffix(';') {
            Some(s) => s.trim(),
            None => {
                self.error(
                    "E001",
                    "import declaration must end with ';'",
                    line_no,
                    raw_line,
                );
                return;
            }
        };

        // Forms:
        //   module::name
        //   module::name as alias
        //   module::*
        //   module
        if let Some((module, tail)) = spec.split_once("::") {
            let module = module.trim();
            let tail = tail.trim();
            if tail == "*" {
                self.import_glob(module, line_no, raw_line, out);
            } else if let Some((name, alias)) = tail.split_once(" as ") {
                self.import_named(
                    module,
                    name.trim(),
                    Some(alias.trim()),
                    line_no,
                    raw_line,
                    out,
                );
            } else {
                // Could still be "name as alias" with odd spacing; handle "as".
                let mut parts = tail.split_whitespace();
                let name = parts.next().unwrap_or("");
                let alias = match parts.next() {
                    Some("as") => parts.next(),
                    Some(other) => {
                        self.error(
                            "E001",
                            format!("unexpected token `{}` in import", other),
                            line_no,
                            raw_line,
                        );
                        None
                    }
                    None => None,
                };
                self.import_named(module, name, alias, line_no, raw_line, out);
            }
        } else {
            // Module import: record it, paste nothing.
            let module = spec.trim();
            if module.is_empty() || !is_ident(module) {
                self.error("E001", "invalid module name in import", line_no, raw_line);
                return;
            }
            self.load_module_source(module, line_no, raw_line);
        }
    }

    fn import_named(
        &mut self,
        module: &str,
        name: &str,
        alias: Option<&str>,
        line_no: usize,
        raw_line: &str,
        out: &mut String,
    ) {
        let mf = match self.get_module(module, line_no, raw_line) {
            Some(mf) => mf,
            None => return,
        };
        let def_text = mf
            .defs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone());
        match def_text {
            Some(text) => {
                let pasted = match alias {
                    Some(a) => rename_definition(&text, name, a),
                    None => text,
                };
                out.push_str(&pasted);
                out.push('\n');
            }
            None => {
                self.error(
                    "E002",
                    format!("item `{}` not found in module `{}`", name, module),
                    line_no,
                    raw_line,
                );
            }
        }
    }

    fn import_glob(&mut self, module: &str, line_no: usize, raw_line: &str, out: &mut String) {
        let mf = match self.get_module(module, line_no, raw_line) {
            Some(mf) => mf,
            None => return,
        };
        for (_, text) in &mf.defs {
            out.push_str(text);
            out.push_str("\n\n");
        }
    }

    /// Record `import module;` — load and preprocess the module's own source so
    /// the type checker can resolve qualified `module::name` references.
    fn load_module_source(&mut self, module: &str, line_no: usize, raw_line: &str) {
        if self.modules.contains_key(module) {
            return;
        }
        let (text, _path) = match self.read_module(module) {
            Some(pair) => pair,
            None => {
                self.error(
                    "E003",
                    format!("module file `{}.jo` not found", module),
                    line_no,
                    raw_line,
                );
                return;
            }
        };
        // A module's own source may itself import / #define; recursively run the
        // preprocessor on it. Diagnostics are surfaced with the module's name.
        match Preprocessor::run(&text, format!("{}.jo", module), &self.include_paths) {
            Ok(output) => {
                self.modules.insert(module.to_string(), output.source);
                // Pull in any transitively-loaded modules.
                for (k, v) in output.modules {
                    self.modules.entry(k).or_insert(v);
                }
            }
            Err(mut ds) => self.diags.append(&mut ds),
        }
    }

    // -----------------------------------------------------------------------
    // Module file loading & indexing
    // -----------------------------------------------------------------------

    /// Read `module.jo` by searching each include path in order. Returns the
    /// file text and the path it was found at.
    fn read_module(&self, module: &str) -> Option<(String, PathBuf)> {
        for dir in &self.include_paths {
            let path = dir.join(format!("{}.jo", module));
            if let Ok(text) = std::fs::read_to_string(&path) {
                return Some((text, path));
            }
        }
        None
    }

    fn get_module(&mut self, module: &str, line_no: usize, raw_line: &str) -> Option<&ModuleFile> {
        if !self.module_cache.contains_key(module) {
            let text = match self.read_module(module) {
                Some((t, _)) => t,
                None => {
                    self.error(
                        "E003",
                        format!("module file `{}.jo` not found", module),
                        line_no,
                        raw_line,
                    );
                    return None;
                }
            };
            let defs = index_top_level_defs(&text);
            self.module_cache
                .insert(module.to_string(), ModuleFile { defs });
        }
        self.module_cache.get(module)
    }

    // -----------------------------------------------------------------------
    // #define substitution
    // -----------------------------------------------------------------------

    /// Scan for `#define` directives, build the substitution tables, and apply
    /// them to the remainder of the text. Constants and macros are both plain
    /// textual replacement of the whole-word NAME.
    fn apply_defines(&mut self, src: &str) -> String {
        let chars: Vec<char> = src.chars().collect();
        let mut i = 0usize;
        let mut line = 1usize;
        // (name, replacement) pairs, applied in declaration order.
        let mut subs: Vec<(String, String)> = Vec::new();
        let mut out = String::new();

        while i < chars.len() {
            // Detect a `#define` at the current position (start-of-token).
            if chars[i] == '#' && matches_keyword(&chars, i + 1, "define") {
                let define_line = line;
                // Advance past `#define` and following spaces (no newline).
                let mut j = i + 1 + "define".len();
                while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                    j += 1;
                }
                // Read NAME.
                let name_start = j;
                while j < chars.len() && is_ident_char_at(&chars, j) {
                    j += 1;
                }
                let name: String = chars[name_start..j].iter().collect();
                if name.is_empty() {
                    self.error_at("E004", "expected name after `#define`", define_line);
                    // Skip the '#' to avoid looping.
                    i += 1;
                    continue;
                }
                // Skip spaces/tabs.
                let mut k = j;
                while k < chars.len() && (chars[k] == ' ' || chars[k] == '\t') {
                    k += 1;
                }
                if k < chars.len() && chars[k] == ':' {
                    // Constant: value is everything between ':' and the closing '#'.
                    k += 1;
                    let val_start = k;
                    while k < chars.len() && chars[k] != '#' {
                        if chars[k] == '\n' {
                            line += 1;
                        }
                        k += 1;
                    }
                    if k >= chars.len() {
                        self.error_at(
                            "E005",
                            "unterminated `#define` (missing closing `#`)",
                            define_line,
                        );
                        break;
                    }
                    let value: String = chars[val_start..k].iter().collect();
                    subs.push((name, value.trim().to_string()));
                    k += 1; // consume closing '#'
                    i = k;
                    // count newline already handled above
                    continue;
                } else {
                    // Macro: body is everything from here up to a line that is just '#'.
                    // Skip the rest of the NAME line first.
                    while k < chars.len() && chars[k] != '\n' {
                        k += 1;
                    }
                    if k < chars.len() {
                        k += 1; // consume newline
                        line += 1;
                    }
                    let body_start = k;
                    let mut body_end = None;
                    while k < chars.len() {
                        // Is the rest of this line just '#'?
                        let line_start = k;
                        let mut m = k;
                        while m < chars.len() && (chars[m] == ' ' || chars[m] == '\t') {
                            m += 1;
                        }
                        if m < chars.len() && chars[m] == '#' {
                            // Check nothing but whitespace until newline/eof.
                            let mut n = m + 1;
                            while n < chars.len()
                                && chars[n] != '\n'
                                && (chars[n] == ' ' || chars[n] == '\t')
                            {
                                n += 1;
                            }
                            if n >= chars.len() || chars[n] == '\n' {
                                body_end = Some(line_start);
                                // advance k past the closing '#' line
                                k = if n < chars.len() { n + 1 } else { n };
                                line += 1;
                                break;
                            }
                        }
                        // consume the line
                        while k < chars.len() && chars[k] != '\n' {
                            k += 1;
                        }
                        if k < chars.len() {
                            k += 1;
                            line += 1;
                        }
                    }
                    match body_end {
                        Some(end) => {
                            let body: String = chars[body_start..end].iter().collect();
                            subs.push((name, body.trim_end().to_string()));
                            i = k;
                            continue;
                        }
                        None => {
                            self.error_at(
                                "E005",
                                "unterminated `#define` macro (missing closing `#`)",
                                define_line,
                            );
                            break;
                        }
                    }
                }
            }

            if chars[i] == '\n' {
                line += 1;
            }
            out.push(chars[i]);
            i += 1;
        }

        // Apply substitutions as whole-word replacements, in order.
        let mut result = out;
        for (name, value) in &subs {
            result = replace_whole_word(&result, name, value);
        }
        result
    }

    // -----------------------------------------------------------------------
    // Diagnostics
    // -----------------------------------------------------------------------

    fn error(&mut self, code: &str, msg: impl Into<String>, line: usize, src_line: &str) {
        self.diags.push(Diagnostic::new(
            code,
            msg,
            self.file.clone(),
            line,
            1,
            src_line.to_string(),
        ));
    }

    fn error_at(&mut self, code: &str, msg: impl Into<String>, line: usize) {
        self.diags.push(Diagnostic::new(
            code,
            msg,
            self.file.clone(),
            line,
            1,
            String::new(),
        ));
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Index every top-level `struct NAME { … }` and `fn NAME(...) ... { … }` in a
/// module file, returning (name, full-source-text) pairs in file order.
fn index_top_level_defs(src: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut defs = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        // Skip whitespace and line comments at top level.
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // Try to read a keyword `struct` or `fn`.
        if matches_keyword(&chars, i, "struct") || matches_keyword(&chars, i, "fn") {
            let def_start = i;
            let is_struct = matches_keyword(&chars, i, "struct");
            i += if is_struct {
                "struct".len()
            } else {
                "fn".len()
            };
            // skip spaces
            while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                i += 1;
            }
            // read name
            let name_start = i;
            while i < chars.len() && is_ident_char_at(&chars, i) {
                i += 1;
            }
            let name: String = chars[name_start..i].iter().collect();
            // Advance to the first '{' (skipping the param list / return type).
            while i < chars.len() && chars[i] != '{' {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }
            // Consume the balanced brace block.
            let mut depth = 0i32;
            while i < chars.len() {
                match chars[i] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let text: String = chars[def_start..i].iter().collect();
            if !name.is_empty() {
                defs.push((name, text));
            }
        } else {
            // Unknown top-level token: skip to next whitespace.
            i += 1;
        }
    }
    defs
}

/// Rename the *declared* identifier of a definition (the name right after
/// `struct`/`fn`) from `old` to `new`. Used for `import … as alias;`.
///
/// For a struct, this also leaves method names untouched (only the type name
/// changes); for a free function only the function name changes.
fn rename_definition(text: &str, old: &str, new: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    // find `struct`/`fn`
    while i < chars.len() {
        if matches_keyword(&chars, i, "struct") {
            i += "struct".len();
        } else if matches_keyword(&chars, i, "fn") {
            i += "fn".len();
        } else if chars[i].is_whitespace() {
            i += 1;
            continue;
        } else {
            i += 1;
            continue;
        }
        // skip spaces
        while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
            i += 1;
        }
        let name_start = i;
        while i < chars.len() && is_ident_char_at(&chars, i) {
            i += 1;
        }
        let name: String = chars[name_start..i].iter().collect();
        if name == old {
            let mut result = String::new();
            result.extend(&chars[..name_start]);
            result.push_str(new);
            result.extend(&chars[i..]);
            // For a struct, also rename internal `return OldName {` and the
            // method-call/label uses are by-name; we conservatively replace
            // whole-word occurrences of the old name in the remainder too,
            // since the type name appears in `return Old { ... }` etc.
            return replace_whole_word(&result, old, new);
        }
        break;
    }
    text.to_string()
}

/// Replace every whole-word (identifier-boundary) occurrence of `name` with
/// `value`. Does not replace inside string literals... well, in v1 it does the
/// simple thing and replaces everywhere on identifier boundaries.
fn replace_whole_word(src: &str, name: &str, value: &str) -> String {
    if name.is_empty() {
        return src.to_string();
    }
    let chars: Vec<char> = src.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == name_chars[0]
            && i + name_chars.len() <= chars.len()
            && chars[i..i + name_chars.len()] == name_chars[..]
        {
            let before_ok = i == 0 || !is_ident_boundary(chars[i - 1]);
            let after_idx = i + name_chars.len();
            let after_ok = after_idx >= chars.len() || !is_ident_boundary(chars[after_idx]);
            if before_ok && after_ok {
                out.push_str(value);
                i = after_idx;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_ident_boundary(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

fn is_ident_char_at(chars: &[char], i: usize) -> bool {
    chars
        .get(i)
        .map(|&c| c == '_' || c.is_ascii_alphanumeric())
        .unwrap_or(false)
}

/// True if `kw` appears at `chars[at..]` and is followed by a non-identifier
/// char (so `fnx` does not match `fn`).
fn matches_keyword(chars: &[char], at: usize, kw: &str) -> bool {
    let kw_chars: Vec<char> = kw.chars().collect();
    if at + kw_chars.len() > chars.len() {
        return false;
    }
    if chars[at..at + kw_chars.len()] != kw_chars[..] {
        return false;
    }
    // boundary before
    if at > 0 && is_ident_boundary(chars[at - 1]) {
        return false;
    }
    // boundary after
    let after = at + kw_chars.len();
    !(after < chars.len() && is_ident_boundary(chars[after]))
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}
