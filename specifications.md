# Jo Compiler — Implementation Specification

## Invocation

```
jo_compiler <source.jo>
```

Compiles `source.jo` through the full pipeline and produces an executable named after the source file (without extension) in the current directory.

---

## Pipeline Stages

### 1. Preprocessor

**Input:** raw `.jo` source text  
**Output:** preprocessed source text (still text, not tokens)

Steps:
1. Scan for `import` declarations.
   Three forms, each handled differently:

   **Named import** — `import module::name;` or `import module::name as alias;`
   - Find the top-level definition of `name` in `module.jo`.
   - Paste its full source text in place of the import line.
   - If `as alias` is given, rename the pasted definition's identifier to `alias`.
   - `name` (or `alias`) is now available directly in the current file.

   **Glob import** — `import module::*;`
   - Paste the full text of every top-level definition in `module.jo` in place of the import line.
   - All names from that module become available directly.

   **Module import** — `import module;`
   - Do **not** paste anything. Record `module` as a loaded module.
   - Leave the source text as-is; `module::name` references are resolved at type-check time by looking up `name` in the loaded module's AST.

2. Scan for `#define NAME: value#` — build a substitution table `{NAME → value_text}`.
3. Scan for `#define NAME … #` — build a macro table `{NAME → body_text}`.
4. Walk the source text and replace all occurrences of defined names with their substitution text (constants) or body text (macros).

**Errors reported:**
- Module file not found.
- Named item not found in module.
- Unterminated `#define` (missing closing `#`).

---

### 2. Lexer

**Input:** preprocessed source text  
**Output:** `Vec<Token>`

Each `Token` has:
```
Token {
    kind:   TokenKind,
    lexeme: String,
    line:   usize,
    col:    usize,
}
```

#### Token kinds

```
// Literals
IntLit       // decimal integer
FloatLit     // decimal float (contains `.`)
CharLit      // 'x' — single UTF-8 character
BoolLit      // `true` | `false`  (also emitted as True/False keywords — same token)
StringLit    // "..."

// Identifiers and keywords
Ident        // any identifier not matching a keyword
Fn           // fn
Struct       // struct
Let          // let
Return       // return
If           // if
Else         // else
While        // while
Break        // break
Continue     // continue
Import       // import
As           // as
Move         // move
Self_        // self
I64          // i64
F64          // f64
Void         // void
Null         // null
True         // true
False        // false
// Note: int, float, char, bool, string are NOT keywords — they are stdlib struct names (Ident tokens)

// Punctuation
LParen       // (
RParen       // )
LBrace       // {
RBrace       // }
Comma        // ,
Semicolon    // ;
Colon        // :
ColonColon   // ::
Dot          // .
Arrow        // ->
Bang         // !
Pound        // #

// Operators
Eq           // =
EqEq         // ==
Ne           // !=
Lt           // <
Gt           // >
Le           // <=
Ge           // >=
Plus         // +
Minus        // -
Star         // *
Slash        // /
Percent      // %
Ampersand    // &
AmpAmp       // &&
PipePipe     // ||
Mut          // mut  (keyword, appears after & in type/expr positions)

// Special
Asm          // asm  (keyword, only valid after !)
Eof          // end of input
```

#### Lexer rules

- Whitespace (spaces, tabs, newlines) is skipped between tokens but tracked for line/col.
- `//` starts a comment; the rest of the line is skipped.
- Integer literal: one or more decimal digits with no following `.`.
- Float literal: `digit* '.' digit*` — at least one digit required somewhere; `12.`, `.3`, `0.5` are all valid.
- Char literal: `'` followed by exactly one UTF-8 character followed by `'`. The lexer stores the Unicode codepoint as `u32`.
- String literal: `"` followed by any characters (except unescaped `"`) followed by `"`. No escape sequences in v1.
- `true` / `false` are emitted as `BoolLit` with the boolean value encoded, **not** as `Ident`.
- `::` is a single `ColonColon` token, not two `Colon` tokens.
- `->` is a single `Arrow` token, not `Minus` then `Gt`.
- `==`, `!=`, `<=`, `>=`, `&&`, `||` are single tokens — maximal munch.
- `mut` is always emitted as the `Mut` keyword token. It is not a standalone type or identifier.
- `int`, `float`, `char`, `bool`, `string` lex as plain `Ident` tokens. They have no special lexer treatment.

