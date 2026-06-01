# Jo Language — Formal Syntax Reference

## Overview

Jo is a small, statically typed systems language that compiles to x86-64 Linux assembly via NASM and ld.
It is intentionally minimal. Features are added explicitly; nothing is implied.

---

## Source Files

- Extension: `.jo`
- Encoding: UTF-8
- Entry point: a top-level function named `main` with signature `fn main() i64` or `fn main() int` or `fn main() void`.
  - If `i64`: the raw value is passed directly to the OS as the exit code.
  - If `int`: the compiler extracts `inner_value` and passes it to the OS.
  - If `void`: the OS exit code is 0.

---

## Compilation Pipeline

```
source.jo
  → Preprocessor   (expand imports, replace #define constants/macros)
  → Lexer          (tokenize preprocessed source)
  → Parser         (build AST)
  → Type Checker   (infer and verify types)
  → Code Generator (emit x86-64 NASM assembly)
  → NASM           (assemble to object file)
  → ld             (link to ELF executable)
```

---

## Tokens

### Keywords

```
fn        struct      let         return
if        else        while       break
continue  import      as          move
```

### Machine Type Keywords

```
i64   f64   void   null
```

These are the only compiler-primitive types. `int`, `float`, `char`, `bool`, `string` are stdlib structs, not keywords.

`null` is a primitive constant of type `null` (a zero-sized type). It represents the absence of a value and is the only valid value of type `null`. It is used as the raw null pointer value inside stdlib and asm contexts. User code should not use `null` directly; use an `Option`-style stdlib wrapper instead (when added).

### Punctuation

```
(   )   {   }   ,   ;   :   ::   .   ->   =   !   '   "   *
```

Note: `::` is a single token (module path separator / glob when followed by `*`).
`::*` is the two-token sequence `ColonColon` `Star` — parsed together only in import position.

### Operator Tokens

```
+    -    *    /    %        (arithmetic)
==   !=   <    >    <=   >=  (comparison)
&&   ||                      (logical)
&                            (address-of / reference)
*                            (dereference, also multiply — context-disambiguated)
->                           (type cast)
```

### Preprocessor Tokens

```
#define    #    (opening keyword and closing delimiter)
%x         (ASM value operand — x must be a plain variable name, not a field access)
&x         (ASM address operand — x must be a plain variable name, not a field access)
```

### Literals

| Kind    | Form                                        | Examples              |
|---------|---------------------------------------------|-----------------------|
| Integer | decimal digits                              | `0`, `42`, `1000`     |
| Float   | `digit* '.' digit*`, at least one side      | `0.5`, `.3`, `12.`    |
| Char    | `'` single UTF-8 character `'`              | `'a'`, `'Z'`, `'0'`   |
| Bool    | `true` \| `false`                           | `true`, `false`       |
| String  | `"` … `"` (no escape sequences in v1)       | `"hello"`             |

### Identifiers

An identifier starts with a letter or `_`, followed by any number of letters, digits, or `_`.
Identifiers are case-sensitive.
Keywords are reserved and cannot be used as identifiers.

### Comments

```
// single-line comment to end of line
```

Block comments are not supported in v1.

---

## Preprocessor

The preprocessor runs before lexing and operates on raw text.

### Import

Three forms are supported:

```
import stdlib::print;          // named import
import stdlib::print as p;     // named import with alias
import stdlib::*;              // glob import
import stdlib;                 // module import
```

**Named import** (`import module::name;`)
- The preprocessor finds the definition of `name` in `module.jo` and pastes its full source text in place of the import line.
- The name is now available in the current file directly as `name`.
- `as alias` renames the pasted definition; every use of the name in the current file must use `alias`.

**Glob import** (`import module::*;`)
- Every top-level definition in `module.jo` is pasted into the current file.
- All names become available directly.

**Module import** (`import module;`)
- Nothing is pasted. The module file is recorded as available.
- Names from the module must be accessed as `module::name` at every use site.
- The compiler resolves `module::name` references by looking up `name` in `module.jo` at type-check time.
- Example: `import stdlib;` then `let x: stdlib::int = stdlib::int { inner_value = 10 };`

### Constant Define

```
#define NAME: value#
```

- `NAME` must be an identifier.
- `value` is everything between `:` and the closing `#`, trimmed of whitespace.
- Every occurrence of `NAME` in the source after this point is replaced with `value` textually.

