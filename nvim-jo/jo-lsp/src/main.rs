//! Jo language server.
//!
//! Diagnostics: invokes jo_compiler on the *actual* saved file.
//!              On unsaved changes we skip diagnostics (avoids stdlib path issues).
//!
//! Completions: parses the document text with a lightweight regex-based
//!              extractor to build a per-file symbol table, then answers
//!              completion requests with context-aware results:
//!              - After `x.`        → fields + methods of x's type
//!              - After `Type::`    → static methods of that type
//!              - After `let <ident> =` / top-level → type names
//!              - Everywhere else   → variables in scope + keywords

use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::process::Command;

use serde_json::{json, Value};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // uri → DocumentState
    let mut docs: HashMap<String, DocState> = HashMap::new();

    loop {
        // ---- read one LSP message ----------------------------------------
        let mut content_length = 0usize;
        let mut line = String::new();
        loop {
            line.clear();
            if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                return; // stdin closed
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(v) = trimmed.strip_prefix("Content-Length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        if content_length == 0 {
            continue;
        }
        let mut body = vec![0u8; content_length];
        if stdin.lock().read_exact(&mut body).is_err() {
            return;
        }
        let msg: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = msg["method"].as_str().unwrap_or("").to_string();
        let id = msg.get("id").cloned();

        match method.as_str() {
            // ---- lifecycle --------------------------------------------------
            "initialize" => {
                send_response(
                    &mut stdout,
                    id,
                    json!({
                        "capabilities": {
                            "textDocumentSync": {
                                "openClose": true,
                                "change": 1,   // full sync
                                "save": { "includeText": false }
                            },
                            "completionProvider": {
                                "triggerCharacters": [".", ":", " "],
                                "resolveProvider": false
                            }
                        },
                        "serverInfo": { "name": "jo-lsp", "version": "0.2.0" }
                    }),
                );
            }
            "initialized" => {}
            "shutdown" => send_response(&mut stdout, id, Value::Null),
            "exit" => break,

            // ---- document sync ----------------------------------------------
            "textDocument/didOpen" => {
                let uri = str_field(&msg, &["params", "textDocument", "uri"]);
                let text = str_field(&msg, &["params", "textDocument", "text"]);
                let path = uri_to_path(&uri);
                let ds = DocState::new(text, path.clone());
                // Diagnostics only on the real saved file
                let diags = lint_saved(&path);
                publish_diagnostics(&mut stdout, &uri, diags);
                docs.insert(uri, ds);
            }
            "textDocument/didChange" => {
                let uri = str_field(&msg, &["params", "textDocument", "uri"]);
                if let Some(arr) = msg["params"]["contentChanges"].as_array() {
                    if let Some(last) = arr.last() {
                        let text = last["text"].as_str().unwrap_or("").to_string();
                        let path = uri_to_path(&uri);
                        if let Some(ds) = docs.get_mut(&uri) {
                            ds.update(text);
                        } else {
                            docs.insert(uri.clone(), DocState::new(text, path));
                        }
                    }
                }
                // Don't re-lint on every keystroke — diagnostics update on save
            }
            "textDocument/didSave" => {
                let uri = str_field(&msg, &["params", "textDocument", "uri"]);
                let path = uri_to_path(&uri);
                let diags = lint_saved(&path);
                publish_diagnostics(&mut stdout, &uri, diags);
            }
            "textDocument/didClose" => {
                let uri = str_field(&msg, &["params", "textDocument", "uri"]);
                docs.remove(&uri);
                publish_diagnostics(&mut stdout, &uri, vec![]);
            }

            // ---- completions ------------------------------------------------
            "textDocument/completion" => {
                let uri = str_field(&msg, &["params", "textDocument", "uri"]);
                let line_num = msg["params"]["position"]["line"]
                    .as_u64()
                    .unwrap_or(0) as usize;
                let col = msg["params"]["position"]["character"]
                    .as_u64()
                    .unwrap_or(0) as usize;

                let items = if let Some(ds) = docs.get(&uri) {
                    ds.completions(line_num, col)
                } else {
                    vec![]
                };

                send_response(
                    &mut stdout,
                    id,
                    json!({ "isIncomplete": false, "items": items }),
                );
            }

            _ => {
                if id.is_some() {
                    send_response(&mut stdout, id, Value::Null);
                }
            }
        }
    }
}

