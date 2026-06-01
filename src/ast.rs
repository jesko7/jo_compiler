//! Abstract syntax tree for Jo.
//!
//! The parser produces these nodes; the type checker fills in `Expr::ty`.
//! `Expr` is modelled as a struct carrying the `ExprKind` plus a `ty:
//! Option<Type>` (None until type-checked) and a source location for
//! diagnostics — this satisfies "every Expr variant carries a ty field"
//! without duplicating the field across the enum.
//!
//! Some node fields (e.g. certain `span`s, `ImportKind` payloads consumed by
//! the preprocessor rather than later passes) exist to mirror the documented
//! AST node reference even where a given pass does not read them.
#![allow(dead_code)]

pub use crate::types::Type;

// ---------------------------------------------------------------------------
// Source locations
// ---------------------------------------------------------------------------

/// A 1-based (line, col) position into the preprocessed source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(line: usize, col: usize) -> Span {
        Span { line, col }
    }
}

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Import(ImportDecl),
    Struct(StructDecl),
    Fn(FnDecl),
    Extend(ExtendDecl),
}

#[derive(Debug, Clone)]
pub struct ExtendDecl {
    /// The module path, if any (e.g. `extend stdlib::string { ... }` → module = Some("stdlib")).
    pub module: Option<String>,
    /// The bare struct name being extended.
    pub name: String,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub kind: ImportKind,
}

#[derive(Debug, Clone)]
pub enum ImportKind {
    Named {
        module: String,
        name: String,
        alias: Option<String>,
    },
    Glob {
        module: String,
    },
    Module {
        name: String,
    },
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub kind: ParamKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ParamKind {
    Named { name: String, ty: Type },
    SelfRef,    // &self
    SelfMutRef, // &mut self
    SelfMove,   // move self
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(LetStmt),
    Assign(AssignStmt),
    Return(ReturnStmt),
    If(IfStmt),
    While(WhileStmt),
    Break(Span),
    Continue(Span),
    Expr(Expr),
    Asm(AsmStmt),
}

#[derive(Debug, Clone)]
pub struct LetStmt {
    pub name: String,
    pub ty: Option<Type>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AssignStmt {
    pub target: LValue,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IfStmt {
    pub branches: Vec<(Expr, Block)>, // (condition, then-block) for if + each else-if
    pub else_block: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub condition: Expr,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum LValue {
    Ident(String, Span),
    Deref(Box<Expr>, Span),
    Field(Box<Expr>, String, Span),
}

#[derive(Debug, Clone)]
pub struct AsmStmt {
    pub lines: Vec<AsmLine>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AsmLine {
    pub tokens: Vec<AsmToken>,
}

#[derive(Debug, Clone)]
pub enum AsmToken {
    Raw(String),
    Value(String), // %name
    Addr(String),  // &name
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Option<Type>,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Expr {
        Expr {
            kind,
            ty: None,
            span,
        }
    }

    /// The resolved type. Panics if called before type checking — used only in
    /// codegen where every expression is guaranteed annotated.
    pub fn ty(&self) -> &Type {
        self.ty
            .as_ref()
            .expect("expression type not resolved (type-checker bug)")
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    IntLit(i64),
    FloatLit(f64),
    CharLit(u32),
    BoolLit(bool),
    StringLit(String),
    Ident(String),
    QualifiedIdent {
        module: String,
        name: String,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        target_type: Type,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Field {
        object: Box<Expr>,
        field: String,
    },
    Ref {
        mutable: bool,
        operand: Box<Expr>,
    },
    Deref {
        operand: Box<Expr>,
    },
    StructInit {
        name: StructName,
        fields: Vec<(String, Expr)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructName {
    Unqualified(String),
    Qualified(String, String),
}

impl StructName {
    /// The bare type name (the part used to look up the struct decl, after
    /// import resolution qualified names still resolve by their last segment).
    pub fn base(&self) -> &str {
        match self {
            StructName::Unqualified(n) => n,
            StructName::Qualified(_, n) => n,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    Deref,
    Ref,
    RefMut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

impl BinaryOp {
    /// The stdlib method name this operator desugars to.
    pub fn method_name(self) -> &'static str {
        match self {
            BinaryOp::Add => "add",
            BinaryOp::Sub => "sub",
            BinaryOp::Mul => "mul",
            BinaryOp::Div => "div",
            BinaryOp::Mod => "mod_",
            BinaryOp::Eq => "eq",
            BinaryOp::Ne => "ne",
            BinaryOp::Lt => "lt",
            BinaryOp::Gt => "gt",
            BinaryOp::Le => "le",
            BinaryOp::Ge => "ge",
            BinaryOp::And => "and",
            BinaryOp::Or => "or",
        }
    }
}

impl UnaryOp {
    /// The stdlib method name for the desugarable unary ops (`-`, `!`).
    pub fn method_name(self) -> Option<&'static str> {
        match self {
            UnaryOp::Neg => Some("neg"),
            UnaryOp::Not => Some("not"),
            _ => None,
        }
    }
}