**Errors reported:**
- Unterminated string literal (EOF before closing `"`).
- Unterminated char literal (EOF or more than one character before `'`).
- Unexpected character (with line and column).

---

### 3. Parser

**Input:** `Vec<Token>`  
**Output:** `Program` AST (see AST Node Reference in syntax.md)

The parser is a hand-written **recursive descent** parser.

#### Parsing strategy

- One function per grammar rule.
- Current token is peeked without consuming; `advance()` consumes.
- `expect(kind)` consumes and returns the token, or emits an error and attempts recovery.
- Error recovery: on unexpected token, skip tokens until a synchronization point (`;`, `}`, or a keyword that starts a new item/statement).

#### Disambiguation

- `*` in unary position is dereference; in binary position is multiply.
- `&` followed by `mut` is `&mut` (mutable reference); alone is `&` (immutable reference / address-of).
- `->` in expression context (after a complete expression) is a cast; it never appears in function signatures (return type follows `)` directly).
- `!` followed by `asm` is an inline assembly block; in any other position it is logical not.
- `{` after a plain `IDENT` or a `IDENT::IDENT` qualified name in expression position is a struct initializer; after a condition or loop header it is always a block (struct init is never parsed in condition position).
- `IDENT::IDENT` followed by `{` is a qualified struct init (`module::Type { … }`), not a `QualifiedIdent` expression.
- Statement parsing begins by checking the current token for a keyword: `let`, `return`, `if`, `while`, `break`, `continue` each start their respective statement forms unambiguously. `!` followed by `asm` starts an `AsmStmt`. Any other token starts the expression-first path: parse a full expression, then peek — if `=` follows, reparse the expression as an `LValue` and emit `AssignStmt`; otherwise emit `ExprStmt` (consuming the trailing `;`).

**Return type rule:** the return type token is required. If the token after `)` is not a type, it is a parse error: `error[E2xx]: expected return type after ')'`. There is no defaulting; the programmer must write `void` explicitly.

**Errors reported:**
- Expected token X, got Y (with location).
- Missing return type after `)`.
- Struct initializer field missing `=` (note: fields use `=`, not `:`).
- Unclosed brace/paren.

---

### 4. Type Checker

**Input:** `Program` AST  
**Output:** type-annotated `Program` AST (every `Expr` node has a `ty: Type` field filled in)

#### Type checking rules

**Variables:**
- `let x = e;` → infer type of `e`, bind `x` to that type in current scope.
- `let x: T = e;` → check `e` has type `T`; bind `x: T`.
- `x = e;` → look up `x`, check `e` matches, check `x` is mutable (not an immutable ref target).

**Functions:**
- Check each argument type matches the corresponding parameter type.
- Check the return expression type matches the declared return type.
- `void` functions may use bare `return;`.

**Literal desugaring** (runs before operator resolution):

Desugaring is suppressed inside any function whose containing struct has a field of machine type (`i64`/`f64`/`null`) — i.e. inside stdlib method bodies. In those bodies, integer and float literals stay as raw `IntLit`/`FloatLit` with types `i64`/`f64`. This gives stdlib full control over machine-level construction.

Everywhere else:
- `IntLit(n)` → rewrite to `StructInit("int", [("inner_value", IntLit(n))])`, type `int`.
- `FloatLit(f)` → rewrite to `StructInit("float", [("inner_value", FloatLit(f))])`, type `float`.
- `CharLit(cp)` → rewrite to `StructInit("char", [("inner_value", IntLit(cp as i64))])`, type `char`.
- `BoolLit(true)` → `StructInit("bool", [("inner_value", IntLit(1))])`, type `bool`.
- `BoolLit(false)` → `StructInit("bool", [("inner_value", IntLit(0))])`, type `bool`.
- `StringLit(s)` → allocate `s` as a null-terminated byte string in `.rodata` (read-only, static lifetime); rewrite to `StructInit("string", [("ptr", AddressOf(_SN)), ("length", IntLit(byte_len))])`, type `string`. String literals are never heap-allocated in v1. The `ptr` field holds the `.rodata` address as an `i64`. Mutation of the bytes pointed to is undefined behaviour.

Exception: if `let x: i64 = 10;` is written explicitly with type `i64`, the literal is kept raw and no desugaring occurs. Same for `f64`. This allows stdlib bodies to use typed raw literals directly.