// ============================================================================
// Document state + symbol extraction
// ============================================================================

struct DocState {
    text: String,
    #[allow(dead_code)]
    path: String,
    symbols: Symbols,
}

impl DocState {
    fn new(text: String, path: String) -> Self {
        let symbols = Symbols::extract(&text);
        DocState { text, path, symbols }
    }

    fn update(&mut self, text: String) {
        self.symbols = Symbols::extract(&text);
        self.text = text;
    }

    fn completions(&self, line: usize, col: usize) -> Vec<Value> {
        // Get the text of the current line up to the cursor
        let line_text = self.text.lines().nth(line).unwrap_or("");
        let prefix = &line_text[..col.min(line_text.len())];

        // Collect variables visible at this point (all let bindings in file — good enough
        // for single-file source; a real scope analysis would be per-function)
        let vars = self.symbols.vars_at(line);

        // Case 1: `something.` — field/method completion
        if let Some(recv) = dot_receiver(prefix) {
            // Find type of recv
            let enclosing = self.symbols.enclosing_struct(line);
            let ty_opt = vars.get(recv).copied().or_else(|| {
                if recv == "self" { enclosing } else { None }
            });
            if let Some(ty) = ty_opt {
                return self.symbols.members_of(ty);
            }
            // Unknown receiver — return empty so we don't spam wrong completions
            return vec![];
        }

        // Case 2: `Something::` — static method completion
        if let Some(ty) = colon_colon_receiver(prefix) {
            return self.symbols.statics_of(ty);
        }

        // Case 3: context before `=` in let binding, or explicit type annotation
        // e.g. `let x: ` or `let x: Fo` → suggest type names
        if is_type_position(prefix) {
            return self.symbols.type_completions();
        }

        // Case 4: general — keywords + types + visible variables
        let mut items = keyword_completions();
        items.extend(self.symbols.type_completions());
        items.extend(var_completions(&vars));
        items
    }
}

// ============================================================================
// Symbol table
// ============================================================================

#[derive(Debug, Default)]
struct Symbols {
    // struct name → (fields, instance_methods, static_methods)
    structs: HashMap<String, StructInfo>,
    // variable name → type name, with the line it was declared on
    vars: Vec<(usize, String, String)>, // (line, name, type)
}

#[derive(Debug, Default, Clone)]
struct StructInfo {
    fields: Vec<(String, String)>,          // (name, type)
    methods: Vec<(String, MethodKind, String)>, // (name, kind, return_type)
}

#[derive(Debug, Clone, PartialEq)]
enum MethodKind {
    Instance, // has &self / &mut self / move self
    Static,
}

impl Symbols {
    fn extract(src: &str) -> Self {
        let mut syms = Symbols::default();
        syms.parse_structs(src);
        syms.parse_vars(src);
        syms
    }

    // ------------------------------------------------------------------
    // Struct parsing
    // ------------------------------------------------------------------

