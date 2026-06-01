# Implementation Prompt — Jo Compiler

You are implementing the Jo compiler in Rust. The full language specification is in `syntax.md` and `specifications.md`. The standard library source is in `stdlib.jo`. Read all three files before writing any code.

## What to build

A compiler invoked as:

```
cargo run -- <source.jo>
```

That runs the full pipeline and produces an ELF executable (same name as the source file, no extension) in the current directory.

## Pipeline — implement in this order

### 1. `src/error.rs`
Define `Diagnostic` and the error printing infrastructure first. Every stage depends on it.

```rust
struct Diagnostic {
    code:    String,   // e.g. "E201"
    message: String,
    file:    String,
    line:    usize,
    col:     usize,
    source_line: String,
}
```

Print format:
```
error[E201]: expected return type after ')'
  --> src/main.jo:5:12
   |
 5 | fn foo(x: int) {
   |               ^ expected return type
```

### 2. `src/ast.rs`
Define every AST node exactly as listed in the **AST Node Reference** section of `syntax.md`. Use Rust enums and structs. Every `Expr` variant must carry a `ty: Option<Type>` field that starts as `None` and is filled in by the type checker.

Key types:
- `Program`, `Item`, `FnDecl`, `StructDecl`, `ImportDecl`, `ImportKind`
- `Stmt`, `Expr`, `LValue`, `Block`
- `Type` enum: `I64`, `F64`, `Void`, `Null`, `Named(String)`, `Qualified(String, String)`, `Ref(Box<Type>)`, `MutRef(Box<Type>)`
- `Param`, `ParamKind`, `FieldDecl`
- `UnaryOp`, `BinaryOp`
- `AsmLine`, `AsmToken`

### 3. `src/lexer.rs`
Implement `Lexer` with a `tokenize(src: &str) -> Result<Vec<Token>, Vec<Diagnostic>>` method.

Token kinds are listed in the **Token kinds** section of `specifications.md`. Key rules:
- `::` is a single `ColonColon` token (maximal munch).
- `->` is a single `Arrow` token.
- `true`/`false` → `BoolLit(bool)`.
- Char literal: `'x'` → `CharLit(u32)` (Unicode codepoint).
- Float: `digit* '.' digit*` (at least one digit somewhere).
- `int`, `float`, `char`, `bool`, `string` lex as plain `Ident` — not keywords.
- `null` → `Null` keyword token.

### 4. `src/parser.rs`
Implement a hand-written recursive descent parser. `parse(tokens: Vec<Token>) -> Result<Program, Vec<Diagnostic>>`.

Grammar is in the **Grammar (EBNF)** section of `syntax.md`. Disambiguation rules:

- Return type is **required** after `)`. No `?`. Error if missing.
- `#define` bodies are raw text blobs — these are handled by the preprocessor before lexing. The parser never sees `#define`.
- `struct_init` (`Name { ... }`) vs block (`{ ... }`): parse struct init only when an `Ident` token (or `Ident::Ident` qualified name) is followed immediately by `{` **and** the parser is not in condition position (immediately after `if` or `while`). In condition position `{` always starts a block.
- Struct init fields use `=` not `:`: `Person { name = "x", age = 1 }`. Fields may appear in any order.
- `IDENT::IDENT` followed by `{` is a qualified struct init, not a qualified expression.
- Statement parsing: parse an expression, then peek. If next token is `=`, reparse the expression as an `LValue` and emit `AssignStmt`. Otherwise emit `ExprStmt`.
- `!asm { ... }` is only valid inside a function body. Emit a parse error if encountered at top level.
- `self_param` forms: `&self`, `&mut self`, `move self`. Bare `self` alone is not valid.
- Cast `->` is left-associative and may chain: `a -> B -> C` = `(a -> B) -> C`. Parse as `*` repetition not `?`.

### 5. `src/preprocessor.rs`
Implement `Preprocessor::run(src: &str, include_path: &Path) -> Result<String, Vec<Diagnostic>>`.

Three import forms (see **Import** section in `syntax.md`):
- `import module::name;` — find `name`'s definition in `module.jo`, paste its text.
- `import module::name as alias;` — same, but rename the identifier.
- `import module::*;` — paste all top-level definitions.
- `import module;` — record the module as loaded; emit the import line unchanged for the type checker to resolve `module::name` references.

`#define NAME: value#` — text substitution, value is everything between `:` and `#`.
`#define NAME … #` — macro substitution, body is the text between the name line and closing `#`.

### 6. `src/typechecker.rs`
Implement `TypeChecker::check(program: &mut Program) -> Result<(), Vec<Diagnostic>>`.

Rules are in the **Type Checker** section of `specifications.md`. Key points:

**Literal desugaring:**
- Inside a function whose struct has machine-type (`i64`/`f64`/`null`) fields: suppress desugaring. Raw `IntLit`/`FloatLit` stay as-is with types `i64`/`f64`.
- Everywhere else: `IntLit(n)` → `StructInit("int", [("inner_value", IntLit(n))])`, etc.
- Exception: `let x: i64 = 10;` — explicit `i64` annotation suppresses desugaring for that literal.

**Operator desugaring** — rewrite before type-checking:
- `a + b` → `a.add(b_ref)` where `b_ref` is: `&b` if b has type `T`, `b` if type `&T`, `*b` if type `&&T`.
- Full table in `syntax.md` **Operator Desugaring** section.
- Reference ops (`&`, `&mut`, `*`) are NOT desugared.

**Asm operands:** `%name` and `&name` only accept plain variable names in scope. Field access (`%x.field`) is not valid — the stdlib always assigns fields to locals first.

**Conditions:** `if`/`while` condition must have type `bool` (the stdlib struct `Named("bool")`). Error if not.

**Return rules:**
- Non-void function: `return expr;` required. Bare `return;` is a type error.
- `void` function: `return;` only. `return expr;` is a type error. Falling off the end is fine.
- **Exhaustive return checking** on every non-void function: a block "definitely returns" if its last statement is `return expr;`, or if it is an `if`/`else` where **both** branches definitely return. An `if` without `else` never counts. Missing return path → `error[E4xx]: not all paths return a value`.

**`break`/`continue`:** error if used outside a `while` loop body.

**`main` special cases:**
- `fn main() i64` — valid, raw exit code.
- `fn main() int` — valid, compiler extracts `inner_value`.
- `fn main() void` — valid, exit code 0.
- Any other return type for `main` → type error.

**Scoping and shadowing:**
- Variables only exist inside functions. No global variable scope.
- Any name may be shadowed by a `let` in an inner scope, including parameters.

### 7. `src/codegen.rs`
Implement `CodeGen::emit(program: &Program) -> String` (returns NASM assembly text).

ABI and codegen rules are in the **Code Generator** section of `specifications.md`.

Key points:
- Slot sizes: machine type = 8 bytes; struct with N fields = N×8 bytes; reference = 8 bytes. Frame total = sum of all slots, rounded up to multiple of 16.
- `StructInit` → sort fields into declaration order; allocate N×8 bytes (already in frame); emit each field expr and store at `[rbp - base + index*8]`; result is `lea rax, [rbp - base]`.
- `Ident(x)` for a struct-type variable → `lea rax, [rbp - offset(x)]` (pointer to struct, not a load).
- `FieldExpr(e, name)` → emit `e` → `rax` (pointer); `mov rax, [rax + index*8]` (or `movsd xmm0` for f64 fields).
- `bool` condition for `if`/`while`: emit condition expr (result pointer in `rax`); `mov rax, [rax]` to load `inner_value`; `cmp rax, 0`; `je`.
- Method call `e.method(args)`: pass receiver as first arg per self-convention (`lea` for `&self`/`&mut self`; value for `move self`); then remaining args in ABI order; `call StructName_methodName`.
- `CastExpr` is already a `CallExpr` after type-checking — no special codegen needed.
- `main` returning `int`: load `inner_value` field into `rdi`; use syscall 60.
- `main` returning `void`: `mov rdi, 0`; syscall 60.
- `AsmStmt`: emit each `AsmToken` — `Raw(s)` verbatim, `Value(name)` as `[rbp - offset(name)]`, `Addr(name)` as the literal text `[rbp - N]`.
- Labels: monotone counter `_L0`, `_L1`, …

### 8. `src/main.rs`
Wire everything together:
```
args → preprocessor → lexer → parser → typechecker → codegen → write .asm → nasm → ld → done
```
Stop and print all errors if any stage produces diagnostics. Run NASM and ld via `std::process::Command`.

## Standard library

`stdlib.jo` is a regular Jo source file. It is preprocessed and compiled like any other file. When a user file does `import stdlib::int;`, the preprocessor pastes the `int` struct definition from `stdlib.jo` verbatim. The compiler has no hardcoded knowledge of stdlib types; it learns them only by parsing the pasted source.

## What NOT to do

- Do not add features not in `syntax.md`: no for-loops, no arrays, no closures, no generics.
- Do not special-case `int`, `bool`, etc. in the compiler — treat them as ordinary struct names.
- Do not add implicit coercions between types.
- Do not skip the `void` return type — it is always required explicitly.
- Do not allow top-level `!asm` blocks — asm is only valid inside a function body.

## File layout

```
src/
  main.rs
  preprocessor.rs
  lexer.rs
  parser.rs
  ast.rs
  types.rs          (Type enum — can live in ast.rs if small)
  typechecker.rs
  codegen.rs
  error.rs
stdlib.jo
```