**Operator desugaring** (runs after literal desugaring, before type checking):
All binary/unary operators are rewritten into method calls on the left operand's type.

The right-hand operand `b` of a binary operator is always passed as a reference. The rule for how `b` is wrapped:
- If `b` has type `T` (not a reference) → pass `&b` (take immutable reference).
- If `b` has type `&T` → pass `b` as-is (already a reference).
- If `b` has type `&&T` → pass `*b` (strip one level of reference).

| Operator   | Rewrite (after reference normalisation of `b`) |
|------------|------------------------------------------------|
| `a + b`    | `a.add(&b)` / `a.add(b)` / `a.add(*b)`        |
| `a - b`    | `a.sub(…)`                                     |
| `a * b`    | `a.mul(…)`                                     |
| `a / b`    | `a.div(…)`                                     |
| `a % b`    | `a.mod_(…)`                                    |
| `a == b`   | `a.eq(…)`                                      |
| `a != b`   | `a.ne(…)`                                      |
| `a < b`    | `a.lt(…)`                                      |
| `a > b`    | `a.gt(…)`                                      |
| `a <= b`   | `a.le(…)`                                      |
| `a >= b`   | `a.ge(…)`                                      |
| `a && b`   | `a.and(…)`                                     |
| `a \|\| b` | `a.or(…)`                                      |
| `-a`       | `a.neg()`                                      |
| `!a`       | `a.not()`                                      |

The `…` in the table above means "b after reference normalisation" as described above.

After rewriting, the method call is type-checked normally. If the method does not exist on the type, it is a type error:
`error[E4xx]: type 'T' does not implement method 'add'`.

**Casts** desugar to a single method call per `->` step, left-to-right:
- `a -> B -> C` means: call `a.to_B()` → get value of type `B`, then call that `.to_C()`.

Reference operators (`&`, `&mut`, `*`) are **not** desugared — they are handled directly:
- `&e` → type `&T` where `e: T`.
- `&mut e` → type `&mut T` where `e: T`.
- `*e` → `e` must have type `&T` or `&mut T`; result type is `T`.

**Casts:**
- `e -> T` is valid only if the type of `e` has a method `fn to_T(&self) T`.
- Stdlib provides: `int::to_float`, `float::to_int`.

**Struct init:**
- Syntax uses `=` not `:`: `Name { field = expr, ... }`.
- Fields may appear in any order; the type checker matches by name.
- Look up struct `Name` (unqualified) or `module::Name` (qualified). Check each field name exists and each expr matches the declared field type.
- Missing fields are an error. Extra fields are an error. Duplicate fields are an error.

**Field access:**
- `e.field`: look up the type of `e` (or `*e` if `e` is a reference), find `field` in the struct definition.

**Method calls:**
- `e.method(args)` desugars to a call where the first parameter is the receiver.
- `&self` → pass `&e`.
- `&mut self` → pass `&mut e`.
- `move self` → pass `e` (consuming it).

**Conditions:**
- The condition of `if` and `while` must have type `bool` (the stdlib struct). Raw `i64` is rejected.
- The condition expression is checked after literal desugaring and operator desugaring, so `x == 0` desugars to `x.eq(&0)` which returns `bool` — this is valid.

**`main` special-casing:**
- `fn main() i64` — the return value is used as the OS exit code directly (raw `i64`).
- `fn main() int` — the compiler emits extra code to extract `inner_value` from the returned `int` struct and pass it to the OS.
- `fn main() void` — the compiler emits `mov rdi, 0` / `call exit` (exit code 0).
- Any other return type for `main` is a type error.

