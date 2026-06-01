//! The Jo type checker.
//!
//! Walks the parsed `Program`, building a struct/function table, then checks
//! every function body. During the walk it performs three rewrites in place:
//!   * literal desugaring  (`10` → `int { inner_value = 10 }`, etc.)
//!   * operator desugaring  (`a + b` → `a.add(&b)`, etc.)
//!   * cast desugaring      (`x -> float` → `x.to_float()`)
//! and annotates every `Expr` with its resolved `ty`.

use std::collections::HashMap;

use crate::ast::*;
use crate::error::Diagnostic;

pub struct TypeChecker {
    file: String,
    src: String,
    /// All struct declarations by name (includes stdlib + module structs).
    structs: HashMap<String, StructDecl>,
    /// All free functions by name.
    functions: HashMap<String, FnDecl>,
    /// Structs/functions reachable via `module::name` (module form imports).
    modules: HashMap<String, ModuleInfo>,
    primitive_methods: HashMap<String, Vec<FnDecl>>,
    diags: Vec<Diagnostic>,
}

#[derive(Default)]
struct ModuleInfo {
    structs: HashMap<String, StructDecl>,
    functions: HashMap<String, FnDecl>,
}

/// Lexical scope stack of variable name → type.
struct Scopes {
    stack: Vec<HashMap<String, Type>>,
}

impl Scopes {
    fn new() -> Scopes {
        Scopes {
            stack: vec![HashMap::new()],
        }
    }
    fn push(&mut self) {
        self.stack.push(HashMap::new());
    }
    fn pop(&mut self) {
        self.stack.pop();
    }
    fn declare(&mut self, name: String, ty: Type) {
        self.stack.last_mut().unwrap().insert(name, ty);
    }
    fn lookup(&self, name: &str) -> Option<&Type> {
        for scope in self.stack.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t);
            }
        }
        None
    }
}

impl TypeChecker {
    /// Type-check `program` in place. `module_asts` maps module name → its
    /// already-parsed `Program` (for `module::name` resolution).
    pub fn check(
        program: &mut Program,
        file: impl Into<String>,
        src: impl Into<String>,
        module_asts: &mut HashMap<String, Program>,
    ) -> Result<(), Vec<Diagnostic>> {
        let mut tc = TypeChecker {
            file: file.into(),
            src: src.into(),
            structs: HashMap::new(),
            functions: HashMap::new(),
            modules: HashMap::new(),
            primitive_methods: HashMap::new(),
            diags: Vec::new(),
        };

        // Collect top-level declarations.
        let mut top_level_names: HashMap<String, Span> = HashMap::new();
        for item in &program.items {
            match item {
                Item::Struct(s) => {
                    if let Some(first) = top_level_names.insert(s.name.clone(), s.span) {
                        tc.duplicate_name_error(s.span, &s.name, "top-level item", first);
                    } else {
                        tc.structs.insert(s.name.clone(), s.clone());
                    }
                }
                Item::Fn(f) => {
                    if let Some(first) = top_level_names.insert(f.name.clone(), f.span) {
                        tc.duplicate_name_error(f.span, &f.name, "top-level item", first);
                    } else {
                        tc.functions.insert(f.name.clone(), f.clone());
                    }
                }
                Item::Import(_) | Item::Extend(_) => {}
            }
        }
        // Collect module declarations.
        for (mod_name, mod_prog) in module_asts.iter() {
            let mut info = ModuleInfo::default();
            let mut module_names: HashMap<String, Span> = HashMap::new();
            for item in &mod_prog.items {
                match item {
                    Item::Struct(s) => {
                        if let Some(first) = module_names.insert(s.name.clone(), s.span) {
                            tc.duplicate_name_error(s.span, &s.name, "module item", first);
                        } else {
                            info.structs.insert(s.name.clone(), s.clone());
                        }
                    }
                    Item::Fn(f) => {
                        if let Some(first) = module_names.insert(f.name.clone(), f.span) {
                            tc.duplicate_name_error(f.span, &f.name, "module item", first);
                        } else {
                            info.functions.insert(f.name.clone(), f.clone());
                        }
                    }
                    Item::Import(_) | Item::Extend(_) => {}
                }
            }
            tc.modules.insert(mod_name.clone(), info);
        }

        // Merge extend declarations into the target struct's method list.
        // Must run after all structs (and modules) are collected.
        for item in &program.items {
            if let Item::Extend(e) = item {
                let duplicate_methods = {
                    let target = match &e.module {
                        Some(mod_name) => {
                            if let Some(info) = tc.modules.get_mut(mod_name) {
                                if let Some(s) = info.structs.get_mut(&e.name) {
                                    s
                                } else {
                                    tc.diags.push(
                                        crate::error::Diagnostic::new(
                                            "E4D0",
                                            format!(
                                                "extend target `{}::{}` not found",
                                                mod_name, e.name
                                            ),
                                            tc.file.clone(),
                                            e.span.line,
                                            e.span.col,
                                            crate::error::source_line_of(&tc.src, e.span.line),
                                        )
                                        .with_label("unknown struct".to_string()),
                                    );
                                    continue;
                                }
                            } else {
                                tc.diags.push(
                                    crate::error::Diagnostic::new(
                                        "E4D0",
                                        format!(
                                            "extend target `{}::{}` not found (module not imported)",
                                            mod_name, e.name
                                        ),
                                        tc.file.clone(),
                                        e.span.line,
                                        e.span.col,
                                        crate::error::source_line_of(&tc.src, e.span.line),
                                    )
                                    .with_label("unknown module".to_string()),
                                );
                                continue;
                            }
                        }
                        None if is_primitive_extend_target(&e.name) => {
                            let methods = tc.primitive_methods.entry(e.name.clone()).or_default();
                            let mut method_names: HashMap<String, Span> =
                                methods.iter().map(|m| (m.name.clone(), m.span)).collect();
                            let mut duplicates = Vec::new();
                            for method in &e.methods {
                                if let Some(first) =
                                    method_names.insert(method.name.clone(), method.span)
                                {
                                    duplicates.push((method.span, method.name.clone(), first));
                                }
                            }
                            methods.extend(e.methods.clone());
                            for (span, name, first) in duplicates {
                                tc.duplicate_name_error(span, &name, "method", first);
                            }
                            continue;
                        }
                        None => {
                            if let Some(s) = tc.structs.get_mut(&e.name) {
                                s
                            } else {
                                tc.diags.push(
                                    crate::error::Diagnostic::new(
                                        "E4D0",
                                        format!("extend target `{}` not found", e.name),
                                        tc.file.clone(),
                                        e.span.line,
                                        e.span.col,
                                        crate::error::source_line_of(&tc.src, e.span.line),
                                    )
                                    .with_label("unknown struct".to_string()),
                                );
                                continue;
                            }
                        }
                    };
                    let mut method_names: HashMap<String, Span> = target
                        .methods
                        .iter()
                        .map(|m| (m.name.clone(), m.span))
                        .collect();
                    let mut duplicates = Vec::new();
                    for method in &e.methods {
                        if let Some(first) = method_names.insert(method.name.clone(), method.span) {
                            duplicates.push((method.span, method.name.clone(), first));
                        }
                    }
                    target.methods.extend(e.methods.clone());
                    duplicates
                };
                for (span, name, first) in duplicate_methods {
                    tc.duplicate_name_error(span, &name, "method", first);
                }
            }
        }

        // Check every function body (free functions and struct methods).
        // We clone the items out, check, and write annotations back.
        let mut items = std::mem::take(&mut program.items);
        for item in &mut items {
            match item {
                Item::Fn(f) => tc.check_fn(f, None, false),
                Item::Struct(s) => {
                    let struct_name = s.name.clone();
                    let suppress = tc.struct_is_machine_backed(&struct_name);
                    let mut methods = std::mem::take(&mut s.methods);
                    for m in &mut methods {
                        tc.check_fn(m, Some(&struct_name), suppress);
                    }
                    s.methods = methods;
                }
                Item::Extend(e) => {
                    let struct_name = e.name.clone();
                    let suppress = is_primitive_extend_target(&struct_name);
                    for m in &mut e.methods {
                        tc.check_fn(m, Some(&struct_name), suppress);
                    }
                }
                Item::Import(_) => {}
            }
        }
        program.items = items;

        // Type-check (and desugar) the bodies of loaded modules too, so that
        // `import module;` references resolve and codegen can emit their
        // functions/methods. Their structs/methods are already registered in the
        // module tables, reachable via `lookup_struct`'s module fallback.
        for mod_prog in module_asts.values_mut() {
            let mut mod_items = std::mem::take(&mut mod_prog.items);
            for item in &mut mod_items {
                match item {
                    Item::Fn(f) => tc.check_fn(f, None, false),
                    Item::Struct(s) => {
                        let struct_name = s.name.clone();
                        let suppress = tc.struct_is_machine_backed(&struct_name);
                        let mut methods = std::mem::take(&mut s.methods);
                        for m in &mut methods {
                            tc.check_fn(m, Some(&struct_name), suppress);
                        }
                        s.methods = methods;
                    }
                    Item::Extend(e) => {
                        let struct_name = e.name.clone();
                        let suppress = is_primitive_extend_target(&struct_name);
                        for m in &mut e.methods {
                            tc.check_fn(m, Some(&struct_name), suppress);
                        }
                    }
                    Item::Import(_) => {}
                }
            }
            mod_prog.items = mod_items;
        }

        // Validate that `main` exists and has a valid signature.
        tc.check_main();

        if tc.diags.is_empty() {
            Ok(())
        } else {
            Err(tc.diags)
        }
    }

