//! Hand-written recursive-descent parser for Jo.

use crate::ast::*;
use crate::error::Diagnostic;
use crate::lexer::{Token, TokenKind};

pub struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    file: String,
    src: &'a str,
    diags: Vec<Diagnostic>,
}

/// Internal control-flow signal used for error recovery.
struct ParseAbort;
type PResult<T> = Result<T, ParseAbort>;

impl<'a> Parser<'a> {
    pub fn new(tokens: Vec<Token>, file: impl Into<String>, src: &'a str) -> Parser<'a> {
        Parser {
            tokens,
            pos: 0,
            file: file.into(),
            src,
            diags: Vec::new(),
        }
    }

    pub fn parse(mut self) -> Result<Program, Vec<Diagnostic>> {
        let mut items = Vec::new();
        while !self.check(&TokenKind::Eof) {
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(ParseAbort) => {
                    self.synchronize_top_level();
                }
            }
        }
        if self.diags.is_empty() {
            Ok(Program { items })
        } else {
            Err(self.diags)
        }
    }

    // --- token helpers -------------------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn peek_at(&self, n: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + n)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(kind)
    }

    fn at_end(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if !self.at_end() {
            self.pos += 1;
        }
        tok
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn span_here(&self) -> Span {
        let t = self.peek();
        Span::new(t.line, t.col)
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> PResult<Token> {
        if self.check(&kind) {
            Ok(self.advance())
        } else {
            self.error_here(&format!("expected {}", what), what);
            Err(ParseAbort)
        }
    }

    fn error_here(&mut self, msg: &str, label: &str) {
        let t = self.peek().clone();
        let src_line = crate::error::source_line_of(self.src, t.line);
        let got = describe_token(&t);
        self.diags.push(
            Diagnostic::new(
                "E200",
                format!("{}, found {}", msg, got),
                self.file.clone(),
                t.line,
                t.col,
                src_line,
            )
            .with_label(label.to_string()),
        );
    }

    fn error_at(
        &mut self,
        tok: &Token,
        code: &str,
        msg: impl Into<String>,
        label: impl Into<String>,
    ) {
        let src_line = crate::error::source_line_of(self.src, tok.line);
        self.diags.push(
            Diagnostic::new(code, msg, self.file.clone(), tok.line, tok.col, src_line)
                .with_label(label),
        );
    }

    // --- error recovery ------------------------------------------------------

    fn synchronize_top_level(&mut self) {
        while !self.at_end() {
            match self.peek_kind() {
                TokenKind::Fn | TokenKind::Struct | TokenKind::Import | TokenKind::Extend => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn synchronize_stmt(&mut self) {
        while !self.at_end() {
            if self.eat(&TokenKind::Semicolon) {
                return;
            }
            match self.peek_kind() {
                TokenKind::RBrace
                | TokenKind::Let
                | TokenKind::Return
                | TokenKind::If
                | TokenKind::While
                | TokenKind::Break
                | TokenKind::Continue => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // --- items ---------------------------------------------------------------

    fn parse_item(&mut self) -> PResult<Item> {
        match self.peek_kind() {
            TokenKind::Import => Ok(Item::Import(self.parse_import()?)),
            TokenKind::Struct => Ok(Item::Struct(self.parse_struct()?)),
            TokenKind::Fn => Ok(Item::Fn(self.parse_fn()?)),
            TokenKind::Extend => Ok(Item::Extend(self.parse_extend()?)),
            TokenKind::Bang => {
                // Top-level !asm is illegal.
                self.error_here(
                    "inline assembly is only valid inside a function body",
                    "asm not allowed here",
                );
                Err(ParseAbort)
            }
            _ => {
                self.error_here(
                    "expected an item (fn, struct, extend, or import)",
                    "unexpected token",
                );
                Err(ParseAbort)
            }
        }
    }

    fn parse_import(&mut self) -> PResult<ImportDecl> {
        // Imports are normally consumed by the preprocessor. If one survives
        // (module form leaves nothing pasted but the `import module;` line is
        // removed by the preprocessor too), we still parse defensively.
        self.expect(TokenKind::Import, "`import`")?;
        let module_tok = self.expect(TokenKind::Ident, "module name")?;
        let module = module_tok.lexeme.clone();
        if self.eat(&TokenKind::ColonColon) {
            if self.eat(&TokenKind::Star) {
                self.expect(TokenKind::Semicolon, "`;`")?;
                return Ok(ImportDecl {
                    kind: ImportKind::Glob { module },
                });
            }
            let name_tok = self.expect(TokenKind::Ident, "imported name")?;
            let name = name_tok.lexeme.clone();
            let alias = if self.eat(&TokenKind::As) {
                Some(self.expect(TokenKind::Ident, "alias name")?.lexeme.clone())
            } else {
                None
            };
            self.expect(TokenKind::Semicolon, "`;`")?;
            Ok(ImportDecl {
                kind: ImportKind::Named {
                    module,
                    name,
                    alias,
                },
            })
        } else {
            self.expect(TokenKind::Semicolon, "`;`")?;
            Ok(ImportDecl {
                kind: ImportKind::Module { name: module },
            })
        }
    }

    fn parse_struct(&mut self) -> PResult<StructDecl> {
        let span = self.span_here();
        self.expect(TokenKind::Struct, "`struct`")?;
        let name = self.expect(TokenKind::Ident, "struct name")?.lexeme.clone();
        self.expect(TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            if self.check(&TokenKind::Fn) {
                methods.push(self.parse_fn()?);
            } else if self.check(&TokenKind::Ident) {
                // field_decl: IDENT ":" type ","
                let fspan = self.span_here();
                let fname = self.advance().lexeme.clone();
                self.expect(TokenKind::Colon, "`:` after field name")?;
                let ty = self.parse_type()?;
                self.expect(TokenKind::Comma, "`,` after field declaration")?;
                fields.push(FieldDecl {
                    name: fname,
                    ty,
                    span: fspan,
                });
            } else {
                self.error_here(
                    "expected a field or method in struct body",
                    "unexpected token",
                );
                return Err(ParseAbort);
            }
        }
        self.expect(TokenKind::RBrace, "`}` to close struct")?;
        Ok(StructDecl {
            name,
            fields,
            methods,
            span,
        })
    }

    fn parse_extend(&mut self) -> PResult<ExtendDecl> {
        let span = self.span_here();
        self.expect(TokenKind::Extend, "`extend`")?;
        let first = self.parse_extend_target_name()?;
        let (module, name) = if self.eat(&TokenKind::ColonColon) {
            let n = self.expect(TokenKind::Ident, "struct name")?.lexeme.clone();
            (Some(first), n)
        } else {
            (None, first)
        };
        self.expect(TokenKind::LBrace, "`{`")?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            if self.check(&TokenKind::Fn) {
                methods.push(self.parse_fn()?);
            } else {
                self.error_here(
                    "expected a method in extend body",
                    "only methods allowed in extend",
                );
                return Err(ParseAbort);
            }
        }
        self.expect(TokenKind::RBrace, "`}` to close extend")?;
        Ok(ExtendDecl {
            module,
            name,
            methods,
            span,
        })
    }

    fn parse_extend_target_name(&mut self) -> PResult<String> {
        match self.peek_kind() {
            TokenKind::Ident | TokenKind::I64 | TokenKind::F64 => Ok(self.advance().lexeme),
            _ => {
                self.error_here(
                    "expected struct or primitive type name",
                    "expected type name",
                );
                Err(ParseAbort)
            }
        }
    }

    fn parse_fn(&mut self) -> PResult<FnDecl> {
        let span = self.span_here();
        self.expect(TokenKind::Fn, "`fn`")?;
        let name = self
            .expect(TokenKind::Ident, "function name")?
            .lexeme
            .clone();
        self.expect(TokenKind::LParen, "`(`")?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen, "`)`")?;
        // Return type is REQUIRED.
        if !self.is_type_start() {
            self.error_here("expected return type after ')'", "expected return type");
            return Err(ParseAbort);
        }
        let return_type = self.parse_type()?;
        let body = self.parse_block()?;
        Ok(FnDecl {
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    fn parse_params(&mut self) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        if self.check(&TokenKind::RParen) {
            return Ok(params);
        }
        loop {
            params.push(self.parse_param()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            // allow trailing comma before ')'
            if self.check(&TokenKind::RParen) {
                break;
            }
        }
        Ok(params)
    }

    fn parse_param(&mut self) -> PResult<Param> {
        let span = self.span_here();
        match self.peek_kind() {
            TokenKind::Ampersand => {
                self.advance();
                if self.eat(&TokenKind::Mut) {
                    self.expect(TokenKind::Self_, "`self` after `&mut`")?;
                    Ok(Param {
                        kind: ParamKind::SelfMutRef,
                        span,
                    })
                } else {
                    self.expect(TokenKind::Self_, "`self` after `&`")?;
                    Ok(Param {
                        kind: ParamKind::SelfRef,
                        span,
                    })
                }
            }
            TokenKind::Move => {
                self.advance();
                self.expect(TokenKind::Self_, "`self` after `move`")?;
                Ok(Param {
                    kind: ParamKind::SelfMove,
                    span,
                })
            }
            TokenKind::Self_ => {
                // bare `self` is not allowed
                self.error_here("bare `self` is not a valid parameter (use `&self`, `&mut self`, or `move self`)", "invalid self");
                Err(ParseAbort)
            }
            TokenKind::Ident => {
                let name = self.advance().lexeme.clone();
                self.expect(TokenKind::Colon, "`:` after parameter name")?;
                let ty = self.parse_type()?;
                Ok(Param {
                    kind: ParamKind::Named { name, ty },
                    span,
                })
            }
            _ => {
                self.error_here("expected a parameter", "unexpected token");
                Err(ParseAbort)
            }
        }
    }

    // --- types ---------------------------------------------------------------

    fn is_type_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::I64
                | TokenKind::F64
                | TokenKind::Void
                | TokenKind::Null
                | TokenKind::Ampersand
                | TokenKind::Ident
        )
    }

    fn parse_type(&mut self) -> PResult<Type> {
        match self.peek_kind() {
            TokenKind::I64 => {
                self.advance();
                Ok(Type::I64)
            }
            TokenKind::F64 => {
                self.advance();
                Ok(Type::F64)
            }
            TokenKind::Void => {
                self.advance();
                Ok(Type::Void)
            }
            TokenKind::Null => {
                self.advance();
                Ok(Type::Null)
            }
            TokenKind::Ampersand => {
                self.advance();
                if self.eat(&TokenKind::Mut) {
                    let inner = self.parse_type()?;
                    Ok(Type::MutRef(Box::new(inner)))
                } else {
                    let inner = self.parse_type()?;
                    Ok(Type::Ref(Box::new(inner)))
                }
            }
            TokenKind::Ident => {
                let first = self.advance().lexeme.clone();
                if self.eat(&TokenKind::ColonColon) {
                    let second = self
                        .expect(TokenKind::Ident, "type name after `::`")?
                        .lexeme
                        .clone();
                    Ok(Type::Qualified(first, second))
                } else {
                    Ok(Type::Named(first))
                }
            }
            _ => {
                self.error_here("expected a type", "expected type");
                Err(ParseAbort)
            }
        }
    }

    // --- blocks & statements -------------------------------------------------

    fn parse_block(&mut self) -> PResult<Block> {
        self.expect(TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(ParseAbort) => {
                    self.synchronize_stmt();
                }
            }
        }
        self.expect(TokenKind::RBrace, "`}`")?;
        Ok(Block { stmts })
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        match self.peek_kind() {
            TokenKind::Let => self.parse_let(),
            TokenKind::Return => self.parse_return(),
            TokenKind::If => Ok(Stmt::If(self.parse_if()?)),
            TokenKind::While => Ok(Stmt::While(self.parse_while()?)),
            TokenKind::Break => {
                let span = self.span_here();
                self.advance();
                self.expect(TokenKind::Semicolon, "`;` after break")?;
                Ok(Stmt::Break(span))
            }
            TokenKind::Continue => {
                let span = self.span_here();
                self.advance();
                self.expect(TokenKind::Semicolon, "`;` after continue")?;
                Ok(Stmt::Continue(span))
            }
            TokenKind::Bang if matches!(self.peek_at(1), TokenKind::Asm) => {
                Ok(Stmt::Asm(self.parse_asm()?))
            }
            _ => self.parse_expr_or_assign(),
        }
    }

    fn parse_let(&mut self) -> PResult<Stmt> {
        let span = self.span_here();
        self.expect(TokenKind::Let, "`let`")?;
        let name = self
            .expect(TokenKind::Ident, "variable name")?
            .lexeme
            .clone();
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq, "`=` in let binding")?;
        let init = self.parse_expr()?;
        self.expect(TokenKind::Semicolon, "`;` after let")?;
        Ok(Stmt::Let(LetStmt {
            name,
            ty,
            init,
            span,
        }))
    }

    fn parse_return(&mut self) -> PResult<Stmt> {
        let span = self.span_here();
        self.expect(TokenKind::Return, "`return`")?;
        if self.eat(&TokenKind::Semicolon) {
            return Ok(Stmt::Return(ReturnStmt { value: None, span }));
        }
        let value = self.parse_expr()?;
        self.expect(TokenKind::Semicolon, "`;` after return value")?;
        Ok(Stmt::Return(ReturnStmt {
            value: Some(value),
            span,
        }))
    }

    fn parse_if(&mut self) -> PResult<IfStmt> {
        let span = self.span_here();
        self.expect(TokenKind::If, "`if`")?;
        let mut branches = Vec::new();
        let cond = self.parse_expr_no_struct()?;
        let then = self.parse_block()?;
        branches.push((cond, then));
        let mut else_block = None;
        while self.eat(&TokenKind::Else) {
            if self.eat(&TokenKind::If) {
                let cond = self.parse_expr_no_struct()?;
                let blk = self.parse_block()?;
                branches.push((cond, blk));
            } else {
                else_block = Some(self.parse_block()?);
                break;
            }
        }
        Ok(IfStmt {
            branches,
            else_block,
            span,
        })
    }

    fn parse_while(&mut self) -> PResult<WhileStmt> {
        let span = self.span_here();
        self.expect(TokenKind::While, "`while`")?;
        let condition = self.parse_expr_no_struct()?;
        let body = self.parse_block()?;
        Ok(WhileStmt {
            condition,
            body,
            span,
        })
    }

    /// Parse an expression statement or an assignment. Strategy from the spec:
    /// parse a full expression, then peek — if `=` follows, reinterpret it as an
    /// lvalue and emit AssignStmt; else emit ExprStmt.
    fn parse_expr_or_assign(&mut self) -> PResult<Stmt> {
        let span = self.span_here();
        let expr = self.parse_expr()?;
        if self.check(&TokenKind::Eq) {
            self.advance(); // consume '='
            let lvalue = self.expr_to_lvalue(expr)?;
            let value = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, "`;` after assignment")?;
            Ok(Stmt::Assign(AssignStmt {
                target: lvalue,
                value,
                span,
            }))
        } else {
            self.expect(TokenKind::Semicolon, "`;` after expression statement")?;
            Ok(Stmt::Expr(expr))
        }
    }

    fn expr_to_lvalue(&mut self, expr: Expr) -> PResult<LValue> {
        let span = expr.span;
        match expr.kind {
            ExprKind::Ident(name) => Ok(LValue::Ident(name, span)),
            ExprKind::Deref { operand } => Ok(LValue::Deref(operand, span)),
            ExprKind::Field { object, field } => Ok(LValue::Field(object, field, span)),
            _ => {
                let tok = Token {
                    kind: TokenKind::Eq,
                    lexeme: "=".into(),
                    line: span.line,
                    col: span.col,
                };
                self.error_at(&tok, "E201", "invalid assignment target", "not assignable");
                Err(ParseAbort)
            }
        }
    }

    // --- inline asm ----------------------------------------------------------

    /// Parse `!asm { ... }`. The body is reconstructed line-by-line from the
    /// token stream: `%IDENT` → Value, `&IDENT` → Addr, everything else → Raw
    /// text (NASM syntax), preserving token order within a source line.
    fn parse_asm(&mut self) -> PResult<AsmStmt> {
        let span = self.span_here();
        self.expect(TokenKind::Bang, "`!`")?;
        self.expect(TokenKind::Asm, "`asm`")?;
        self.expect(TokenKind::LBrace, "`{` to open asm block")?;

        let mut lines: Vec<AsmLine> = Vec::new();
        let mut current: Vec<AsmToken> = Vec::new();
        let mut current_line_no: Option<usize> = None;

        // Helper to flush the current line into `lines`.
        let flush = |lines: &mut Vec<AsmLine>, current: &mut Vec<AsmToken>| {
            if !current.is_empty() {
                lines.push(AsmLine {
                    tokens: std::mem::take(current),
                });
            }
        };

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            let tok = self.peek().clone();
            // New source line → flush previous.
            if let Some(prev) = current_line_no {
                if tok.line != prev {
                    flush(&mut lines, &mut current);
                }
            }
            current_line_no = Some(tok.line);

            match &tok.kind {
                TokenKind::Percent => {
                    self.advance();
                    let name = self
                        .expect(TokenKind::Ident, "variable name after `%`")?
                        .lexeme
                        .clone();
                    current.push(AsmToken::Value(name));
                }
                TokenKind::Ampersand => {
                    self.advance();
                    let name = self
                        .expect(TokenKind::Ident, "variable name after `&`")?
                        .lexeme
                        .clone();
                    current.push(AsmToken::Addr(name));
                }
                _ => {
                    self.advance();
                    // Each lexical token becomes its own Raw fragment; spacing is
                    // reapplied at emit time (NASM only needs whitespace between a
                    // mnemonic and word-like operands).
                    current.push(AsmToken::Raw(asm_raw_text(&tok)));
                }
            }
        }
        flush(&mut lines, &mut current);
        self.expect(TokenKind::RBrace, "`}` to close asm block")?;
        Ok(AsmStmt { lines, span })
    }

    // --- expressions ---------------------------------------------------------

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_cast(true)
    }

    /// Expression in condition position: struct initializers are disabled so
    /// `if x { ... }` parses `x` as the condition and `{ ... }` as the block.
    fn parse_expr_no_struct(&mut self) -> PResult<Expr> {
        self.parse_cast(false)
    }

    fn parse_cast(&mut self, allow_struct: bool) -> PResult<Expr> {
        let mut expr = self.parse_logical(allow_struct)?;
        let mut did_cast = false;
        while self.check(&TokenKind::Arrow) {
            let span = expr.span;
            self.advance();
            let target = self.parse_type()?;
            expr = Expr::new(
                ExprKind::Cast {
                    expr: Box::new(expr),
                    target_type: target,
                },
                span,
            );
            did_cast = true;
        }
        // `a -> T <binop> b` cannot reattach the operator: cast is the loosest
        // level and its operand is a type, not an expression. Point the user at
        // parentheses rather than emitting a confusing "expected `;`".
        if did_cast && self.is_binary_op_ahead() {
            let tok = self.peek().clone();
            self.error_at(
                &tok,
                "E202",
                "a binary operator cannot directly follow a cast; wrap the cast in parentheses",
                "wrap the cast: `(expr -> Type) <op> …`",
            );
            return Err(ParseAbort);
        }
        Ok(expr)
    }

    /// True if the current token is an infix binary operator.
    fn is_binary_op_ahead(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::EqEq
                | TokenKind::Ne
                | TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::Le
                | TokenKind::Ge
                | TokenKind::AmpAmp
                | TokenKind::PipePipe
        )
    }

    fn parse_logical(&mut self, allow_struct: bool) -> PResult<Expr> {
        let mut left = self.parse_comparison(allow_struct)?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::AmpAmp => BinaryOp::And,
                TokenKind::PipePipe => BinaryOp::Or,
                _ => break,
            };
            let span = left.span;
            self.advance();
            let right = self.parse_comparison(allow_struct)?;
            left = Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    fn parse_comparison(&mut self, allow_struct: bool) -> PResult<Expr> {
        let mut left = self.parse_additive(allow_struct)?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::EqEq => BinaryOp::Eq,
                TokenKind::Ne => BinaryOp::Ne,
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::Le => BinaryOp::Le,
                TokenKind::Ge => BinaryOp::Ge,
                _ => break,
            };
            let span = left.span;
            self.advance();
            let right = self.parse_additive(allow_struct)?;
            left = Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    fn parse_additive(&mut self, allow_struct: bool) -> PResult<Expr> {
        let mut left = self.parse_multiplicative(allow_struct)?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            let span = left.span;
            self.advance();
            let right = self.parse_multiplicative(allow_struct)?;
            left = Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self, allow_struct: bool) -> PResult<Expr> {
        let mut left = self.parse_unary(allow_struct)?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Mod,
                _ => break,
            };
            let span = left.span;
            self.advance();
            let right = self.parse_unary(allow_struct)?;
            left = Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    fn parse_unary(&mut self, allow_struct: bool) -> PResult<Expr> {
        let span = self.span_here();
        match self.peek_kind() {
            TokenKind::Bang => {
                self.advance();
                let operand = self.parse_unary(allow_struct)?;
                Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_unary(allow_struct)?;
                Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::Star => {
                self.advance();
                let operand = self.parse_unary(allow_struct)?;
                Ok(Expr::new(
                    ExprKind::Deref {
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::Ampersand => {
                self.advance();
                let mutable = self.eat(&TokenKind::Mut);
                let operand = self.parse_unary(allow_struct)?;
                Ok(Expr::new(
                    ExprKind::Ref {
                        mutable,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            _ => self.parse_postfix(allow_struct),
        }
    }

    fn parse_postfix(&mut self, allow_struct: bool) -> PResult<Expr> {
        let mut expr = self.parse_primary(allow_struct)?;
        loop {
            match self.peek_kind() {
                TokenKind::LParen => {
                    let span = expr.span;
                    self.advance();
                    let args = self.parse_arg_list()?;
                    self.expect(TokenKind::RParen, "`)` after arguments")?;
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    );
                }
                TokenKind::Dot => {
                    let span = expr.span;
                    self.advance();
                    let field = self
                        .expect(TokenKind::Ident, "field or method name after `.`")?
                        .lexeme
                        .clone();
                    expr = Expr::new(
                        ExprKind::Field {
                            object: Box::new(expr),
                            field,
                        },
                        span,
                    );
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_arg_list(&mut self) -> PResult<Vec<Expr>> {
        let mut args = Vec::new();
        if self.check(&TokenKind::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.check(&TokenKind::RParen) {
                break;
            }
        }
        Ok(args)
    }

    fn parse_primary(&mut self, allow_struct: bool) -> PResult<Expr> {
        let span = self.span_here();
        match self.peek_kind().clone() {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Expr::new(ExprKind::IntLit(v), span))
            }
            TokenKind::FloatLit(v) => {
                self.advance();
                Ok(Expr::new(ExprKind::FloatLit(v), span))
            }
            TokenKind::CharLit(cp) => {
                self.advance();
                Ok(Expr::new(ExprKind::CharLit(cp), span))
            }
            TokenKind::BoolLit(b) => {
                self.advance();
                Ok(Expr::new(ExprKind::BoolLit(b), span))
            }
            TokenKind::StringLit(s) => {
                self.advance();
                Ok(Expr::new(ExprKind::StringLit(s), span))
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "`)`")?;
                Ok(inner)
            }
            // `self` inside a method body is an ordinary value named "self".
            TokenKind::Self_ => {
                self.advance();
                Ok(Expr::new(ExprKind::Ident("self".to_string()), span))
            }
            // `null` is the sole value of the null type (used in stdlib/asm).
            TokenKind::Null => {
                self.advance();
                // Represent as an i64 zero at the machine level.
                Ok(Expr::new(ExprKind::IntLit(0), span))
            }
            // `i64::method` and `f64::method` — primitive type static calls.
            TokenKind::I64 | TokenKind::F64 => {
                let type_name = self.advance().lexeme.clone();
                if self.check(&TokenKind::ColonColon) {
                    self.advance();
                    let method = self
                        .expect(TokenKind::Ident, "method name after `::`")?
                        .lexeme
                        .clone();
                    return Ok(Expr::new(
                        ExprKind::QualifiedIdent {
                            module: type_name,
                            name: method,
                        },
                        span,
                    ));
                }
                self.error_here("expected an expression", "expected expression");
                Err(ParseAbort)
            }
            TokenKind::Ident => {
                let name = self.advance().lexeme.clone();
                // Qualified: IDENT :: IDENT
                if self.check(&TokenKind::ColonColon) {
                    self.advance();
                    let second = self
                        .expect(TokenKind::Ident, "name after `::`")?
                        .lexeme
                        .clone();
                    // qualified struct init?
                    if allow_struct && self.check(&TokenKind::LBrace) {
                        let sn = StructName::Qualified(name, second);
                        return self.parse_struct_init_body(sn, span);
                    }
                    return Ok(Expr::new(
                        ExprKind::QualifiedIdent {
                            module: name,
                            name: second,
                        },
                        span,
                    ));
                }
                // Unqualified struct init?
                if allow_struct && self.check(&TokenKind::LBrace) {
                    let sn = StructName::Unqualified(name);
                    return self.parse_struct_init_body(sn, span);
                }
                Ok(Expr::new(ExprKind::Ident(name), span))
            }
            _ => {
                self.error_here("expected an expression", "expected expression");
                Err(ParseAbort)
            }
        }
    }

    /// Parse the `{ field = expr, ... }` part of a struct initializer; the name
    /// and opening span are already known.
    fn parse_struct_init_body(&mut self, name: StructName, span: Span) -> PResult<Expr> {
        self.expect(TokenKind::LBrace, "`{` to open struct initializer")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            let fname = self.expect(TokenKind::Ident, "field name")?.lexeme.clone();
            self.expect(
                TokenKind::Eq,
                "`=` in struct field (fields use `=`, not `:`)",
            )?;
            let value = self.parse_expr()?;
            fields.push((fname, value));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace, "`}` to close struct initializer")?;
        Ok(Expr::new(ExprKind::StructInit { name, fields }, span))
    }
}

// ---------------------------------------------------------------------------
// Asm reconstruction helpers
// ---------------------------------------------------------------------------

/// The raw NASM text for an asm token. The lexer always records a faithful
/// `lexeme`, so we use it directly.
fn asm_raw_text(tok: &Token) -> String {
    tok.lexeme.clone()
}

fn describe_token(tok: &Token) -> String {
    match &tok.kind {
        TokenKind::Eof => "end of input".to_string(),
        TokenKind::Ident => format!("identifier `{}`", tok.lexeme),
        TokenKind::IntLit(_) => format!("integer `{}`", tok.lexeme),
        TokenKind::FloatLit(_) => format!("float `{}`", tok.lexeme),
        TokenKind::StringLit(_) => "string literal".to_string(),
        TokenKind::CharLit(_) => format!("char `{}`", tok.lexeme),
        TokenKind::BoolLit(b) => format!("`{}`", b),
        _ if !tok.lexeme.is_empty() => format!("`{}`", tok.lexeme),
        other => format!("{:?}", other),
    }
}