### Macro Define

```
#define NAME
  statement;
  statement;
#
```

- `NAME` must be an identifier.
- The body is every line between the identifier line and the closing `#`.
- Invoking `NAME` as a statement pastes the body inline.

---

## Grammar (EBNF)

```
program         = item* EOF

item            = import_decl
                | struct_decl
                | fn_decl
                (* define_const and define_macro are preprocessor-only; never in the parsed AST *)

import_decl     = "import" import_path ";"
import_path     = IDENT "::" IDENT ("as" IDENT)?   (* named import, optional alias *)
                | IDENT "::" "*"                   (* glob import *)
                | IDENT                            (* module import *)

(* #define is handled entirely by the preprocessor before lexing.          *)
(* The parser never sees #define tokens — they do not appear in the AST.   *)
(* Both forms are raw text:                                                 *)
(*   constant:  #define NAME: value#    — value is a text blob              *)
(*   macro:     #define NAME … #        — body is a text blob               *)

struct_decl     = "struct" IDENT "{" struct_member* "}"
struct_member   = field_decl | method_decl
field_decl      = IDENT ":" type ","
method_decl     = fn_decl

fn_decl         = "fn" IDENT "(" param_list? ")" return_type block
param_list      = param ("," param)*
param           = self_param | named_param
self_param      = "&" "mut" "self"
                | "&" "self"
                | "move" "self"
named_param     = IDENT ":" type
return_type     = type          (* required; use "void" explicitly for no return value *)

block           = "{" statement* "}"

statement       = let_stmt
                | assign_stmt
                | return_stmt
                | if_stmt
                | while_stmt
                | break_stmt
                | continue_stmt
                | expr_stmt
                | asm_block

let_stmt        = "let" IDENT (":" type)? "=" expr ";"
assign_stmt     = lvalue "=" expr ";"
return_stmt     = "return" expr ";"    (* non-void functions *)
                | "return" ";"         (* void functions only; error in non-void context *)
if_stmt         = "if" expr block ("else" "if" expr block)* ("else" block)?
while_stmt      = "while" expr block
break_stmt      = "break" ";"
continue_stmt   = "continue" ";"
expr_stmt       = expr ";"

lvalue          = IDENT
                | "*" expr
                | expr "." IDENT

asm_block       = "!" "asm" "{" asm_line* "}"
asm_line        = (asm_token)* NEWLINE
asm_token       = "%" IDENT          (* current value of plain variable — field access not allowed *)
                | "&" IDENT          (* address of plain variable — field access not allowed *)
                | any non-newline text

expr            = cast_expr
cast_expr       = logical_expr ("->" type)*    (* zero or more casts, left-associative *)
logical_expr    = comparison_expr (("&&" | "||") comparison_expr)*
comparison_expr = additive_expr (("==" | "!=" | "<" | ">" | "<=" | ">=") additive_expr)*
additive_expr   = multiplicative_expr (("+" | "-") multiplicative_expr)*
multiplicative_expr = unary_expr (("*" | "/" | "%") unary_expr)*
unary_expr      = ("!" | "-" | "*" | "&" | "&" "mut")? postfix_expr
postfix_expr    = primary_expr (call_suffix | field_suffix)*
call_suffix     = "(" arg_list? ")"
field_suffix    = "." IDENT
arg_list        = expr ("," expr)*

primary_expr    = IDENT
                | INT_LIT
                | FLOAT_LIT
                | CHAR_LIT
                | BOOL_LIT
                | STRING_LIT
                | qualified_expr         (* module::name *)
                | struct_init
                | "(" expr ")"

qualified_expr  = IDENT "::" IDENT       (* e.g. stdlib::print — resolves to a function or value *)
                                         (* if followed by "{", parsed as qualified struct_init instead *)

struct_init          = struct_name "{" field_init_list? "}"
struct_name          = IDENT                   (* unqualified: Person { ... } *)
                     | IDENT "::" IDENT        (* qualified:   stdlib::int { ... } *)
field_init_list      = field_init ("," field_init)* ","?
field_init           = IDENT "=" expr          (* field = value, order does not matter *)
```

---

## Type System

### Types

```
type = machine_type
     | ref_type
     | mut_ref_type
     | qualified_type    (* module::name *)
     | IDENT             (* struct type in scope *)

machine_type  = "i64" | "f64" | "void" | "null"
ref_type      = "&" type
mut_ref_type  = "&" "mut" type
qualified_type = IDENT "::" IDENT
```