    fn check_main(&mut self) {
        let (ret, span, nparams) = match self.functions.get("main") {
            Some(m) => (m.return_type.clone(), m.span, m.params.len()),
            None => {
                self.diags.push(Diagnostic::new(
                    "E400",
                    "no `main` function found",
                    self.file.clone(),
                    1,
                    1,
                    String::new(),
                ));
                return;
            }
        };
        let ok = matches!(&ret, Type::I64)
            || matches!(&ret, Type::Named(n) if n == "int")
            || matches!(&ret, Type::Void);
        if !ok {
            self.error(
                span,
                "E401",
                format!(
                    "`main` must return `i64`, `int`, or `void`, found `{}`",
                    ret.display()
                ),
                "invalid main signature",
            );
        }
        if nparams != 0 {
            self.error(
                span,
                "E402",
                "`main` must take no parameters",
                "remove parameters",
            );
        }
    }

    // -----------------------------------------------------------------------
    // Struct lookup helpers
    // -----------------------------------------------------------------------

    /// Look up a struct by its bare name. Searches the top-level namespace
    /// first, then any loaded module (so `module::name` value types resolve
    /// their fields/methods even when accessed by the unqualified base name).
    fn lookup_struct(&self, name: &str) -> Option<&StructDecl> {
        if let Some(s) = self.structs.get(name) {
            return Some(s);
        }
        for info in self.modules.values() {
            if let Some(s) = info.structs.get(name) {
                return Some(s);
            }
        }
        None
    }

    fn lookup_struct_qualified(&self, module: &str, name: &str) -> Option<&StructDecl> {
        self.modules.get(module).and_then(|m| m.structs.get(name))
    }

    fn type_name_exists(&self, name: &str) -> bool {
        matches!(name, "i64" | "f64" | "void" | "null") || self.lookup_struct(name).is_some()
    }

    /// Does this struct have any machine-type field? (Determines literal
    /// desugaring suppression inside its method bodies.)
    fn struct_is_machine_backed(&self, name: &str) -> bool {
        self.lookup_struct(name)
            .map(|s| s.fields.iter().any(|f| f.ty.is_machine()))
            .unwrap_or(false)
    }