**Return statement rules:**
- In a non-void function: `return expr;` is required. A bare `return;` is a type error.
- In a `void` function: `return;` is valid. `return expr;` is a type error. Falling off the end of a `void` function is fine.
- **Exhaustive return checking** — performed by the type checker on every non-void function:

  Define "definitely returns" recursively:
  - A `ReturnStmt` with an expression → definitely returns.
  - A `Block` → definitely returns if its last statement definitely returns.
  - An `IfStmt` with both an `if` branch and an `else` branch → definitely returns only if **both** branches definitely return. An `if` without `else` never definitely returns, regardless of its body.
  - `WhileStmt`, `BreakStmt`, `ContinueStmt`, `LetStmt`, `AssignStmt`, `ExprStmt`, `AsmStmt` → do not definitely return.

  If the function body block does not definitely return → compile error: `error[E4xx]: not all paths return a value`.

  Examples that are **valid**:
  ```
  fn f() int {
      if cond { return 1; } else { return 0; }   // both branches return
  }
  fn g() void {
      if cond { return; }    // void: partial return is fine, falls off end
  }
  ```
  Examples that are **errors**:
  ```
  fn h() int {
      if cond { return 1; }  // ERROR: no else — cond-false path returns nothing
  }
  fn k() int {
      let x = 5;             // ERROR: no return statement at all
  }
  ```

**`break` and `continue` rules:**
- `break;` and `continue;` are only valid inside a `while` loop body.
- Using either outside any loop is a compile error: `error[E4xx]: 'break' outside of loop`.
- They target the immediately enclosing `while` loop (no labelled loops in v1).

**Scoping and shadowing:**
- Walk AST, push/pop scopes at each `Block`.
- Variables are only valid inside function bodies — there is no global variable scope.
- Error on use of undeclared variable.
- **Any** name may be shadowed by a `let` in any inner scope, including function parameters. A `let x = …;` inside a function body shadows a parameter also named `x` for the rest of that block. The outer binding is not modified.

**Parameter self-type resolution:**
For `&self`, `&mut self`, and `move self` params, the type checker fills in the type as `TyNamed(enclosing_struct_name)` (wrapped in `TyRef` or `TyMutRef` for `&self`/`&mut self`). The name is always `"self"`. This happens before the body is type-checked.

**Errors reported:**
- Type mismatch (expected T, found U).
- Undeclared variable.
- Undeclared function or struct.
- Field not found on type.
- Wrong number of arguments.
- Invalid cast.
- Not all paths return a value (non-void function with missing return path).
- `return;` in non-void function.
- `return expr;` in void function.
- `break` or `continue` outside a loop.

---

### 5. Code Generator

**Input:** type-annotated `Program` AST  
**Output:** NASM x86-64 assembly text (`.asm` file)

#### Calling convention

System V AMD64 ABI:
- Integer/pointer arguments in order: `rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9`; rest on stack.
- Float arguments: `xmm0`–`xmm7`.
- Return value: `rax` (integer), `xmm0` (float).
- Callee saves: `rbx`, `rbp`, `r12`–`r15`.
- Stack must be 16-byte aligned before `call`.

#### Stack frame layout

Each function:
1. `push rbp` / `mov rbp, rsp`
2. Allocate locals: `sub rsp, N` (N = total size of all locals, rounded up to 16).
3. Spill parameters from registers to stack slots.
4. Emit body.
5. On `return`: load result, `mov rsp, rbp` / `pop rbp` / `ret`.

Each local variable gets a fixed `[rbp - offset]` slot. Slot size:
- Machine type (`i64`, `f64`, `null`): 8 bytes.
- Struct type with N fields: `N * 8` bytes (fields are each 8 bytes, laid out in declaration order at consecutive offsets from the slot base).
- Reference type (`&T`, `&mut T`): 8 bytes (stores a pointer).

The total frame size is the sum of all local slot sizes, rounded up to a multiple of 16.

#### Expression codegen

- `IntLit(n)` → `mov rax, n` (only appears in stdlib/raw contexts after desugaring)
- `FloatLit(f)` → store float constant in `.rodata`, `movsd xmm0, [rel _floatN]`
- `Ident(x)` → `mov rax, [rbp - offset(x)]` for machine-type vars; for struct-type vars, `lea rax, [rbp - offset(x)]` (yields a pointer to the struct on the stack)
- `UnaryExpr(Deref, e)` → emit `e` into `rax`, `mov rax, [rax]`
- `UnaryExpr(Neg, e)` / `UnaryExpr(Not, e)` → only reachable in stdlib bodies (user-level `-a`/`!a` desugar to method calls). Emit `e` into `rax`, then `neg rax` or `xor rax, 1`.
- `RefExpr(mutable, e)` → emit address of `e` into `rax` (`lea rax, [rbp - offset]`)
- `BinaryExpr` → only reachable in stdlib bodies (user-level operators desugar to method calls). Emit left into `rax`, push `rax`; emit right into `rbx`; pop left into `rax`; apply op.
- `CallExpr` — two cases:
  - **Free function call** `f(args)`: evaluate args left-to-right into ABI integer registers (`rdi`, `rsi`, …) or float registers (`xmm0`, `xmm1`, …) according to their types; `call f_label`; result in `rax` or `xmm0`.
  - **Method call** `e.method(args)` (callee is `FieldExpr(receiver, method_name)`): resolve receiver type, look up method in struct definition to find its `self` convention; pass receiver as first arg according to convention (`lea` for `&self`/`&mut self`, value copy for `move self`); pass remaining args normally; `call StructName_method_label`.
