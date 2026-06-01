//! The `Type` enum, shared by the AST, type checker, and code generator.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// 64-bit signed integer machine type.
    I64,
    /// 64-bit IEEE-754 float machine type.
    F64,
    /// No value (function return only).
    Void,
    /// The null / absent-pointer type (zero-sized).
    Null,
    /// A named struct type in scope, e.g. `int`, `Person`.
    Named(String),
    /// A `module::name` qualified type.
    Qualified(String, String),
    /// `&T`
    Ref(Box<Type>),
    /// `&mut T`
    MutRef(Box<Type>),
}

impl Type {
    /// True if this is a machine (primitive) type: `i64`, `f64`, or `null`.
    pub fn is_machine(&self) -> bool {
        matches!(self, Type::I64 | Type::F64 | Type::Null)
    }

    /// If this is a reference (`&T` or `&mut T`), return the referent type.
    pub fn deref_target(&self) -> Option<&Type> {
        match self {
            Type::Ref(inner) | Type::MutRef(inner) => Some(inner),
            _ => None,
        }
    }

    /// True if this is `&T` or `&mut T`.
    pub fn is_ref(&self) -> bool {
        matches!(self, Type::Ref(_) | Type::MutRef(_))
    }

    /// A user-facing rendering used in error messages.
    pub fn display(&self) -> String {
        match self {
            Type::I64 => "i64".to_string(),
            Type::F64 => "f64".to_string(),
            Type::Void => "void".to_string(),
            Type::Null => "null".to_string(),
            Type::Named(n) => n.clone(),
            Type::Qualified(m, n) => format!("{}::{}", m, n),
            Type::Ref(inner) => format!("&{}", inner.display()),
            Type::MutRef(inner) => format!("&mut {}", inner.display()),
        }
    }
}