    fn parse_structs(&mut self, src: &str) {
        // Match: struct Name { ... } or extend Name { ... }
        // We do a brace-counting scan rather than a regex so we handle nested braces.
        let bytes = src.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            // Skip comments
            if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
                while i < len && bytes[i] != b'\n' { i += 1; }
                continue;
            }
            // Skip strings
            if bytes[i] == b'"' {
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' { i += 1; }
                    i += 1;
                }
                i += 1;
                continue;
            }

            // Look for keyword `struct` or `extend`
            let kw = if src[i..].starts_with("struct ") || src[i..].starts_with("struct\t") {
                Some(("struct", 6usize))
            } else if src[i..].starts_with("extend ") || src[i..].starts_with("extend\t") {
                Some(("extend", 6usize))
            } else {
                None
            };

            if let Some((kw_kind, kw_len)) = kw {
                i += kw_len + 1;
                // Skip whitespace
                while i < len && bytes[i].is_ascii_whitespace() { i += 1; }
                // Skip optional module prefix `mod::` for extend
                let name_start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
                let mut name: String = src[name_start..i].to_string();
                // Handle `extend mod::Name`
                if i + 2 < len && bytes[i] == b':' && bytes[i+1] == b':' {
                    i += 2;
                    let ns = i;
                    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
                    name = src[ns..i].to_string();
                }
                if name.is_empty() { continue; }
                // Find opening brace
                while i < len && bytes[i] != b'{' { i += 1; }
                if i >= len { break; }
                i += 1; // consume '{'

                let entry = self.structs.entry(name.clone()).or_default();
                parse_struct_body(src, &mut i, entry, kw_kind == "struct");
                continue;
            }

            i += 1;
        }
    }

    fn parse_vars(&mut self, src: &str) {
        // Collect `let name: Type` and `let name = TypeName { ... }` and
        // `let name = expr` where we can infer the type from a struct init.
        for (lineno, line) in src.lines().enumerate() {
            let trimmed = line.trim();
            if !trimmed.starts_with("let ") { continue; }
            let rest = trimmed[4..].trim();

            // let name: Type = ...
            if let Some((name, ty)) = parse_let_typed(rest) {
                self.vars.push((lineno, name, ty));
                continue;
            }
            // let name = StructName { ... }  or  let name = StructName::new(...)
            if let Some((name, ty)) = parse_let_inferred(rest) {
                self.vars.push((lineno, name, ty));
            }
        }
    }

    // ------------------------------------------------------------------
    // Helpers for completions
    // ------------------------------------------------------------------

    fn vars_at(&self, line: usize) -> HashMap<&str, &str> {
        // Return all variables declared before this line
        let mut map: HashMap<&str, &str> = HashMap::new();
        for (decl_line, name, ty) in &self.vars {
            if *decl_line <= line {
                map.insert(name.as_str(), ty.as_str());
            }
        }
        map
    }

    fn enclosing_struct(&self, _line: usize) -> Option<&str> {
        // Walk backwards and find the innermost `struct Name` or `extend Name`
        // that encloses this line (very approximate — good enough for self.)
        // We'd need brace depth tracking for perfection; skip for now.
        None // placeholder — full impl below
    }

    fn members_of(&self, ty: &str) -> Vec<Value> {
        let info = match self.structs.get(ty) {
            Some(i) => i,
            None => return vec![],
        };
        let mut items = vec![];
        for (fname, ftype) in &info.fields {
            items.push(json!({
                "label": fname,
                "kind": 5,  // field
                "detail": ftype,
                "insertText": fname,
            }));
        }
        for (mname, mkind, ret) in &info.methods {
            if *mkind == MethodKind::Instance {
                items.push(json!({
                    "label": mname,
                    "kind": 2,  // method
                    "detail": format!("fn {}() {}", mname, ret),
                    "insertText": format!("{}(", mname),
                }));
            }
        }
        items
    }

    fn statics_of(&self, ty: &str) -> Vec<Value> {
        let info = match self.structs.get(ty) {
            Some(i) => i,
            None => return vec![],
        };
        info.methods.iter()
            .filter(|(_, k, _)| *k == MethodKind::Static)
            .map(|(mname, _, ret)| json!({
                "label": mname,
                "kind": 2,
                "detail": format!("fn {}() {}", mname, ret),
                "insertText": format!("{}(", mname),
            }))
            .collect()
    }

    fn type_completions(&self) -> Vec<Value> {
        let builtin_types = ["int", "float", "char", "bool", "string", "i64", "f64", "void"];
        let mut items: Vec<Value> = builtin_types.iter().map(|t| json!({
            "label": t,
            "kind": 22, // TypeParameter
            "detail": "type",
        })).collect();
        for name in self.structs.keys() {
            // skip stdlib machine types already listed
            if !["int","float","char","bool","string"].contains(&name.as_str()) {
                items.push(json!({
                    "label": name,
                    "kind": 7,  // class
                    "detail": "struct",
                }));
            }
        }
        items
    }
}