### Machine types

`i64`, `f64`, and `void` are the only compiler-primitive types.

| Type   | Description                                                              |
|--------|--------------------------------------------------------------------------|
| `i64`  | 64-bit signed integer — raw machine value                                |
| `f64`  | 64-bit IEEE 754 float — raw machine value                                |
| `void` | no value; only valid as a function return type                           |
| `null` | the null pointer / absent value; only valid as a field type or raw ptr   |

Machine types exist solely as storage for stdlib structs and for inline assembly.
**User code should never use `i64`, `f64`, or `null` directly** — use `int`, `float`, `char`, `bool`, `string` from the stdlib instead.

---

## Stdlib Types

The standard library (`stdlib.jo`) defines all user-facing value types as structs.
They are not built into the compiler; they are ordinary structs that happen to live in stdlib.

### `int`

```
struct int {
    inner_value: i64,

    fn new(v: i64) int {
        return int { inner_value = v };
    }

    fn add(&self, other: &int) int { ... }
    fn sub(&self, other: &int) int { ... }
    fn mul(&self, other: &int) int { ... }
    fn div(&self, other: &int) int { ... }
    fn mod_(&self, other: &int) int { ... }
    fn eq(&self, other: &int)  bool { ... }
    fn ne(&self, other: &int)  bool { ... }
    fn lt(&self, other: &int)  bool { ... }
    fn gt(&self, other: &int)  bool { ... }
    fn le(&self, other: &int)  bool { ... }
    fn ge(&self, other: &int)  bool { ... }
    fn neg(&self)              int  { ... }
    fn not(&self)              bool { ... }
}
```

### `float`

```
struct float {
    inner_value: f64,

    fn new(v: f64) float { ... }
    fn add(&self, other: &float) float { ... }
    fn sub(&self, other: &float) float { ... }
    fn mul(&self, other: &float) float { ... }
    fn div(&self, other: &float) float { ... }
    fn eq(&self, other: &float)  bool  { ... }
    fn ne(&self, other: &float)  bool  { ... }
    fn lt(&self, other: &float)  bool  { ... }
    fn gt(&self, other: &float)  bool  { ... }
    fn le(&self, other: &float)  bool  { ... }
    fn ge(&self, other: &float)  bool  { ... }
    fn neg(&self)                float { ... }
    fn to_int(&self)             int   { ... }
}
```

### `char`

```
struct char {
    inner_value: i64,    // Unicode codepoint stored as i64

    fn new(v: i64) char { ... }
    fn eq(&self, other: &char) bool { ... }
    fn ne(&self, other: &char) bool { ... }
    fn to_int(&self) int { ... }
}
```

### `bool`

```
struct bool {
    inner_value: i64,    // 0 = false, 1 = true

    fn new(v: i64) bool { ... }
    fn not(&self)              bool { ... }
    fn and(&self, other: &bool) bool { ... }
    fn or(&self,  other: &bool) bool { ... }
    fn eq(&self,  other: &bool) bool { ... }
    fn ne(&self,  other: &bool) bool { ... }
}
```

### `string`

```
struct string {
    ptr:    i64,    // address of UTF-8 bytes in .rodata (static lifetime in v1)
    length: i64,    // byte count, not including null terminator

    fn new(p: i64, len: i64) string { ... }
    fn eq(&self, other: &string) bool { ... }
    fn ne(&self, other: &string) bool { ... }
    fn len(&self) int { ... }
}
```

---

## Operator Desugaring

**All binary and unary operators on stdlib types desugar to method calls.**
The compiler never directly emits arithmetic for stdlib types — it always routes through the method.

| Written syntax     | Desugars to              |
|--------------------|--------------------------|
| `a + b`            | `a.add(&b)`              |
| `a - b`            | `a.sub(&b)`              |
| `a * b`            | `a.mul(&b)`              |
| `a / b`            | `a.div(&b)`              |
| `a % b`            | `a.mod_(&b)`             |
| `a == b`           | `a.eq(&b)`               |
| `a != b`           | `a.ne(&b)`               |
| `a < b`            | `a.lt(&b)`               |
| `a > b`            | `a.gt(&b)`               |
| `a <= b`           | `a.le(&b)`               |
| `a >= b`           | `a.ge(&b)`               |
| `-a`               | `a.neg()`                |
| `!a`               | `a.not()`                |
| `a && b`           | `a.and(&b)`              |
| `a \|\| b`         | `a.or(&b)`               |

