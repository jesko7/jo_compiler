; Keywords
"fn" @keyword.function
"struct" @keyword.type
"extend" @keyword.type
"let" @keyword
"return" @keyword.return
"if" @keyword.conditional
"else" @keyword.conditional
"while" @keyword.repeat
"break" @keyword.repeat
"continue" @keyword.repeat
"import" @keyword.import
"as" @keyword
"move" @keyword
"asm" @keyword

; self keyword
"self" @variable.builtin

; Primitive types
(primitive_type) @type.builtin

; Struct / extend type names
(struct_decl name: (ident) @type)
(extend_decl name: (ident) @type)

; Function names in declarations
(fn_decl name: (ident) @function)

; Parameters
(param name: (ident) @variable.parameter)

; Let bindings
(let_stmt name: (ident) @variable)

; Field declarations in struct body
(field_decl name: (ident) @variable.member)

; Field init in struct literals: Person { age = 5 }
(field_init name: (ident) @variable.member)

; Struct initialiser name: Person { ... } or mod::Person { ... }
(struct_init (ident) @type .)
(struct_init (ident) @module "::")

; Qualified ident: first part is module/type, second is function
(primary_expr (ident) @type "::" (ident) @function)

; Method calls: (expr (expr (expr ...) (ident)) (arg_list))
; The ident right before an arg_list inside a field-access expr is the method name.
(expr
  (expr
    (expr)
    (ident) @function.method)
  (arg_list))

; Literals
(int_lit) @number
(float_lit) @number.float
(char_lit) @character
(bool_lit) @boolean
(string_lit) @string
(null_lit) @constant.builtin

; Operators
"+" @operator
"-" @operator
"*" @operator
"/" @operator
"%" @operator
"==" @operator
"!=" @operator
"<" @operator
">" @operator
"<=" @operator
">=" @operator
"&&" @operator
"||" @operator
"!" @operator
"->" @operator
"&" @operator
"=" @operator

; Delimiters / brackets
"(" @punctuation.bracket
")" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
"," @punctuation.delimiter
";" @punctuation.delimiter
":" @punctuation.delimiter
"::" @punctuation.delimiter
"." @punctuation.delimiter

; Comments
(comment) @comment

; Inline asm
(asm_operand_value "%" @punctuation.special (ident) @variable)
(asm_operand_addr  "&" @punctuation.special (ident) @variable)
(asm_raw) @string.special
