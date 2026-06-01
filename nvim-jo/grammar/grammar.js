/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: "jo",

  extras: ($) => [/\s+/, $.comment],

  word: ($) => $.ident,

  conflicts: ($) => [[$.primary_expr, $.struct_init], [$.asm_line]],

  rules: {
    source_file: ($) => repeat($.item),

    comment: (_) => token(seq("//", /.*/)),

    // -------------------------------------------------------------------------
    // Top-level items
    // -------------------------------------------------------------------------

    item: ($) =>
      choice($.import_decl, $.struct_decl, $.fn_decl, $.extend_decl),

    import_decl: ($) =>
      seq(
        "import",
        choice(
          seq(field("module", $.ident), "::", "*"),
          seq(
            field("module", $.ident),
            "::",
            field("name", $.ident),
            optional(seq("as", field("alias", $.ident)))
          ),
          field("module", $.ident)
        ),
        ";"
      ),

    struct_decl: ($) =>
      seq(
        "struct",
        field("name", $.ident),
        "{",
        repeat(choice($.field_decl, $.fn_decl)),
        "}"
      ),

    field_decl: ($) =>
      seq(field("name", $.ident), ":", field("type", $.type), ","),

    fn_decl: ($) =>
      seq(
        "fn",
        field("name", $.ident),
        "(",
        optional($.param_list),
        ")",
        field("return_type", $.type),
        field("body", $.block)
      ),

    extend_decl: ($) =>
      seq(
        "extend",
        choice(
          seq(field("module", $.ident), "::", field("name", $.ident)),
          field("name", $.ident)
        ),
        "{",
        repeat($.fn_decl),
        "}"
      ),

    param_list: ($) => seq($.param, repeat(seq(",", $.param))),

    param: ($) =>
      choice(
        seq("&", "mut", "self"),
        seq("&", "self"),
        seq("move", "self"),
        seq(field("name", $.ident), ":", field("type", $.type))
      ),

    // -------------------------------------------------------------------------
    // Types
    // -------------------------------------------------------------------------

    type: ($) =>
      choice(
        $.primitive_type,
        seq("&", "mut", $.type),
        seq("&", $.type),
        seq($.ident, "::", $.ident),
        $.ident
      ),

    primitive_type: (_) => choice("i64", "f64", "void", "null"),

    // -------------------------------------------------------------------------
    // Statements
    // -------------------------------------------------------------------------

    block: ($) => seq("{", repeat($.stmt), "}"),

    stmt: ($) =>
      choice(
        $.let_stmt,
        $.return_stmt,
        $.if_stmt,
        $.while_stmt,
        $.break_stmt,
        $.continue_stmt,
        $.asm_stmt,
        $.assign_stmt,
        $.expr_stmt
      ),

    let_stmt: ($) =>
      seq(
        "let",
        field("name", $.ident),
        optional(seq(":", field("type", $.type))),
        "=",
        field("value", $.expr),
        ";"
      ),

    assign_stmt: ($) =>
      seq(field("target", $.expr), "=", field("value", $.expr), ";"),

    return_stmt: ($) => seq("return", optional($.expr), ";"),

    if_stmt: ($) =>
      seq(
        "if",
        field("condition", $.expr),
        field("then", $.block),
        repeat(seq("else", "if", field("condition", $.expr), field("then", $.block))),
        optional(seq("else", field("else", $.block)))
      ),

    while_stmt: ($) =>
      seq("while", field("condition", $.expr), field("body", $.block)),

    break_stmt: (_) => seq("break", ";"),
    continue_stmt: (_) => seq("continue", ";"),
    expr_stmt: ($) => seq($.expr, ";"),

    asm_stmt: ($) =>
      seq("!", "asm", "{", repeat($.asm_line), "}"),

    asm_line: ($) =>
      seq(repeat1(choice($.asm_operand_value, $.asm_operand_addr, $.asm_raw))),

    asm_operand_value: ($) => seq("%", $.ident),
    asm_operand_addr: ($) => seq("&", $.ident),
    // A raw asm token: one or more non-whitespace chars that aren't % or &,
    // followed optionally by non-newline chars.
    asm_raw: (_) => token(/[^\n%&\s][^\n%&]*/),

    // -------------------------------------------------------------------------
    // Expressions (precedence via prec/prec.left)
    // -------------------------------------------------------------------------

    expr: ($) =>
      choice(
        prec.left(1, seq($.expr, "->", $.type)),            // cast
        prec.left(2, seq($.expr, "||", $.expr)),
        prec.left(3, seq($.expr, "&&", $.expr)),
        prec.left(4, seq($.expr, choice("==", "!=", "<", ">", "<=", ">="), $.expr)),
        prec.left(5, seq($.expr, choice("+", "-"), $.expr)),
        prec.left(6, seq($.expr, choice("*", "/", "%"), $.expr)),
        prec.right(7, seq("!", $.expr)),
        prec.right(7, seq("-", $.expr)),
        prec.right(7, seq("*", $.expr)),
        prec.right(7, seq("&", "mut", $.expr)),
        prec.right(7, seq("&", $.expr)),
        prec.left(8, seq($.expr, "(", optional($.arg_list), ")")),
        prec.left(8, seq($.expr, ".", $.ident)),
        $.primary_expr
      ),

    arg_list: ($) => seq($.expr, repeat(seq(",", $.expr))),

    primary_expr: ($) =>
      choice(
        $.int_lit,
        $.float_lit,
        $.char_lit,
        $.bool_lit,
        $.string_lit,
        $.null_lit,
        seq("(", $.expr, ")"),
        $.struct_init,
        seq($.ident, "::", $.ident),
        $.ident
      ),

    // -------------------------------------------------------------------------
    // Struct initializer
    // -------------------------------------------------------------------------

    struct_init: ($) =>
      seq(
        choice(seq($.ident, "::", $.ident), $.ident),
        "{",
        optional($.field_init_list),
        "}"
      ),

    field_init_list: ($) =>
      seq($.field_init, repeat(seq(",", $.field_init)), optional(",")),

    field_init: ($) =>
      seq(field("name", $.ident), "=", field("value", $.expr)),

    // -------------------------------------------------------------------------
    // Literals
    // -------------------------------------------------------------------------

    int_lit: (_) => /[0-9]+/,
    float_lit: (_) => /[0-9]*\.[0-9]+|[0-9]+\.[0-9]*/,
    char_lit: (_) =>
      token(
        seq(
          "'",
          choice(/[^'\\]/, seq("\\", /[nrt0\\'"`x]/), seq("\\x", /[0-9a-fA-F]{2}/)),
          "'"
        )
      ),
    bool_lit: (_) => choice("true", "false"),
    string_lit: (_) => token(seq('"', repeat(choice(/[^"\\]/, seq("\\", /[nrt0\\'"`x]/))), '"')),
    null_lit: (_) => "null",

    ident: (_) => /[a-zA-Z_][a-zA-Z0-9_]*/,
  },
});