Desugaring happens in the type checker after operand types are resolved.
If the type of `a` does not have the required method, it is a type error.

This means operators can be "overloaded" simply by implementing the corresponding method on any struct.

---

## Literal Desugaring

Integer, float, char, bool, and string literals in user code are **automatically wrapped** into their stdlib type by the type checker.

| Literal        | Desugars to                               | Inferred type |
|----------------|-------------------------------------------|---------------|
| `10`           | `int { inner_value = 10 }`                | `int`         |
| `3.14`         | `float { inner_value = 3.14 }`            | `float`       |
| `'a'`          | `char { inner_value = 97 }`               | `char`        |
| `true`         | `bool { inner_value = 1 }`                | `bool`        |
| `false`        | `bool { inner_value = 0 }`                | `bool`        |
| `"hello"`      | `string { ptr = ..., length = 5 }`        | `string`      |

The raw `i64`/`f64` literals still exist at the machine level for use inside stdlib method bodies and inline assembly. Outside of those contexts the compiler warns if a raw machine type appears where a stdlib type is expected.

So:

```
let x = 10;          // x: int  (desugared to int { inner_value: 10 })
let y: int = 10;     // same
let z: i64 = 10;     // z: i64  (raw — only valid in stdlib/asm contexts)
```

---

## Type inference

`let x = expr;` infers `x`'s type from `expr` after desugaring.
`let x: T = expr;` requires the desugared `expr` to have type `T`.
There is no implicit coercion between types. Use explicit cast syntax.

### References

| Syntax       | Meaning                              |
|--------------|--------------------------------------|
| `&T`         | immutable reference to `T`           |
| `&mut T`     | mutable reference to `T`             |
| `&x`         | take immutable reference of `x`      |
| `&mut x`     | take mutable reference of `x`        |
| `*r`         | dereference reference `r`            |

Multiple `&mut` references to the same value are allowed (no exclusive borrow rule).

### Type casting

```
let c = x -> float;
let d = x -> float -> string;    // chained: two casts, left-to-right
```

`x -> T` is valid only if the source type implements a method named `to_T` with signature `fn to_T(&self) T`.
Multiple `->` casts may be chained left-associatively: `a -> B -> C` means `(a -> B) -> C`.
Example built-ins provided by stdlib:

```
int::to_float(&self) float
float::to_int(&self) int
```

---

## Functions

```
fn add(x: int, y: int) int {
    return x + y;
}
```

- Parameters are separated by `,`.
- Return type is **required**. Use `void` explicitly for functions that return no value.
- Return type follows the parameter list directly — no `->` separator (`->` is reserved for casts).
- `return expr;` is required in non-void functions. A bare `return;` in a non-void function is a type error.
- `return;` (no expression) is only valid in `void` functions. `return expr;` in a `void` function is a type error.
- There is no implicit return — the last expression in a block is not automatically returned.
- **Every execution path through a non-void function must end with `return expr;`.** The type checker performs exhaustive return checking:
  - A block "definitely returns" if its last statement is `return expr;`, or if it contains an `if`/`else` where **both** the `if` branch and the `else` branch definitely return.
  - An `if` without an `else` does **not** count as definitely returning, even if the `if` branch has `return expr;` — because the else path falls through.
  - If the function body does not definitely return, it is a **compile error**: `error[E4xx]: not all paths return a value`.

Examples:
```
fn f() int {
    if cond {
        return 1;       // OK: both branches return
    } else {
        return 0;
    }
}

fn g() int {
    if cond {
        return 1;       // ERROR: else path does not return
    }
}

fn h() void {
    if cond {
        return;         // OK: void function; partial return is fine
    }
    // falls off the end — also fine for void
}
```

---

## Structs

```
struct Person {
    name: string,
    age: int,

    fn new() Person {
        return Person { name = "test", age = 10 };
    }

    fn get_age(&self) int {
        return self.age;
    }

    fn set_age(&mut self, value: int) void {
        self.age = value;
    }

    fn take_ownership(move self) Person {
        return self;
    }
}
```

- Fields come first, then methods.
- Each field declaration ends with `,`.
- Methods are full `fn` declarations inside the struct body.
- `self` refers to the current instance.
- Struct literals use `=` not `:` for field assignment: `Person { name = "x", age = 1 }`.
- Fields in a struct literal may appear in **any order**. The type checker matches by name, not position.
- All fields must be present; no field may appear twice. Both are errors.

