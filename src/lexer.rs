//! Hand-written lexer for Jo.
//!
//! Turns preprocessed source text into a `Vec<Token>`. Note that `#define`
//! directives are removed by the preprocessor before lexing, but the `#`,
//! `%name`, and `&name` forms used *inside* `!asm` blocks survive — the parser
//! collects asm bodies from the raw token stream, so we lex `#` as `Pound` and
//! let `%` become a `Percent` operator token; asm operand parsing happens in
//! the parser using token lexemes.

use crate::error::Diagnostic;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    IntLit(i64),
    FloatLit(f64),
    CharLit(u32),
    StringLit(String),
    BoolLit(bool),

    // Identifiers & keywords
    Ident,
    Fn,
    Struct,
    Let,
    Return,
    If,
    Else,
    While,
    Break,
    Continue,
    Import,
    As,
    Move,
    Self_,
    I64,
    F64,
    Void,
    Null,
    Mut,
    Asm,
    Extend,

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    ColonColon,
    Dot,
    Arrow,
    Bang,
    Pound,

    // Operators
    Eq,
    EqEq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Ampersand,
    AmpAmp,
    PipePipe,

    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub line: usize,
    pub col: usize,
}

pub struct Lexer<'a> {
    src: &'a str,
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    file: String,
    tokens: Vec<Token>,
    diags: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, file: impl Into<String>) -> Lexer<'a> {
        Lexer {
            src,
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            file: file.into(),
            tokens: Vec::new(),
            diags: Vec::new(),
        }
    }

    /// Tokenize the whole input. On any error, returns the collected diagnostics.
    pub fn tokenize(mut self) -> Result<Vec<Token>, Vec<Diagnostic>> {
        while !self.at_end() {
            self.scan_token();
        }
        self.push(TokenKind::Eof, String::new(), self.line, self.col);
        if self.diags.is_empty() {
            Ok(self.tokens)
        } else {
            Err(self.diags)
        }
    }

    // --- character helpers ---------------------------------------------------

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> char {
        self.chars.get(self.pos).copied().unwrap_or('\0')
    }

    fn peek2(&self) -> char {
        self.chars.get(self.pos + 1).copied().unwrap_or('\0')
    }

    /// Consume one char, advancing line/col tracking.
    fn advance(&mut self) -> char {
        let c = self.chars[self.pos];
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        c
    }

    fn push(&mut self, kind: TokenKind, lexeme: String, line: usize, col: usize) {
        self.tokens.push(Token {
            kind,
            lexeme,
            line,
            col,
        });
    }

    fn error(&mut self, code: &str, msg: impl Into<String>, line: usize, col: usize) {
        let src_line = crate::error::source_line_of(self.src, line);
        self.diags.push(Diagnostic::new(
            code,
            msg,
            self.file.clone(),
            line,
            col,
            src_line,
        ));
    }

    // --- main scan -----------------------------------------------------------

    fn scan_token(&mut self) {
        let c = self.peek();
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                self.advance();
            }
            '/' if self.peek2() == '/' => {
                // line comment
                while !self.at_end() && self.peek() != '\n' {
                    self.advance();
                }
            }
            '0'..='9' => self.scan_number(),
            '.' if self.peek2().is_ascii_digit() => self.scan_number(),
            c if is_ident_start(c) => self.scan_ident(),
            '"' => self.scan_string(),
            '\'' => self.scan_char(),
            _ => self.scan_symbol(),
        }
    }

    fn scan_number(&mut self) {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        let mut is_float = false;

        while self.peek().is_ascii_digit() {
            self.advance();
        }
        // A single '.' followed by digits (or the leading '.' case) makes a float.
        if self.peek() == '.' {
            is_float = true;
            self.advance();
            while self.peek().is_ascii_digit() {
                self.advance();
            }
        }

        let lexeme: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            // Forms like "12." or ".3" must be padded for Rust's parser.
            let parse_str = normalize_float(&lexeme);
            match parse_str.parse::<f64>() {
                Ok(v) => self.push(TokenKind::FloatLit(v), lexeme, line, col),
                Err(_) => self.error(
                    "E101",
                    format!("invalid float literal `{}`", lexeme),
                    line,
                    col,
                ),
            }
        } else {
            match lexeme.parse::<i64>() {
                Ok(v) => self.push(TokenKind::IntLit(v), lexeme, line, col),
                Err(_) => self.error(
                    "E102",
                    format!("integer literal `{}` out of range", lexeme),
                    line,
                    col,
                ),
            }
        }
    }

    fn scan_ident(&mut self) {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        while is_ident_continue(self.peek()) {
            self.advance();
        }
        let lexeme: String = self.chars[start..self.pos].iter().collect();
        let kind = keyword_kind(&lexeme).unwrap_or(TokenKind::Ident);
        self.push(kind, lexeme, line, col);
    }

    fn scan_string(&mut self) {
        let (line, col) = (self.line, self.col);
        self.advance(); // opening "
        let mut value = String::new();
        loop {
            if self.at_end() {
                self.error("E103", "unterminated string literal", line, col);
                return;
            }
            let c = self.advance();
            if c == '"' {
                break;
            }
            if c == '\n' {
                // A raw newline inside a "..." most likely means a missing
                // closing quote; report it rather than silently spanning lines.
                self.error("E103", "unterminated string literal", line, col);
                return;
            }
            if c == '\\' {
                match self.scan_escape(line, col) {
                    Some(ch) => value.push(ch),
                    None => return, // error already reported
                }
            } else {
                value.push(c);
            }
        }
        let lexeme = format!("\"{}\"", value);
        self.push(TokenKind::StringLit(value), lexeme, line, col);
    }

    fn scan_char(&mut self) {
        let (line, col) = (self.line, self.col);
        self.advance(); // opening '
        if self.at_end() {
            self.error("E104", "unterminated char literal", line, col);
            return;
        }
        let first = self.advance();
        if first == '\'' {
            self.error("E104", "empty char literal", line, col);
            return;
        }
        // Decode a single (possibly escaped) character.
        let ch = if first == '\\' {
            match self.scan_escape(line, col) {
                Some(ch) => ch,
                None => return, // error already reported
            }
        } else {
            first
        };
        if self.peek() != '\'' {
            self.error(
                "E104",
                "unterminated char literal (expected a single character)",
                line,
                col,
            );
            // best-effort recovery: skip to the next quote or newline
            while !self.at_end() && self.peek() != '\'' && self.peek() != '\n' {
                self.advance();
            }
            if self.peek() == '\'' {
                self.advance();
            }
            return;
        }
        self.advance(); // closing '
        let lexeme = format!("'{}'", ch);
        self.push(TokenKind::CharLit(ch as u32), lexeme, line, col);
    }

    /// Decode the character following a backslash in a string/char literal.
    /// The backslash has already been consumed. Returns `None` (after reporting
    /// an error) on an invalid or truncated escape.
    fn scan_escape(&mut self, line: usize, col: usize) -> Option<char> {
        if self.at_end() {
            self.error("E106", "unterminated escape sequence", line, col);
            return None;
        }
        let e = self.advance();
        let ch = match e {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            '`' => '`',
            // \xHH — a raw byte given as two hex digits (0x00..=0xFF).
            'x' => {
                let hi = self.advance_hex_digit(line, col)?;
                let lo = self.advance_hex_digit(line, col)?;
                let byte = (hi * 16 + lo) as u8;
                byte as char
            }
            other => {
                self.error(
                    "E107",
                    format!("unknown escape sequence `\\{}`", other),
                    line,
                    col,
                );
                return None;
            }
        };
        Some(ch)
    }

    /// Consume one hex digit, returning its value 0..=15.
    fn advance_hex_digit(&mut self, line: usize, col: usize) -> Option<u32> {
        let c = self.peek();
        match c.to_digit(16) {
            Some(d) => {
                self.advance();
                Some(d)
            }
            None => {
                self.error("E108", "expected two hex digits after `\\x`", line, col);
                None
            }
        }
    }

    fn scan_symbol(&mut self) {
        let (line, col) = (self.line, self.col);
        let c = self.advance();
        let kind = match c {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semicolon,
            '.' => TokenKind::Dot,
            '#' => TokenKind::Pound,
            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            ':' => {
                if self.peek() == ':' {
                    self.advance();
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }
            '-' => {
                if self.peek() == '>' {
                    self.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '=' => {
                if self.peek() == '=' {
                    self.advance();
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.peek() == '=' {
                    self.advance();
                    TokenKind::Ne
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if self.peek() == '=' {
                    self.advance();
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek() == '=' {
                    self.advance();
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.peek() == '&' {
                    self.advance();
                    TokenKind::AmpAmp
                } else {
                    TokenKind::Ampersand
                }
            }
            '|' => {
                if self.peek() == '|' {
                    self.advance();
                    TokenKind::PipePipe
                } else {
                    self.error(
                        "E105",
                        "unexpected character `|` (did you mean `||`?)",
                        line,
                        col,
                    );
                    return;
                }
            }
            other => {
                self.error(
                    "E105",
                    format!("unexpected character `{}`", other),
                    line,
                    col,
                );
                return;
            }
        };
        let lex = symbol_lexeme(&kind);
        self.push(kind, lex, line, col);
    }
}

fn symbol_lexeme(kind: &TokenKind) -> String {
    match kind {
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        TokenKind::LBrace => "{",
        TokenKind::RBrace => "}",
        TokenKind::LBracket => "[",
        TokenKind::RBracket => "]",
        TokenKind::Comma => ",",
        TokenKind::Semicolon => ";",
        TokenKind::Colon => ":",
        TokenKind::ColonColon => "::",
        TokenKind::Dot => ".",
        TokenKind::Arrow => "->",
        TokenKind::Bang => "!",
        TokenKind::Pound => "#",
        TokenKind::Eq => "=",
        TokenKind::EqEq => "==",
        TokenKind::Ne => "!=",
        TokenKind::Lt => "<",
        TokenKind::Gt => ">",
        TokenKind::Le => "<=",
        TokenKind::Ge => ">=",
        TokenKind::Plus => "+",
        TokenKind::Minus => "-",
        TokenKind::Star => "*",
        TokenKind::Slash => "/",
        TokenKind::Percent => "%",
        TokenKind::Ampersand => "&",
        TokenKind::AmpAmp => "&&",
        TokenKind::PipePipe => "||",
        _ => "",
    }
    .to_string()
}

fn keyword_kind(s: &str) -> Option<TokenKind> {
    Some(match s {
        "fn" => TokenKind::Fn,
        "struct" => TokenKind::Struct,
        "let" => TokenKind::Let,
        "return" => TokenKind::Return,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "import" => TokenKind::Import,
        "as" => TokenKind::As,
        "move" => TokenKind::Move,
        "self" => TokenKind::Self_,
        "i64" => TokenKind::I64,
        "f64" => TokenKind::F64,
        "void" => TokenKind::Void,
        "null" => TokenKind::Null,
        "mut" => TokenKind::Mut,
        "asm" => TokenKind::Asm,
        "extend" => TokenKind::Extend,
        "true" => TokenKind::BoolLit(true),
        "false" => TokenKind::BoolLit(false),
        // int, float, char, bool, string are NOT keywords.
        _ => return None,
    })
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// Normalize a Jo float lexeme into something `f64::from_str` accepts.
/// `12.` → `12.0`, `.3` → `0.3`, `0.5` → `0.5`.
fn normalize_float(lexeme: &str) -> String {
    let mut s = lexeme.to_string();
    if s.starts_with('.') {
        s.insert(0, '0');
    }
    if s.ends_with('.') {
        s.push('0');
    }
    s
}