    fn find_method<'s>(&'s self, struct_name: &str, method: &str) -> Option<&'s FnDecl> {
        self.lookup_struct(struct_name)
            .and_then(|s| s.methods.iter().find(|m| m.name == method))
            .or_else(|| {
                self.primitive_methods
                    .get(struct_name)
                    .and_then(|methods| methods.iter().find(|m| m.name == method))
            })
    }

    // -----------------------------------------------------------------------
    // Function checking
    // -----------------------------------------------------------------------

    fn check_fn(&mut self, f: &mut FnDecl, enclosing_struct: Option<&str>, suppress: bool) {
        let mut scopes = Scopes::new();

        // Resolve & declare parameters.
        for p in &mut f.params {
            match &mut p.kind {
                ParamKind::Named { name, ty } => {
                    scopes.declare(name.clone(), ty.clone());
                }
                ParamKind::SelfRef => {
                    let st = enclosing_struct.unwrap_or("");
                    scopes.declare("self".to_string(), Type::Ref(Box::new(self_type(st))));
                }
                ParamKind::SelfMutRef => {
                    let st = enclosing_struct.unwrap_or("");
                    scopes.declare("self".to_string(), Type::MutRef(Box::new(self_type(st))));
                }
                ParamKind::SelfMove => {
                    let st = enclosing_struct.unwrap_or("");
                    scopes.declare("self".to_string(), self_type(st));
                }
            }
        }

        let return_type = f.return_type.clone();
        let mut ctx = FnCtx {
            return_type: return_type.clone(),
            suppress_desugar: suppress,
            loop_depth: 0,
            enclosing_struct: enclosing_struct.map(|s| s.to_string()),
        };

        let mut body = std::mem::replace(&mut f.body, Block { stmts: Vec::new() });
        self.check_block(&mut body, &mut scopes, &mut ctx);

        // Exhaustive return checking for non-void functions.
        if !matches!(return_type, Type::Void) {
            if !block_definitely_returns(&body) {
                let span = f.span;
                self.error(
                    span,
                    "E410",
                    "not all paths return a value",
                    "missing return",
                );
            }
        }

        f.body = body;
    }

    fn check_block(&mut self, block: &mut Block, scopes: &mut Scopes, ctx: &mut FnCtx) {
        scopes.push();
        let mut stmts = std::mem::take(&mut block.stmts);
        for stmt in &mut stmts {
            self.check_stmt(stmt, scopes, ctx);
        }
        block.stmts = stmts;
        scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &mut Stmt, scopes: &mut Scopes, ctx: &mut FnCtx) {
        match stmt {
            Stmt::Let(l) => {
                if self.type_name_exists(&l.name) {
                    self.error(
                        l.span,
                        "E411",
                        format!("let binding `{}` conflicts with a type name", l.name),
                        "name already used by a type",
                    );
                }
                // Explicit machine-type annotation suppresses literal desugaring
                // for the initializer's top-level literal.
                let hint = l.ty.clone();
                let ty = self.check_expr(&mut l.init, scopes, ctx, hint.as_ref());
                match &l.ty {
                    Some(declared) => {
                        if !matches!(ty, Type::Void) && !self.types_compatible(declared, &ty) {
                            let span = l.span;
                            self.error(
                                span,
                                "E420",
                                format!(
                                    "type mismatch in let binding: expected `{}`, found `{}`",
                                    declared.display(),
                                    ty.display()
                                ),
                                "type mismatch",
                            );
                        }
                        scopes.declare(l.name.clone(), declared.clone());
                    }
                    None => {
                        scopes.declare(l.name.clone(), ty);
                    }
                }
            }
            Stmt::Assign(a) => {
                let target_ty = self.check_lvalue(&mut a.target, scopes, ctx);
                let value_ty = self.check_expr(&mut a.value, scopes, ctx, target_ty.as_ref());
                if let Some(tt) = target_ty {
                    if !matches!(value_ty, Type::Void) && !self.types_compatible(&tt, &value_ty) {
                        let span = a.span;
                        self.error(
                            span,
                            "E421",
                            format!(
                                "type mismatch in assignment: expected `{}`, found `{}`",
                                tt.display(),
                                value_ty.display()
                            ),
                            "type mismatch",
                        );
                    }
                }
            }
            Stmt::Return(r) => {
                let span = r.span;
                if matches!(ctx.return_type, Type::Void) {
                    if r.value.is_some() {
                        // type error: return expr in void fn
                        if let Some(v) = &mut r.value {
                            self.check_expr(v, scopes, ctx, None);
                        }
                        self.error(
                            span,
                            "E430",
                            "`return expr;` in a void function",
                            "void function",
                        );
                    }
                } else {
                    match &mut r.value {
                        None => {
                            self.error(
                                span,
                                "E431",
                                "bare `return;` in a non-void function",
                                "expected a return value",
                            );
                        }
                        Some(v) => {
                            let expected = ctx.return_type.clone();
                            let vt = self.check_expr(v, scopes, ctx, Some(&expected));
                            if !matches!(vt, Type::Void) && !self.types_compatible(&expected, &vt) {
                                self.error(
                                    span,
                                    "E432",
                                    format!(
                                        "return type mismatch: expected `{}`, found `{}`",
                                        expected.display(),
                                        vt.display()
                                    ),
                                    "type mismatch",
                                );
                            }
                        }
                    }
                }
            }
            Stmt::If(if_stmt) => {
                for (cond, blk) in &mut if_stmt.branches {
                    let ct = self.check_expr(cond, scopes, ctx, None);
                    self.expect_bool(&ct, cond.span);
                    self.check_block(blk, scopes, ctx);
                }
                if let Some(eb) = &mut if_stmt.else_block {
                    self.check_block(eb, scopes, ctx);
                }
            }
            Stmt::While(w) => {
                let ct = self.check_expr(&mut w.condition, scopes, ctx, None);
                self.expect_bool(&ct, w.condition.span);
                ctx.loop_depth += 1;
                self.check_block(&mut w.body, scopes, ctx);
                ctx.loop_depth -= 1;
            }
            Stmt::Break(span) => {
                if ctx.loop_depth == 0 {
                    self.error(*span, "E440", "`break` outside of a loop", "not in a loop");
                }
            }
            Stmt::Continue(span) => {
                if ctx.loop_depth == 0 {
                    self.error(
                        *span,
                        "E441",
                        "`continue` outside of a loop",
                        "not in a loop",
                    );
                }
            }
            Stmt::Expr(e) => {
                self.check_expr(e, scopes, ctx, None);
            }
            Stmt::Asm(a) => {
                self.check_asm(a, scopes);
            }
        }
    }

    fn check_asm(&mut self, asm: &AsmStmt, scopes: &Scopes) {
        for line in &asm.lines {
            for tok in &line.tokens {
                match tok {
                    AsmToken::Value(name) | AsmToken::Addr(name) => {
                        if scopes.lookup(name).is_none() {
                            self.error(
                                asm.span,
                                "E450",
                                format!("unknown variable `{}` in inline assembly", name),
                                "not in scope",
                            );
                        }
                    }
                    AsmToken::Raw(_) => {}
                }
            }
        }
    }

    fn check_lvalue(
        &mut self,
        lv: &mut LValue,
        scopes: &mut Scopes,
        ctx: &mut FnCtx,
    ) -> Option<Type> {
        match lv {
            LValue::Ident(name, span) => match scopes.lookup(name) {
                Some(t) => Some(t.clone()),
                None => {
                    self.error(
                        *span,
                        "E460",
                        format!("undeclared variable `{}`", name),
                        "not found",
                    );
                    None
                }
            },
            LValue::Deref(e, _span) => {
                let t = self.check_expr(e, scopes, ctx, None);
                match t.deref_target() {
                    Some(inner) => Some(inner.clone()),
                    None => {
                        self.error(
                            e.span,
                            "E461",
                            format!("cannot dereference non-reference type `{}`", t.display()),
                            "not a reference",
                        );
                        None
                    }
                }
            }
            LValue::Field(obj, field, span) => {
                let obj_ty = self.check_expr(obj, scopes, ctx, None);
                self.field_type(&obj_ty, field, *span)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expression checking + desugaring
    // -----------------------------------------------------------------------

    /// Check `expr`, rewriting it in place (literal/operator/cast desugaring)
    /// and setting `expr.ty`. Returns the resolved type.
    ///
    /// `hint` is the expected type, used solely to keep a bare literal raw when
    /// an explicit machine type (`i64`/`f64`) is expected.
    fn check_expr(
        &mut self,
        expr: &mut Expr,
        scopes: &mut Scopes,
        ctx: &mut FnCtx,
        hint: Option<&Type>,
    ) -> Type {
        let ty = self.check_expr_inner(expr, scopes, ctx, hint);
        expr.ty = Some(ty.clone());
        ty
    }

    fn check_expr_inner(
        &mut self,
        expr: &mut Expr,
        scopes: &mut Scopes,
        ctx: &mut FnCtx,
        hint: Option<&Type>,
    ) -> Type {
        let span = expr.span;
        match &mut expr.kind {
            // --- literals ---------------------------------------------------
            ExprKind::IntLit(n) => {
                let n = *n;
                if self.keep_literal_raw(hint, ctx, Type::I64)
                    || !self.wrapper_available("int", span)
                {
                    Type::I64
                } else {
                    expr.kind =
                        make_struct_init_int("int", "inner_value", ExprKind::IntLit(n), span);
                    self.annotate_struct_init(expr, scopes, ctx);
                    Type::Named("int".to_string())
                }
            }
            ExprKind::FloatLit(f) => {
                let f = *f;
                if self.keep_literal_raw(hint, ctx, Type::F64)
                    || !self.wrapper_available("float", span)
                {
                    Type::F64
                } else {
                    expr.kind = make_struct_init_float("float", "inner_value", f, span);
                    self.annotate_struct_init(expr, scopes, ctx);
                    Type::Named("float".to_string())
                }
            }
            ExprKind::CharLit(cp) => {
                let cp = *cp;
                if ctx.suppress_desugar || !self.wrapper_available("char", span) {
                    Type::I64
                } else {
                    expr.kind = make_struct_init_int(
                        "char",
                        "inner_value",
                        ExprKind::IntLit(cp as i64),
                        span,
                    );
                    self.annotate_struct_init(expr, scopes, ctx);
                    Type::Named("char".to_string())
                }
            }
            ExprKind::BoolLit(b) => {
                let b = *b;
                if ctx.suppress_desugar || !self.wrapper_available("bool", span) {
                    Type::I64
                } else {
                    let v = if b { 1 } else { 0 };
                    expr.kind =
                        make_struct_init_int("bool", "inner_value", ExprKind::IntLit(v), span);
                    self.annotate_struct_init(expr, scopes, ctx);
                    Type::Named("bool".to_string())
                }
            }
            ExprKind::StringLit(_) => {
                // Inside the desugared `string { ptr = <lit>, length = n }`, the
                // `ptr` field carries the raw literal with an `i64` hint — keep
                // it raw (it represents the .rodata address). Same in stdlib
                // bodies. Otherwise wrap into the `string` struct.
                if ctx.suppress_desugar
                    || matches!(hint, Some(Type::I64))
                    || !self.wrapper_available("string", span)
                {
                    Type::I64
                } else {
                    let s = match &expr.kind {
                        ExprKind::StringLit(s) => s.clone(),
                        _ => unreachable!(),
                    };
                    let byte_len = s.as_bytes().len() as i64;
                    // string { ptr = <addr literal>, length = byte_len }
                    // We keep the string content in a StringLit child of the ptr
                    // field so codegen can emit the .rodata and take its address.
                    let ptr_expr = Expr::new(ExprKind::StringLit(s), span);
                    let len_expr = Expr::new(ExprKind::IntLit(byte_len), span);
                    expr.kind = ExprKind::StructInit {
                        name: StructName::Unqualified("string".to_string()),
                        fields: vec![
                            ("ptr".to_string(), ptr_expr),
                            ("length".to_string(), len_expr),
                        ],
                    };
                    self.annotate_struct_init(expr, scopes, ctx);
                    Type::Named("string".to_string())
                }
            }

            // --- identifiers ------------------------------------------------
            ExprKind::Ident(name) => match scopes.lookup(name) {
                Some(t) => t.clone(),
                None => {
                    self.error(
                        span,
                        "E460",
                        format!("undeclared variable `{}`", name),
                        "not found",
                    );
                    Type::Void
                }
            },
            ExprKind::QualifiedIdent { module, name } => {
                // Resolve module::name as a value/function reference. In v1 this
                // is only meaningful as a callee for a free function.
                if let Some(info) = self.modules.get(module) {
                    if let Some(f) = info.functions.get(name) {
                        // Represent as a function value via its return type only
                        // when called; here just report it's callable.
                        let _ = f;
                        Type::Void
                    } else {
                        self.error(
                            span,
                            "E462",
                            format!("`{}` not found in module `{}`", name, module),
                            "not found",
                        );
                        Type::Void
                    }
                } else {
                    self.error(
                        span,
                        "E463",
                        format!("unknown module `{}`", module),
                        "unknown module",
                    );
                    Type::Void
                }
            }

            // --- references -------------------------------------------------
            ExprKind::Ref { mutable, operand } => {
                let inner = self.check_expr(operand, scopes, ctx, None);
                if *mutable {
                    Type::MutRef(Box::new(inner))
                } else {
                    Type::Ref(Box::new(inner))
                }
            }
            ExprKind::Deref { operand } => {
                let t = self.check_expr(operand, scopes, ctx, None);
                match t.deref_target() {
                    Some(inner) => inner.clone(),
                    None => {
                        self.error(
                            span,
                            "E461",
                            format!("cannot dereference non-reference type `{}`", t.display()),
                            "not a reference",
                        );
                        Type::Void
                    }
                }
            }

            // --- unary (desugars to method calls, except in raw contexts) ---
            ExprKind::Unary { op, operand } => {
                let op = *op;
                let operand_ty = self.check_expr(operand, scopes, ctx, None);
                // In raw stdlib contexts, `-`/`!` on i64 stay as machine ops.
                if ctx.suppress_desugar && operand_ty.is_machine() {
                    return operand_ty;
                }
                let method = match op.method_name() {
                    Some(m) => m,
                    None => {
                        // Deref/Ref/RefMut shouldn't appear here (handled above).
                        self.error(span, "E470", "invalid unary operator", "invalid");
                        return Type::Void;
                    }
                };
                self.desugar_unary_to_method(expr, &operand_ty, method, span)
            }

            // --- binary (desugars to method calls, except in raw contexts) --
            ExprKind::Binary { op, left, right } => {
                let op = *op;
                let left_ty = self.check_expr(left, scopes, ctx, None);
                let right_ty = self.check_expr(right, scopes, ctx, None);
                if ctx.suppress_desugar && left_ty.is_machine() {
                    // Raw machine arithmetic stays as a BinaryExpr; result type
                    // matches the left operand (i64 or f64).
                    return left_ty;
                }
                self.desugar_binary_to_method(expr, op, &left_ty, &right_ty, span)
            }

            // --- casts ------------------------------------------------------
            ExprKind::Cast {
                expr: inner,
                target_type,
            } => {
                let target = target_type.clone();
                let src_ty = self.check_expr(inner, scopes, ctx, None);
                self.desugar_cast_to_method(expr, &src_ty, &target, span, scopes, ctx)
            }

            // --- calls ------------------------------------------------------
            ExprKind::Call { .. } => self.check_call(expr, scopes, ctx),

            // --- field access -----------------------------------------------
            ExprKind::Field { object, field } => {
                let field = field.clone();
                let obj_ty = self.check_expr(object, scopes, ctx, None);
                self.field_type(&obj_ty, &field, span).unwrap_or(Type::Void)
            }

            // --- struct init ------------------------------------------------
            ExprKind::StructInit { .. } => self.annotate_struct_init(expr, scopes, ctx),
        }
    }

    /// True when a bare literal should be kept raw (no desugaring).
    fn keep_literal_raw(&self, hint: Option<&Type>, ctx: &FnCtx, lit_machine: Type) -> bool {
        if ctx.suppress_desugar {
            return true;
        }
        // Explicit `let x: i64 = 10;` / `f64` annotation keeps the literal raw.
        matches!(hint, Some(h) if *h == lit_machine)
    }

    /// A literal can only be wrapped into its stdlib struct (`int`, `float`, …)
    /// if that struct is actually in scope. If it is missing, report an error
    /// (the user forgot to import it) and return false — keeping the literal raw
    /// so desugaring does not recurse forever building `int { inner_value =
    /// int { inner_value = … } }`.
    fn wrapper_available(&mut self, struct_name: &str, span: Span) -> bool {
        if self.lookup_struct(struct_name).is_some() {
            return true;
        }
        self.error(
            span,
            "E4C0",
            format!(
                "type `{}` is not in scope (did you forget to `import stdlib::{}` or `import stdlib::*`?)",
                struct_name, struct_name
            ),
            "unknown type",
        );
        false
    }

    /// Desugar `-a`/`!a` into `a.neg()`/`a.not()` and check it.
    fn desugar_unary_to_method(
        &mut self,
        expr: &mut Expr,
        operand_ty: &Type,
        method: &str,
        span: Span,
    ) -> Type {
        // Pull out the operand.
        let operand = match &mut expr.kind {
            ExprKind::Unary { operand, .. } => {
                std::mem::replace(operand, Box::new(dummy_expr(span)))
            }
            _ => unreachable!(),
        };
        let recv_struct = match self.receiver_struct_name(operand_ty) {
            Some(n) => n,
            None => {
                self.error(
                    span,
                    "E471",
                    format!(
                        "type `{}` does not support this operator",
                        operand_ty.display()
                    ),
                    "no such operator",
                );
                return Type::Void;
            }
        };
        let ret = match self.find_method(&recv_struct, method) {
            Some(m) => m.return_type.clone(),
            None => {
                self.error(
                    span,
                    "E472",
                    format!(
                        "type `{}` does not implement method `{}`",
                        recv_struct, method
                    ),
                    "method not found",
                );
                return Type::Void;
            }
        };
        // Build `operand.method()`.
        let callee = Expr {
            kind: ExprKind::Field {
                object: operand,
                field: method.to_string(),
            },
            ty: Some(Type::Void),
            span,
        };
        expr.kind = ExprKind::Call {
            callee: Box::new(callee),
            args: Vec::new(),
        };
        ret
    }

    /// Desugar `a OP b` into `a.method(b_ref)` and check it.
    fn desugar_binary_to_method(
        &mut self,
        expr: &mut Expr,
        op: BinaryOp,
        left_ty: &Type,
        right_ty: &Type,
        span: Span,
    ) -> Type {
        let method = op.method_name();
        let (left, right) = match &mut expr.kind {
            ExprKind::Binary { left, right, .. } => (
                std::mem::replace(left, Box::new(dummy_expr(span))),
                std::mem::replace(right, Box::new(dummy_expr(span))),
            ),
            _ => unreachable!(),
        };

        let recv_struct = match self.receiver_struct_name(left_ty) {
            Some(n) => n,
            None => {
                self.error(
                    span,
                    "E471",
                    format!(
                        "type `{}` does not support operator `{}`",
                        left_ty.display(),
                        method
                    ),
                    "no such operator",
                );
                return Type::Void;
            }
        };
        let method_decl = match self.find_method(&recv_struct, method) {
            Some(m) => m.clone(),
            None => {
                self.error(
                    span,
                    "E472",
                    format!(
                        "type `{}` does not implement method `{}`",
                        recv_struct, method
                    ),
                    "method not found",
                );
                return Type::Void;
            }
        };

        // Normalise the right operand into a reference per spec.
        let right_arg = self.normalize_arg_ref(*right, right_ty, span);

        // Type-check the argument against the method's (non-self) parameter.
        if let Some(param_ty) = method_decl.params.iter().find_map(|p| match &p.kind {
            ParamKind::Named { ty, .. } => Some(ty.clone()),
            _ => None,
        }) {
            let arg_ty = right_arg.ty.clone().unwrap_or(Type::Void);
            if !self.types_compatible(&param_ty, &arg_ty) {
                self.error(
                    span,
                    "E473",
                    format!(
                        "operator argument mismatch: method `{}` expects `{}`, found `{}`",
                        method,
                        param_ty.display(),
                        arg_ty.display()
                    ),
                    "type mismatch",
                );
            }
        }

        let callee = Expr {
            kind: ExprKind::Field {
                object: left,
                field: method.to_string(),
            },
            ty: Some(Type::Void),
            span,
        };
        expr.kind = ExprKind::Call {
            callee: Box::new(callee),
            args: vec![right_arg],
        };
        method_decl.return_type
    }

    /// Wrap `arg` (already type-checked, with `arg.ty` set) into a reference
    /// according to: T → &arg, &T → arg, &&T → *arg.
    fn normalize_arg_ref(&mut self, arg: Expr, arg_ty: &Type, span: Span) -> Expr {
        match arg_ty {
            Type::Ref(inner) | Type::MutRef(inner) => {
                if inner.is_ref() {
                    // &&T → *arg
                    let result_ty = (**inner).clone();
                    Expr {
                        kind: ExprKind::Deref {
                            operand: Box::new(arg),
                        },
                        ty: Some(result_ty),
                        span,
                    }
                } else {
                    // &T → as-is
                    arg
                }
            }
            _ => {
                // T → &arg
                let ref_ty = Type::Ref(Box::new(arg_ty.clone()));
                Expr {
                    kind: ExprKind::Ref {
                        mutable: false,
                        operand: Box::new(arg),
                    },
                    ty: Some(ref_ty),
                    span,
                }
            }
        }
    }

    /// Desugar `e -> T` into `e.to_T()` and check it.
    fn desugar_cast_to_method(
        &mut self,
        expr: &mut Expr,
        src_ty: &Type,
        target: &Type,
        span: Span,
        _scopes: &mut Scopes,
        ctx: &mut FnCtx,
    ) -> Type {
        let inner = match &mut expr.kind {
            ExprKind::Cast { expr: inner, .. } => {
                std::mem::replace(inner, Box::new(dummy_expr(span)))
            }
            _ => unreachable!(),
        };

        let target_name = match target {
            Type::I64 | Type::F64 if ctx.suppress_desugar && self.types_compatible(target, src_ty) => {
                expr.kind = inner.kind;
                return target.clone();
            }
            Type::I64 | Type::F64 => {
                let target = target.clone();
                if self.single_field_wrapper_type(src_ty).as_ref() == Some(&target) {
                    let field_name = self
                        .receiver_struct_name(src_ty)
                        .and_then(|name| self.lookup_struct(&name))
                        .and_then(|decl| decl.fields.first())
                        .map(|field| field.name.clone());
                    if let Some(field) = field_name {
                        expr.kind = ExprKind::Field {
                            object: inner,
                            field,
                        };
                        return target;
                    }
                }
                self.error(
                    span,
                    "E481",
                    format!("invalid cast target `{}`", target.display()),
                    "invalid cast target",
                );
                return Type::Void;
            }
            Type::Named(n) => n.clone(),
            Type::Qualified(_, n) => n.clone(),
            other => {
                self.error(
                    span,
                    "E481",
                    format!("invalid cast target `{}`", other.display()),
                    "invalid cast target",
                );
                return Type::Void;
            }
        };

        // Boxing cast: a raw machine value (`i64`/`f64`/`null`) cast to a stdlib
        // wrapper struct whose single field has that machine type. There is no
        // `to_T` method on a primitive, so we wrap directly (like `T::new(v)`).
        // This is the natural inverse of reading a struct's machine field.
        if src_ty.is_machine() {
            if let Some(field_name) = self.single_field_of_type(&target_name, src_ty) {
                expr.kind = ExprKind::StructInit {
                    name: StructName::Unqualified(target_name.clone()),
                    fields: vec![(field_name, *inner)],
                };
                // The wrapped field is already a raw machine value; re-checking
                // the StructInit validates and annotates it.
                return self.annotate_struct_init(expr, _scopes, ctx);
            }
            // No suitable wrapper → genuine error.
            self.error(
                span,
                "E480",
                format!(
                    "cannot cast from `{}` to `{}`",
                    src_ty.display(),
                    target_name
                ),
                "invalid cast",
            );
            return Type::Void;
        }

        let recv_struct = match self.receiver_struct_name(src_ty) {
            Some(n) => n,
            None => {
                self.error(
                    span,
                    "E480",
                    format!("cannot cast from `{}`", src_ty.display()),
                    "invalid cast",
                );
                return Type::Void;
            }
        };
        let method = format!("to_{}", target_name);
        let ret = match self.find_method(&recv_struct, &method) {
            Some(m) => m.return_type.clone(),
            None => {
                self.error(
                    span,
                    "E482",
                    format!(
                        "type `{}` cannot be cast to `{}` (no method `{}`)",
                        recv_struct, target_name, method
                    ),
                    "no cast method",
                );
                return Type::Void;
            }
        };
        let callee = Expr {
            kind: ExprKind::Field {
                object: inner,
                field: method,
            },
            ty: Some(Type::Void),
            span,
        };
        expr.kind = ExprKind::Call {
            callee: Box::new(callee),
            args: Vec::new(),
        };
        ret
    }

    /// If struct `name` has exactly one field and that field's type equals
    /// `ty`, return the field's name. Used for boxing casts (`i64 -> int`).
    fn single_field_of_type(&self, name: &str, ty: &Type) -> Option<String> {
        let decl = self.lookup_struct(name)?;
        if decl.fields.len() == 1 && &decl.fields[0].ty == ty {
            Some(decl.fields[0].name.clone())
        } else {
            None
        }
    }

    fn single_field_wrapper_type(&self, ty: &Type) -> Option<Type> {
        let name = self.receiver_struct_name(ty)?;
        let decl = self.lookup_struct(&name)?;
        if decl.fields.len() == 1 {
            Some(decl.fields[0].ty.clone())
        } else {
            None
        }
    }

    /// Given a value type, find the struct name to dispatch methods on.
    /// References auto-deref to their referent struct.
    fn receiver_struct_name(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::I64 => Some("i64".to_string()),
            Type::F64 => Some("f64".to_string()),
            Type::Named(n) => Some(n.clone()),
            Type::Qualified(_, n) => Some(n.clone()),
            Type::Ref(inner) | Type::MutRef(inner) => self.receiver_struct_name(inner),
            _ => None,
        }
    }

    fn check_call(&mut self, expr: &mut Expr, scopes: &mut Scopes, ctx: &mut FnCtx) -> Type {
        let span = expr.span;
        // Split the call into callee + args.
        let (mut callee, mut args) = match std::mem::replace(&mut expr.kind, ExprKind::IntLit(0)) {
            ExprKind::Call { callee, args } => (callee, args),
            _ => unreachable!(),
        };

        // Method call: callee is a FieldExpr (receiver.method).
        if let ExprKind::Field { object, field } = &mut callee.kind {
            // First check the receiver.
            let recv_ty = self.check_expr(object, scopes, ctx, None);
            // Distinguish field-access-then-call vs method call: if the struct
            // has a method named `field`, it's a method call.
            if let Some(struct_name) = self.receiver_struct_name(&recv_ty) {
                if let Some(method) = self.find_method(&struct_name, field).cloned() {
                    // Check arguments against the method's named params.
                    let named_params: Vec<Type> = method
                        .params
                        .iter()
                        .filter_map(|p| match &p.kind {
                            ParamKind::Named { ty, .. } => Some(ty.clone()),
                            _ => None,
                        })
                        .collect();
                    self.check_args(&mut args, &named_params, span, scopes, ctx, &method.name);
                    callee.ty = Some(Type::Void);
                    expr.kind = ExprKind::Call { callee, args };
                    return method.return_type.clone();
                }
            }
            // Not a method — fall through; field-as-callable isn't supported.
            self.error(
                span,
                "E490",
                format!("`{}` is not a method of `{}`", field, recv_ty.display()),
                "no such method",
            );
            // still annotate children
            callee.ty = Some(Type::Void);
            expr.kind = ExprKind::Call { callee, args };
            return Type::Void;
        }

        // Associated (static) method call: `StructName::method(args)` where the
        // method has no `self` parameter. The qualifier names a struct, not a
        // module.
        if let ExprKind::QualifiedIdent { module, name } = &callee.kind {
            if let Some(method) = self.find_method(module, name).cloned() {
                if !method_has_self(&method) {
                    let named_params: Vec<Type> = method
                        .params
                        .iter()
                        .filter_map(|p| match &p.kind {
                            ParamKind::Named { ty, .. } => Some(ty.clone()),
                            _ => None,
                        })
                        .collect();
                    let mname = format!("{}::{}", module, name);
                    self.check_args(&mut args, &named_params, span, scopes, ctx, &mname);
                    callee.ty = Some(Type::Void);
                    expr.kind = ExprKind::Call { callee, args };
                    return method.return_type.clone();
                }
            }
        }

        // Free function call: callee is Ident or QualifiedIdent.
        let (fn_decl, _is_qualified) = match &callee.kind {
            ExprKind::Ident(name) => (self.functions.get(name).cloned(), false),
            ExprKind::QualifiedIdent { module, name } => (
                self.modules
                    .get(module)
                    .and_then(|m| m.functions.get(name))
                    .cloned(),
                true,
            ),
            _ => {
                self.error(span, "E491", "expression is not callable", "not callable");
                expr.kind = ExprKind::Call { callee, args };
                return Type::Void;
            }
        };

        let fn_decl = match fn_decl {
            Some(f) => f,
            None => {
                let name = callee_name(&callee);
                self.error(
                    span,
                    "E492",
                    format!("undeclared function `{}`", name),
                    "not found",
                );
                // still check args to surface their errors
                for a in &mut args {
                    self.check_expr(a, scopes, ctx, None);
                }
                expr.kind = ExprKind::Call { callee, args };
                return Type::Void;
            }
        };

        let param_types: Vec<Type> = fn_decl
            .params
            .iter()
            .filter_map(|p| match &p.kind {
                ParamKind::Named { ty, .. } => Some(ty.clone()),
                _ => None,
            })
            .collect();
        self.check_args(&mut args, &param_types, span, scopes, ctx, &fn_decl.name);

        // annotate callee
        callee.ty = Some(Type::Void);
        expr.kind = ExprKind::Call { callee, args };
        fn_decl.return_type.clone()
    }

    fn check_args(
        &mut self,
        args: &mut [Expr],
        param_types: &[Type],
        span: Span,
        scopes: &mut Scopes,
        ctx: &mut FnCtx,
        fn_name: &str,
    ) {
        if args.len() != param_types.len() {
            self.error(
                span,
                "E493",
                format!(
                    "wrong number of arguments to `{}`: expected {}, found {}",
                    fn_name,
                    param_types.len(),
                    args.len()
                ),
                "argument count mismatch",
            );
        }
        for (i, arg) in args.iter_mut().enumerate() {
            let hint = param_types.get(i);
            let at = self.check_expr(arg, scopes, ctx, hint);
            if let Some(pt) = param_types.get(i) {
                if !matches!(at, Type::Void) && !self.types_compatible(pt, &at) {
                    self.error(
                        arg.span,
                        "E494",
                        format!(
                            "argument {} to `{}`: expected `{}`, found `{}`",
                            i + 1,
                            fn_name,
                            pt.display(),
                            at.display()
                        ),
                        "type mismatch",
                    );
                }
            }
        }
    }

    /// Annotate (and validate) a StructInit expression; returns its type.
    fn annotate_struct_init(
        &mut self,
        expr: &mut Expr,
        scopes: &mut Scopes,
        ctx: &mut FnCtx,
    ) -> Type {
        let span = expr.span;
        let (name, fields) = match &mut expr.kind {
            ExprKind::StructInit { name, fields } => (name.clone(), fields),
            _ => unreachable!(),
        };

        // Resolve the struct declaration.
        let decl = match &name {
            StructName::Unqualified(n) => self.lookup_struct(n).cloned(),
            StructName::Qualified(m, n) => self
                .lookup_struct_qualified(m, n)
                .cloned()
                .or_else(|| self.lookup_struct(n).cloned()),
        };
        let decl = match decl {
            Some(d) => d,
            None => {
                self.error(
                    span,
                    "E495",
                    format!("unknown struct `{}`", name.base()),
                    "unknown struct",
                );
                // still check field exprs
                for (_, fe) in fields.iter_mut() {
                    self.check_expr(fe, scopes, ctx, None);
                }
                return Type::Named(name.base().to_string());
            }
        };

        // Build a field name → declared type map and order.
        let mut declared: HashMap<String, Type> = HashMap::new();
        for f in &decl.fields {
            declared.insert(f.name.clone(), f.ty.clone());
        }

        // Check each provided field.
        let mut seen: HashMap<String, bool> = HashMap::new();
        for (fname, fexpr) in fields.iter_mut() {
            match declared.get(fname) {
                Some(fty) => {
                    let ft = self.check_expr(fexpr, scopes, ctx, Some(fty));
                    if !matches!(ft, Type::Void) && !self.types_compatible(fty, &ft) {
                        self.error(
                            fexpr.span,
                            "E496",
                            format!(
                                "field `{}` of `{}`: expected `{}`, found `{}`",
                                fname,
                                decl.name,
                                fty.display(),
                                ft.display()
                            ),
                            "type mismatch",
                        );
                    }
                    if seen.insert(fname.clone(), true).is_some() {
                        self.error(
                            fexpr.span,
                            "E497",
                            format!("duplicate field `{}` in struct initializer", fname),
                            "duplicate field",
                        );
                    }
                }
                None => {
                    self.check_expr(fexpr, scopes, ctx, None);
                    self.error(
                        fexpr.span,
                        "E498",
                        format!("`{}` has no field `{}`", decl.name, fname),
                        "no such field",
                    );
                }
            }
        }
        // Check for missing fields.
        for f in &decl.fields {
            if !seen.contains_key(&f.name) {
                self.error(
                    span,
                    "E499",
                    format!(
                        "missing field `{}` in initializer for `{}`",
                        f.name, decl.name
                    ),
                    "missing field",
                );
            }
        }

        Type::Named(decl.name.clone())
    }

    /// Resolve `e.field`'s type, auto-dereferencing references.
    fn field_type(&mut self, obj_ty: &Type, field: &str, span: Span) -> Option<Type> {
        let struct_name = match self.receiver_struct_name(obj_ty) {
            Some(n) => n,
            None => {
                self.error(
                    span,
                    "E4A0",
                    format!("type `{}` has no fields", obj_ty.display()),
                    "not a struct",
                );
                return None;
            }
        };
        match self.lookup_struct(&struct_name) {
            Some(decl) => match decl.fields.iter().find(|f| f.name == field) {
                Some(f) => Some(f.ty.clone()),
                None => {
                    self.error(
                        span,
                        "E4A1",
                        format!("`{}` has no field `{}`", struct_name, field),
                        "no such field",
                    );
                    None
                }
            },
            None => {
                self.error(
                    span,
                    "E4A2",
                    format!("unknown struct `{}`", struct_name),
                    "unknown struct",
                );
                None
            }
        }
    }

    /// Whether the `if`/`while` condition type is the stdlib `bool` struct.
    fn expect_bool(&mut self, ty: &Type, span: Span) {
        let ok = matches!(ty, Type::Named(n) if n == "bool");
        if !ok {
            self.error(
                span,
                "E4B0",
                format!("condition must be of type `bool`, found `{}`", ty.display()),
                "expected bool",
            );
        }
    }

    /// Structural type compatibility (no implicit coercions).
    fn types_compatible(&self, expected: &Type, found: &Type) -> bool {
        if expected == found {
            return true;
        }
        match (expected, found) {
            (Type::I64 | Type::F64, Type::Named(_) | Type::Qualified(_, _)) => {
                self.single_field_wrapper_type(found).as_ref() == Some(expected)
            }
            (Type::Ref(a), Type::Ref(b)) => self.types_compatible(a, b),
            (Type::MutRef(a), Type::MutRef(b)) => self.types_compatible(a, b),
            // Allow a &mut T to satisfy a &T expectation (mut is stronger).
            (Type::Ref(a), Type::MutRef(b)) => self.types_compatible(a, b),
            // Named vs Qualified with same base name.
            (Type::Named(a), Type::Qualified(_, b)) | (Type::Qualified(_, b), Type::Named(a)) => {
                a == b
            }
            _ => false,
        }
    }

    fn error(&mut self, span: Span, code: &str, msg: impl Into<String>, label: impl Into<String>) {
        let src_line = crate::error::source_line_of(&self.src, span.line);
        self.diags.push(
            Diagnostic::new(code, msg, self.file.clone(), span.line, span.col, src_line)
                .with_label(label),
        );
    }

    fn duplicate_name_error(&mut self, span: Span, name: &str, kind: &str, first: Span) {
        self.error(
            span,
            "E412",
            format!(
                "duplicate {} `{}` (first declared at line {})",
                kind, name, first.line
            ),
            "duplicate declaration",
        );
    }
}

// ---------------------------------------------------------------------------
// Function-checking context
// ---------------------------------------------------------------------------

struct FnCtx {
    return_type: Type,
    suppress_desugar: bool,
    loop_depth: usize,
    #[allow(dead_code)]
    enclosing_struct: Option<String>,
}

// ---------------------------------------------------------------------------
// Exhaustive return analysis
// ---------------------------------------------------------------------------

fn block_definitely_returns(block: &Block) -> bool {
    match block.stmts.last() {
        Some(stmt) => stmt_definitely_returns(stmt),
        None => false,
    }
}

fn stmt_definitely_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(r) => r.value.is_some(),
        Stmt::If(if_stmt) => {
            // Must have an else, and every branch + else definitely returns.
            match &if_stmt.else_block {
                Some(eb) => {
                    if_stmt
                        .branches
                        .iter()
                        .all(|(_, b)| block_definitely_returns(b))
                        && block_definitely_returns(eb)
                }
                None => false,
            }
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Small AST builders
// ---------------------------------------------------------------------------

fn dummy_expr(span: Span) -> Expr {
    Expr::new(ExprKind::IntLit(0), span)
}

fn make_struct_init_int(struct_name: &str, field: &str, inner: ExprKind, span: Span) -> ExprKind {
    ExprKind::StructInit {
        name: StructName::Unqualified(struct_name.to_string()),
        fields: vec![(field.to_string(), Expr::new(inner, span))],
    }
}

fn make_struct_init_float(struct_name: &str, field: &str, value: f64, span: Span) -> ExprKind {
    ExprKind::StructInit {
        name: StructName::Unqualified(struct_name.to_string()),
        fields: vec![(
            field.to_string(),
            Expr::new(ExprKind::FloatLit(value), span),
        )],
    }
}

fn method_has_self(f: &FnDecl) -> bool {
    f.params.iter().any(|p| {
        matches!(
            p.kind,
            ParamKind::SelfRef | ParamKind::SelfMutRef | ParamKind::SelfMove
        )
    })
}

fn is_primitive_extend_target(name: &str) -> bool {
    matches!(name, "i64" | "f64")
}

fn self_type(name: &str) -> Type {
    match name {
        "i64" => Type::I64,
        "f64" => Type::F64,
        other => Type::Named(other.to_string()),
    }
}

fn callee_name(callee: &Expr) -> String {
    match &callee.kind {
        ExprKind::Ident(n) => n.clone(),
        ExprKind::QualifiedIdent { module, name } => format!("{}::{}", module, name),
        _ => "<expr>".to_string(),
    }
}
