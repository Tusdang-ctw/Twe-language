// Tree-sitter grammar for the Twe game scripting language.
//
// Pairs with src/scanner.c, which emits the indentation-aware
// _newline / _indent / _dedent tokens. Together they parse the
// hand-written grammar in src/parser.rs to a CST suitable for
// editor tooling.
//
// Session 18 (this commit): literals + expressions + simple
// statements + blocks. Enough to parse `tests/programs/hello.twe`,
// `methods.twe`, `arithmetic.twe`, and similar declarative-block-
// free programs.
//
// Session 19 will extend with declarations (entity / scene /
// particles), states, control flow (if / while / for), spawn /
// despawn / transition, and the rest of the surface used by the
// eleven examples.

const PREC = {
  or: 1,
  and: 2,
  not: 3,
  compare: 4,
  add: 5,
  mul: 6,
  range: 7,
  unary: 8,
  call: 9,
};

module.exports = grammar({
  name: 'twe',

  externals: $ => [
    $._newline,
    $._indent,
    $._dedent,
  ],

  extras: $ => [
    // Whitespace and newlines are skipped *between* tokens unless
    // a token rule explicitly expects them. The scanner emits
    // `_newline` as a real token where statement termination is
    // required; extras kicks in only at positions where no
    // grammar rule would otherwise match `\n` (leading blank
    // lines, trailing blanks, blank lines between top-level
    // declarations, etc.).
    /[ \t\r\n]/,
    $.line_comment,
  ],

  word: $ => $.identifier,

  conflicts: $ => [
    // Tuple `(x,)` vs parenthesized `(x)` — the parser needs a
    // 1-token lookahead past `(expr` to see whether a comma
    // follows. Tree-sitter handles this with a dynamic conflict.
    [$.tuple, $.parenthesized_expression],
  ],

  rules: {
    // A program is a sequence of top-level statements. The
    // scanner emits _newline between consecutive statements.
    source_file: $ => repeat($._statement),

    // ----- statements -----

    _statement: $ => choice(
      $.let_statement,
      $.var_statement,
      $.assignment_statement,
      $.return_statement,
      $.break_statement,
      $.continue_statement,
      $.expression_statement,
    ),

    let_statement: $ => seq(
      'let',
      field('name', $.identifier),
      optional($._type_annotation),
      '=',
      field('value', $._expression),
      $._newline,
    ),

    // `var x: int = 0` is the same shape as `let` — the parser
    // collapses both forms to Stmt::Let, and the type annotation
    // is parsed-and-discarded in v0.1.
    var_statement: $ => seq(
      'var',
      field('name', $.identifier),
      optional($._type_annotation),
      '=',
      field('value', $._expression),
      $._newline,
    ),

    // Any postfix expression can be an assignment target — the
    // semantic check ("only identifier and field/index work") is
    // pushed to the next pipeline stage. This lets `self.x = ...`
    // and `xs[i] = ...` both parse without grammar ambiguity
    // between `field_expression` (RHS) and a duplicate field
    // target rule.
    assignment_statement: $ => seq(
      field('target', $._postfix_expression),
      field('op', choice('=', '+=', '-=', '*=', '/=')),
      field('value', $._expression),
      $._newline,
    ),

    return_statement: $ => seq(
      'return',
      optional(field('value', $._expression)),
      $._newline,
    ),

    break_statement: $ => seq('break', $._newline),
    continue_statement: $ => seq('continue', $._newline),

    expression_statement: $ => seq(
      $._expression,
      $._newline,
    ),

    // Type annotations are accepted and discarded by the
    // parser; we tokenize them so editor tooling can highlight.
    _type_annotation: $ => seq(':', $._type_expr),

    _type_expr: $ => choice(
      $.identifier,
      // Future: tuple types, function types, etc. Keep simple
      // for now.
    ),

    // ----- expressions -----

    _expression: $ => choice(
      $.binary_expression,
      $.unary_expression,
      $._postfix_expression,
    ),

    _postfix_expression: $ => choice(
      $.call_expression,
      $.field_expression,
      $.index_expression,
      $._atom,
    ),

    _atom: $ => choice(
      $.integer_literal,
      $.float_literal,
      $.percent_literal,
      $.quantity_literal,
      $.string_literal,
      $.boolean_literal,
      $.nil_literal,
      $.self_expression,
      $.identifier,
      $.list,
      $.tuple,
      $.parenthesized_expression,
    ),

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    tuple: $ => seq(
      '(',
      $._expression,
      ',',
      optional(seq(
        $._expression,
        repeat(seq(',', $._expression)),
        optional(','),
      )),
      ')',
    ),

    list: $ => seq(
      '[',
      optional(seq(
        $._expression,
        repeat(seq(',', $._expression)),
        optional(','),
      )),
      ']',
    ),

    // Binary operators with precedence. Tree-sitter's `prec.left`
    // / `prec.right` express associativity.
    binary_expression: $ => choice(
      prec.left(PREC.or,      seq(field('left', $._expression), field('op', 'or'),     field('right', $._expression))),
      prec.left(PREC.and,     seq(field('left', $._expression), field('op', 'and'),    field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', '=='),     field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', '!='),     field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', '<'),      field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', '<='),     field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', '>'),      field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', '>='),     field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', 'in'),     field('right', $._expression))),
      prec.left(PREC.compare, seq(field('left', $._expression), field('op', seq('not', 'in')), field('right', $._expression))),
      prec.left(PREC.add,     seq(field('left', $._expression), field('op', '+'),      field('right', $._expression))),
      prec.left(PREC.add,     seq(field('left', $._expression), field('op', '-'),      field('right', $._expression))),
      prec.left(PREC.mul,     seq(field('left', $._expression), field('op', '*'),      field('right', $._expression))),
      prec.left(PREC.mul,     seq(field('left', $._expression), field('op', '/'),      field('right', $._expression))),
      prec.left(PREC.range,   seq(field('left', $._expression), field('op', '..'),     field('right', $._expression))),
      prec.left(PREC.range,   seq(field('left', $._expression), field('op', '..<'),    field('right', $._expression))),
    ),

    unary_expression: $ => choice(
      prec(PREC.unary, seq(field('op', '-'),   field('operand', $._expression))),
      prec(PREC.not,   seq(field('op', 'not'), field('operand', $._expression))),
    ),

    call_expression: $ => prec(PREC.call, seq(
      field('function', $._postfix_expression),
      '(',
      optional(commaSep1(choice(
        $.keyword_argument,
        $._expression,
      ))),
      optional(','),
      ')',
    )),

    keyword_argument: $ => seq(
      field('name', $.identifier),
      ':',
      field('value', $._expression),
    ),

    field_expression: $ => prec.left(PREC.call, seq(
      field('object', $._postfix_expression),
      '.',
      field('name', $.identifier),
    )),

    index_expression: $ => prec.left(PREC.call, seq(
      field('object', $._postfix_expression),
      '[',
      field('index', $._expression),
      ']',
    )),

    self_expression: $ => 'self',

    // ----- literals -----

    integer_literal: $ => /\d[\d_]*/,

    float_literal: $ => /\d[\d_]*\.\d[\d_]*/,

    percent_literal: $ => /\d[\d_]*(\.\d[\d_]*)?%/,

    // Duration / mass / etc. literals — number followed by a
    // unit suffix. The set of units the lexer recognizes:
    // s, ms, min, h, kg.
    quantity_literal: $ => /\d[\d_]*(\.\d[\d_]*)?(ms|min|h|kg|s)/,

    boolean_literal: $ => choice('true', 'false'),
    nil_literal: $ => 'nil',

    // Strings: double-quoted, with `{...}` interpolation and
    // backslash escapes. Triple-quoted multi-line strings live
    // in the same node — we don't structurally distinguish them
    // for now (the editor tooling renders them the same).
    string_literal: $ => choice(
      seq('"', repeat($._string_part), '"'),
      seq('"""', repeat(choice(/[^"]/, /"[^"]/, /""[^"]/)), '"""'),
    ),

    _string_part: $ => choice(
      $._string_char,
      $.escape_sequence,
      $.interpolation,
    ),

    _string_char: $ => /[^"\\{]+/,

    escape_sequence: $ => /\\[\\"nrtfb0{}]/,

    interpolation: $ => seq('{', $._expression, '}'),

    // Identifiers must not start with a digit. The grammar's
    // `word` field is set to this so keywords win on conflict.
    identifier: $ => /[a-zA-Z_][a-zA-Z_0-9]*/,

    // Comments are extras: `# ...` to end of line.
    line_comment: $ => /#[^\n]*/,
  },
});

function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)));
}