### Self parameter forms

| Form          | Meaning                                      |
|---------------|----------------------------------------------|
| `&self`       | immutable reference to self                  |
| `&mut self`   | mutable reference to self                    |
| `move self`   | takes ownership; caller cannot use value after |
| *(none)*      | static/associated function — no `self`       |

---

## Control Flow

### If / else if / else

```
if x == 0 {
    // ...
} else if x < 0 {
    // ...
} else {
    // ...
}
```

- Condition must be an expression of type `bool` (the stdlib struct). Raw `i64` is not accepted as a condition.
- Braces are mandatory.
- No ternary operator in v1.

### While

```
while x < 10 {
    x = x + 1;
    if x == 5 { break; }
    continue;
}
```

- `break` exits the innermost `while` loop. Using `break` outside any loop is a compile error.
- `continue` jumps to the next iteration of the innermost `while` loop. Using `continue` outside any loop is a compile error.

---

## Inline Assembly

```
!asm {
    mov rax, %x
    lea rbx, &y
}
```

- `!asm { ... }` may **only** appear inside a function body. Top-level asm is not valid (no global scope).
- Lines inside are raw NASM syntax, except:
  - `%name` is replaced with the current value (register or stack slot) of variable `name`.
  - `&name` is replaced with the address of variable `name` (e.g., `[rbp - N]`).
- `name` must be a **plain variable** in the enclosing function's scope. Field access (`%x.field`) is not valid — assign the field to a local variable first, then use `%localvar`.
- Using an unknown name in `%name` or `&name` is a compile error.
- The compiler does not validate asm instruction content beyond operand substitution.
- Clobber semantics: the compiler assumes asm may clobber any register; callee-saved registers are spilled around asm blocks.

---

## Operators and Precedence

Lower number = binds tighter (higher precedence).

| Level | Operators               | Associativity |
|-------|-------------------------|---------------|
| 1     | unary `-`, `!`, `*`, `&`, `&mut` | right |
| 2     | `*`, `/`, `%`           | left          |
| 3     | `+`, `-`                | left          |
| 4     | `==`, `!=`, `<`, `>`, `<=`, `>=` | left |
| 5     | `&&`                    | left          |
| 6     | `\|\|`                  | left          |
| 7     | `->` (cast)             | left          |

Postfix call `f(...)` and field access `.x` bind tighter than all of the above.

---

## AST Node Reference

Every node below is what the parser produces. The type checker annotates each expression node with a resolved `Type`.

### Top-level nodes

`DefineConst`, `DefineMacro`, and top-level `AsmBlock` are handled entirely by the preprocessor before lexing. They never appear in the AST.

| Node            | Fields                                                   |
|-----------------|----------------------------------------------------------|
| `Program`       | `items: Vec<Item>`                                       |
| `ImportDecl`    | `kind: ImportKind`                                       |
| `StructDecl`    | `name: String`, `fields: Vec<FieldDecl>`, `methods: Vec<FnDecl>` |
| `FnDecl`        | `name: String`, `params: Vec<Param>`, `return_type: Type`, `body: Block` |

### Statement nodes

| Node            | Fields                                                   |
|-----------------|----------------------------------------------------------|
| `LetStmt`       | `name: String`, `ty: Option<Type>`, `init: Expr`         |
| `AssignStmt`    | `target: LValue`, `value: Expr`                          |
| `ReturnStmt`    | `value: Option<Expr>`                                    |
| `IfStmt`        | `branches: Vec<(Expr, Block)>`, `else_block: Option<Block>` |
| `WhileStmt`     | `condition: Expr`, `body: Block`                         |
| `BreakStmt`     | *(no fields)*                                            |
| `ContinueStmt`  | *(no fields)*                                            |
| `ExprStmt`      | `expr: Expr`                                             |
| `AsmStmt`       | `lines: Vec<AsmLine>`                                    |

### Expression nodes

Each expression node carries an implicit `ty: Type` field set by the type checker.