// ============================================================================
// Parsing helpers
// ============================================================================

fn parse_struct_body(src: &str, i: &mut usize, info: &mut StructInfo, is_struct: bool) {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut depth = 1usize;

    while *i < len && depth > 0 {
        // Skip comments
        if *i + 1 < len && bytes[*i] == b'/' && bytes[*i + 1] == b'/' {
            while *i < len && bytes[*i] != b'\n' { *i += 1; }
            continue;
        }
        // Skip strings
        if bytes[*i] == b'"' {
            *i += 1;
            while *i < len && bytes[*i] != b'"' {
                if bytes[*i] == b'\\' { *i += 1; }
                *i += 1;
            }
            *i += 1;
            continue;
        }
        match bytes[*i] {
            b'{' => { depth += 1; *i += 1; }
            b'}' => {
                depth -= 1;
                if depth == 0 { *i += 1; break; }
                *i += 1;
            }
            _ => {
                // Try to match a field: `name: Type,`  (only at depth 1)
                if depth == 1 && is_struct {
                    if let Some((fname, ftype, adv)) = try_parse_field(&src[*i..]) {
                        info.fields.push((fname, ftype));
                        *i += adv;
                        continue;
                    }
                }
                // Try to match `fn name(... self ...) RetType`
                if depth == 1 && src[*i..].starts_with("fn ") {
                    if let Some((mname, has_self, ret, adv)) = try_parse_fn_sig(&src[*i..]) {
                        let kind = if has_self { MethodKind::Instance } else { MethodKind::Static };
                        info.methods.push((mname, kind, ret));
                        *i += adv;
                        continue;
                    }
                }
                *i += 1;
            }
        }
    }
}

/// Parse `name: Type,` at start of slice. Returns (name, type, bytes_consumed).
fn try_parse_field(s: &str) -> Option<(String, String, usize)> {
    // Must start with ident
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() && bytes[0] != b'_' {
        return None;
    }
    let mut i = 0;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
    let name = s[..i].to_string();
    // skip whitespace
    while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
    if i >= bytes.len() || bytes[i] != b':' { return None; }
    i += 1;
    // make sure it's not `::` (would be a qualified type)
    if i < bytes.len() && bytes[i] == b':' { return None; }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
    // read type until `,` or newline
    let type_start = i;
    while i < bytes.len() && bytes[i] != b',' && bytes[i] != b'\n' && bytes[i] != b'}' { i += 1; }
    let ty = s[type_start..i].trim().to_string();
    if ty.is_empty() { return None; }
    // consume the comma if present
    if i < bytes.len() && bytes[i] == b',' { i += 1; }
    Some((name, ty, i))
}

/// Parse `fn name(params) RetType` — return (name, has_self, return_type, bytes_to_skip_past_sig).
/// We only need the signature, not the body.
fn try_parse_fn_sig(s: &str) -> Option<(String, bool, String, usize)> {
    if !s.starts_with("fn ") { return None; }
    let bytes = s.as_bytes();
    let mut i = 3;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
    let ns = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
    let name = s[ns..i].to_string();
    if name.is_empty() { return None; }
    // find opening paren
    while i < bytes.len() && bytes[i] != b'(' { i += 1; }
    if i >= bytes.len() { return None; }
    i += 1; // consume '('
    // scan params for self
    let params_start = i;
    let mut depth = 1;
    while i < bytes.len() && depth > 0 {
        if bytes[i] == b'(' { depth += 1; }
        if bytes[i] == b')' { depth -= 1; }
        i += 1;
    }
    let params = &s[params_start..i - 1];
    let has_self = params.contains("self");
    // skip whitespace
    while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
    // read return type until `{`
    let ret_start = i;
    while i < bytes.len() && bytes[i] != b'{' && bytes[i] != b'\n' { i += 1; }
    let ret = s[ret_start..i].trim().to_string();
    Some((name, has_self, ret, i))
}