- `CastExpr(e, target)` → desugars to a method call `e.to_target()` during type checking; by codegen time it is already a `CallExpr`.
- `StructInit { name, fields }` → sort fields into declaration order; allocate `N * 8` bytes on the stack (already counted in frame size); emit each field expression and store at `[rbp - base_offset + field_index * 8]`; result is `lea rax, [rbp - base_offset]`.
- `FieldExpr(e, field_name)` → emit `e` (yields base address in `rax`); add `field_index * 8`; load 8 bytes: `mov rax, [rax + field_index * 8]`. For f64 fields: `movsd xmm0, [rax + field_index * 8]`.

#### Statement codegen

- `LetStmt` → emit init expr into `rax` (or `xmm0`), store to local's stack slot.
- `AssignStmt` → emit value, store to lvalue address.
- `ReturnStmt` → emit expr into return register, jump to function epilogue.
- `IfStmt` → condition has type `bool` (a struct with `inner_value` at offset 0). Emit condition expression (result: pointer to bool struct in `rax`); `mov rax, [rax]` to load `inner_value`; `cmp rax, 0`; `je else_label`; emit then-block; `jmp end_label`; emit else block (or fall through if no else); `end_label:`.
- `WhileStmt` → `loop_start:` emit condition (same bool-struct extraction as IfStmt); `cmp rax, 0`; `je loop_end`; emit body; `jmp loop_start`; `loop_end:`
- `BreakStmt` → `jmp` to current loop's end label.
- `ContinueStmt` → `jmp` to current loop's start label.
- `AsmStmt` → emit lines verbatim after substituting `%name` (stack slot or register of variable) and `&name` (address `[rbp - offset]` of variable). All referenced names must be in the enclosing function's scope.

#### Labels

- Function names become NASM global labels: `global main` / `main:`.
- Internal labels use a counter: `_L0`, `_L1`, etc.
- Float constants: `_F0`, `_F1`, etc. in `.section .rodata`.
- String constants: `_S0`, `_S1`, etc. in `.section .rodata`.

#### Output file structure

```
section .data
    ; (nothing in v1 — no global mutable vars)

section .rodata
    _F0: dq 0x...   ; float constants
    _S0: db "hello", 0

section .text
    global main
    main:
        push rbp
        mov rbp, rsp
        ...
        pop rbp
        ret
```

---

### 6. Assembly (NASM)

```
nasm -f elf64 -o output.o output.asm
```

---

### 7. Linking (ld)

```
ld -o output output.o
```

For programs that call C library functions (e.g. from `stdio`), link with:
```
ld -o output output.o -lc --dynamic-linker /lib64/ld-linux-x86-64.so.2
```

---

## Error Format

All compiler errors follow this format:

```
error[EXXX]: <message>
  --> <file>:<line>:<col>
   |
NN | <source line>
   | <caret(s)>
```

Error codes by stage:

| Range   | Stage         |
|---------|---------------|
| E001–E099 | Preprocessor |
| E100–E199 | Lexer        |
| E200–E399 | Parser       |
| E400–E599 | Type Checker |
| E600–E699 | Code Generator |

Multiple errors may be reported before aborting. The compiler moves to the next stage only if the previous stage produced zero errors.

---

## File Layout (Cargo Project)

```
src/
  main.rs           entry point: parse args, run pipeline, handle top-level errors
  preprocessor.rs   Preprocessor struct
  lexer.rs          Lexer struct, Token, TokenKind
  parser.rs         Parser struct, produces AST
  ast.rs            all AST node types
  types.rs          Type enum
  typechecker.rs    TypeChecker struct
  codegen.rs        CodeGen struct, emits NASM
  error.rs          Diagnostic struct, error formatting
```