| Node              | Fields                                                 |
|-------------------|--------------------------------------------------------|
| `IntLit`          | `value: i64`                                           |
| `FloatLit`        | `value: f64`                                           |
| `CharLit`         | `codepoint: u32`                                       |
| `BoolLit`         | `value: bool`                                          |
| `StringLit`       | `value: String`                                        |
| `Ident`           | `name: String`                                         |
| `QualifiedIdent`  | `module: String`, `name: String`                       |
| `UnaryExpr`       | `op: UnaryOp`, `operand: Box<Expr>`                    |
| `BinaryExpr`      | `op: BinaryOp`, `left: Box<Expr>`, `right: Box<Expr>`  |
| `CastExpr`        | `expr: Box<Expr>`, `target_type: Type`                 |
| `CallExpr`        | `callee: Box<Expr>`, `args: Vec<Expr>`                 |
| `FieldExpr`       | `object: Box<Expr>`, `field: String`                   |
| `IndexExpr`       | *(reserved for later)*                                 |
| `RefExpr`         | `mutable: bool`, `operand: Box<Expr>`                  |
| `DerefExpr`       | `operand: Box<Expr>`                                   |
| `StructInit`      | `name: StructName`, `fields: Vec<(String, Expr)>`      |

### Type nodes

| Node          | Fields                         |
|---------------|--------------------------------|
| `TyI64`       | *(none)*                         |
| `TyF64`       | *(none)*                         |
| `TyVoid`      | *(none)*                         |
| `TyNull`      | *(none)*                         |
| `TyNamed`     | `name: String`                   |
| `TyQualified` | `module: String`, `name: String` |
| `TyRef`       | `inner: Box<Type>`               |
| `TyMutRef`    | `inner: Box<Type>`               |

### Auxiliary nodes

| Node          | Fields                                              |
|---------------|-----------------------------------------------------|
| `Block`       | `stmts: Vec<Stmt>`                                  |
| `FieldDecl`   | `name: String`, `ty: Type`                          |
| `Param`       | `kind: ParamKind`                                   |
| `ParamKind`   | `Named { name: String, ty: Type }` \| `SelfRef` \| `SelfMutRef` \| `SelfMove` — for self variants, name is always `"self"` and type is resolved to `TyNamed(enclosing_struct)` by the type checker |
| `LValue`      | `Ident(String)` \| `Deref(Box<Expr>)` \| `Field(Box<Expr>, String)` |
| `AsmLine`     | `tokens: Vec<AsmToken>`                             |
| `AsmToken`    | `Raw(String)` \| `Value(String)` \| `Addr(String)` |
| `ImportKind`  | `Named { module: String, name: String, alias: Option<String> }` \| `Glob { module: String }` \| `Module { name: String }` |
| `StructName`  | `Unqualified(String)` \| `Qualified(String, String)`              |

### Operator enums

```
UnaryOp  = Neg | Not | Deref | Ref | RefMut
BinaryOp = Add | Sub | Mul | Div | Mod
         | Eq | Ne | Lt | Gt | Le | Ge
         | And | Or
```

---

## Error Reporting

All errors (lexer, parser, type checker) are reported in the following format, mimicking Rust's style:

```
error[E001]: unexpected token `}`
  --> src/main.jo:12:5
   |
12 |     }
   |     ^ expected expression
```

- Errors include: error code, message, file path, line number, column number, source line, and a caret pointing to the offending position.
- After an error, the compiler attempts to continue (error recovery) to report further errors in the same pass.
- Compilation stops before the next pass if any errors were reported in the previous pass.

---

## Scoping Rules

- **There is no global scope for variables.** Variables only exist inside function bodies.
- Top-level items (function declarations, struct declarations) are in the **top-level namespace**, not a runtime scope. They cannot be shadowed and are always accessible by name from anywhere in the file.
- Each `{` `}` block introduces a new nested scope within a function.
- Variables declared with `let` are scoped to their enclosing block and are not accessible outside it.
- Any name may be shadowed by a `let` in an inner scope — including parameters, and including the function's own parameter names within its body. The inner binding hides the outer one for the remainder of that scope.
- Struct fields and methods are in a separate per-struct namespace; they are accessed only via `.` syntax.
- `import` (named or glob) adds names to the top-level namespace.
- `import module;` (module form) does not add names; they must be accessed as `module::name`.

---

## Undefined Behavior (v1)

To keep the implementation simple, the following are unchecked in v1:

- Dereferencing a null or dangling pointer.
- Integer overflow (wraps on x86-64).
- Out-of-bounds memory access.
- Reading uninitialized memory.

These may be addressed in a future version.