fn parse_let_typed(rest: &str) -> Option<(String, String)> {
    // `name: Type = ...`
    let colon = rest.find(':')?;
    let name = rest[..colon].trim().to_string();
    if name.contains(' ') || name.is_empty() { return None; }
    let after_colon = rest[colon + 1..].trim();
    // don't consume `::` as a type annotation
    if after_colon.starts_with(':') { return None; }
    let eq = after_colon.find('=')?;
    let ty = after_colon[..eq].trim().to_string();
    if ty.is_empty() || ty.contains(' ') { return None; }
    Some((name, ty))
}

fn parse_let_inferred(rest: &str) -> Option<(String, String)> {
    // `name = StructName { ...` or `name = StructName::new(...`
    let eq = rest.find('=')?;
    let name = rest[..eq].trim().to_string();
    if name.is_empty() || name.contains(' ') || name.contains(':') { return None; }
    let rhs = rest[eq + 1..].trim();
    // qualified: `mod::Type { ` or `mod::Type::`
    // unqualified: `Type { ` or `Type::` or `Type::new(`
    let type_name = if let Some(pos) = rhs.find("::") {
        // `Mod::Type` or `Type::method` — take the part before `::`
        let first = rhs[..pos].trim();
        // If first looks like a type name (starts upper or lower), use it
        // but check if second part is a constructor pattern
        let second_start = pos + 2;
        let second = rhs[second_start..].trim();
        let first_word: String = second.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        // `Mod::Type { ` — the type is the second part
        if second.starts_with(|c: char| c.is_ascii_alphabetic()) && second.contains('{') {
            first_word
        } else {
            // `Type::new(` — type is first
            first.to_string()
        }
    } else {
        // `TypeName {` or just a function call
        let word: String = rhs.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if rhs[word.len()..].trim_start().starts_with('{') {
            word
        } else {
            return None;
        }
    };
    if type_name.is_empty() { return None; }
    Some((name, type_name))
}

// ============================================================================
// Completion context helpers
// ============================================================================

/// If the prefix ends with `word.` return `word`.
fn dot_receiver(prefix: &str) -> Option<&str> {
    let prefix = prefix.trim_end();
    let prefix = prefix.strip_suffix('.')?;
    // walk back over the identifier
    let end = prefix.len();
    let start = prefix
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|p| p + 1)
        .unwrap_or(0);
    let word = &prefix[start..end];
    if word.is_empty() { None } else { Some(word) }
}

/// If prefix ends with `Word::` return `Word`.
fn colon_colon_receiver(prefix: &str) -> Option<&str> {
    let prefix = prefix.trim_end();
    let prefix = prefix.strip_suffix("::")?;
    let end = prefix.len();
    let start = prefix
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|p| p + 1)
        .unwrap_or(0);
    let word = &prefix[start..end];
    if word.is_empty() { None } else { Some(word) }
}

/// True if cursor is at a position where a type name is expected:
/// `let name: `, `fn foo(x: `, `-> ` in param list, field decl `name: `.
fn is_type_position(prefix: &str) -> bool {
    let p = prefix.trim_end();
    // After `: ` but not `::`
    if p.ends_with(':') && !p.ends_with("::") {
        return true;
    }
    // After `->` (return type position)
    if p.ends_with("->") {
        return true;
    }
    false
}

fn keyword_completions() -> Vec<Value> {
    const KEYWORDS: &[(&str, &str)] = &[
        ("fn", "keyword"),
        ("struct", "keyword"),
        ("let", "keyword"),
        ("return", "keyword"),
        ("if", "keyword"),
        ("else", "keyword"),
        ("while", "keyword"),
        ("break", "keyword"),
        ("continue", "keyword"),
        ("import", "keyword"),
        ("extend", "keyword"),
        ("true", "bool literal"),
        ("false", "bool literal"),
        ("self", "builtin"),
        ("null", "builtin"),
    ];
    KEYWORDS
        .iter()
        .map(|(kw, detail)| {
            json!({
                "label": kw,
                "kind": 14,
                "detail": detail,
            })
        })
        .collect()
}

fn var_completions(vars: &HashMap<&str, &str>) -> Vec<Value> {
    vars.iter()
        .map(|(name, ty)| {
            json!({
                "label": *name,
                "kind": 6,   // variable
                "detail": *ty,
            })
        })
        .collect()
}

// ============================================================================
// Diagnostics
// ============================================================================

fn lint_saved(path: &str) -> Vec<Value> {
    // Only lint actual .jo files that exist on disk
    if !std::path::Path::new(path).exists() {
        return vec![];
    }
    let compiler = find_compiler();
    let output = match Command::new(&compiler).arg(path).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    // jo_compiler writes errors to stderr; success writes "compiled: ..." to stderr too
    // so we detect errors by exit code OR presence of "error[" in stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_diagnostics(&stderr)
}

fn parse_diagnostics(stderr: &str) -> Vec<Value> {
    let mut diags = Vec::new();
    let mut lines = stderr.lines().peekable();
    while let Some(line) = lines.next() {
        let (severity, msg_rest) = if let Some(r) = line.strip_prefix("error[") {
            (1u8, r.splitn(2, "]: ").nth(1).unwrap_or("").to_string())
        } else if let Some(r) = line.strip_prefix("warning[") {
            (2u8, r.splitn(2, "]: ").nth(1).unwrap_or("").to_string())
        } else {
            continue;
        };
        // Next non-empty line: `  --> file:line:col`
        if let Some(loc_line) = lines.next() {
            let loc = loc_line.trim().strip_prefix("--> ").unwrap_or("");
            // loc = "path:line:col" — split from the right to handle paths with colons
            let parts: Vec<&str> = loc.rsplitn(3, ':').collect();
            if parts.len() >= 2 {
                let col: u32 = parts[0].trim().parse().unwrap_or(1);
                let lnum: u32 = parts[1].trim().parse().unwrap_or(1);
                diags.push(json!({
                    "range": {
                        "start": { "line": lnum.saturating_sub(1), "character": col.saturating_sub(1) },
                        "end":   { "line": lnum.saturating_sub(1), "character": col.saturating_sub(1) + 80 }
                    },
                    "severity": severity,
                    "source": "jo",
                    "message": msg_rest
                }));
            }
        }
    }
    diags
}

fn find_compiler() -> String {
    if let Ok(exe) = std::env::current_exe() {
        let s = exe.parent().unwrap_or(std::path::Path::new("")).join("jo_compiler");
        if s.exists() { return s.to_string_lossy().to_string(); }
    }
    "jo_compiler".to_string()
}

// ============================================================================
// LSP transport
// ============================================================================

fn send_response(stdout: &mut impl Write, id: Option<Value>, result: Value) {
    let Some(id) = id else { return };
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0", "id": id, "result": result
    })).unwrap();
    write!(stdout, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    stdout.flush().unwrap();
}

fn send_notification(stdout: &mut impl Write, method: &str, params: Value) {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0", "method": method, "params": params
    })).unwrap();
    write!(stdout, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    stdout.flush().unwrap();
}

fn publish_diagnostics(stdout: &mut impl Write, uri: &str, diags: Vec<Value>) {
    send_notification(stdout, "textDocument/publishDiagnostics",
        json!({ "uri": uri, "diagnostics": diags }));
}

fn uri_to_path(uri: &str) -> String {
    // file:///home/... → /home/...
    // percent-decode the most common case (%20 for space)
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

fn str_field(msg: &Value, keys: &[&str]) -> String {
    let mut v = msg;
    for k in keys { v = &v[k]; }
    v.as_str().unwrap_or("").to_string()
}
