// Tree-sitter grammar for the Twe game scripting language.
//
// Pairs with src/scanner.c, which emits the indentation-aware
// _newline / _indent / _dedent tokens.
//
// Sessions 18 + 19 of Phase 3 — covers every construct used by
// the eleven examples in `docs/01-examples.md` +
// `docs/example-11-snake.md`.

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
    // Inside a state body, `on update(dt):` could match either
    // on_update_handler (a state member) or on_update_statement
    // (which leaks in via _state_member -> _statement). Tell
    // tree-sitter to keep both interpretations live until the
    // surrounding context disambiguates — `_state_member` lists
    // on_update_handler first so it wins inside state bodies.
    [$.on_update_handler, $.on_update_statement],
  ],

  rules: {
    // A program is a sequence of top-level statements.
    source_file: $ => repeat($._statement),

    // ----- statements -----

    _statement: $ => choice(
      $.let_statement,
      $.var_statement,
      $.assignment_statement,
      $.return_statement,
      $.break_statement,
      $.continue_statement,
      $.if_statement,
      $.while_statement,
      $.for_statement,
      $.function_declaration,
      $.declaration,
      $.spawn_statement,
      $.despawn_statement,
      $.transition_statement,
      $.on_update_statement,
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
    // collapses both to Stmt::Let; the type annotation is parsed-
    // and-discarded in v0.1.
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
    // pushed past parsing. This avoids LR ambiguity between
    // `field_expression` (RHS) and a duplicate target rule.
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

    if_statement: $ => seq(
      'if',
      field('condition', $._expression),
      ':',
      field('consequence', $.block),
      repeat(field('elif', $.elif_clause)),
      optional(field('else', $.else_clause)),
    ),

    elif_clause: $ => seq(
      'elif',
      field('condition', $._expression),
      ':',
      field('body', $.block),
    ),

    else_clause: $ => seq(
      'else',
      ':',
      field('body', $.block),
    ),

    while_statement: $ => seq(
      'while',
      field('condition', $._expression),
      ':',
      field('body', $.block),
    ),

    for_statement: $ => seq(
      'for',
      field('var', $.identifier),
      'in',
      field('iter', $._expression),
      ':',
      field('body', $.block),
    ),

    function_declaration: $ => seq(
      'function',
      field('name', $.identifier),
      '(',
      optional(commaSep1(field('parameter', $.parameter))),
      ')',
      // `-> type` return annotation. Tokenized + discarded
      // (semantically a no-op in v0.1's non-strict mode).
      optional(seq('->', $._type_expr)),
      optional($._type_annotation),
      ':',
      field('body', $.block),
    ),

    // Parameter with optional `: type` annotation.
    parameter: $ => seq(
      $.identifier,
      optional($._type_annotation),
    ),

    // Declarative blocks: entity / item / modifier / inventory /
    // scene / particles. The body is a sequence of declaration
    // members (fields, methods, states, initial:).
    declaration: $ => seq(
      field('kind', choice(
        'entity', 'item', 'modifier', 'inventory', 'scene', 'particles',
      )),
      field('name', $.identifier),
      optional(seq('extends', field('parent', $.identifier))),
      ':',
      field('body', $.declaration_body),
    ),

    declaration_body: $ => seq(
      $._indent,
      repeat($._declaration_member),
      $._dedent,
    ),

    _declaration_member: $ => choice(
      $.field_declaration,
      $.method_declaration,
      $.initial_state_declaration,
      $.state_declaration,
    ),

    // Field declarations come in two source-level forms:
    //   `name: value`         — terse form for declarative blocks
    //   `var name = value`    — verbose form, equivalent semantics
    //   `var name: type = value` — verbose with type annotation
    field_declaration: $ => choice(
      seq(
        field('name', $.identifier),
        ':',
        field('value', $._expression),
        $._newline,
      ),
      seq(
        'var',
        field('name', $.identifier),
        optional($._type_annotation),
        '=',
        field('value', $._expression),
        $._newline,
      ),
    ),

    // Methods come in two forms inside a declaration body:
    //   `name(params):`           — terse
    //   `function name(params):`  — verbose, equivalent
    // The verbose form is what spawn_entities.twe + survive.twe
    // use; the terse form is what methods.twe uses. The parser
    // collapses both to DeclMember::Method.
    method_declaration: $ => seq(
      optional('function'),
      field('name', $.identifier),
      '(',
      optional(commaSep1(field('parameter', $.parameter))),
      ')',
      optional(seq('->', $._type_expr)),
      optional($._type_annotation),
      ':',
      field('body', $.block),
    ),

    initial_state_declaration: $ => seq(
      'initial',
      ':',
      field('name', $.identifier),
      $._newline,
    ),

    state_declaration: $ => seq(
      'state',
      field('name', $.identifier),
      ':',
      field('body', $.state_body),
    ),

    // A state body can be empty (terminal idle state, common for
    // `state done:` patterns). Same shape as `block`.
    state_body: $ => choice(
      seq($._indent, repeat1($._state_member), $._dedent),
      $._newline,
    ),

    _state_member: $ => choice(
      $.every_clock,
      $.on_render_handler,
      $.on_update_handler,
      $.on_key_press_handler,
      $._statement,
    ),

    every_clock: $ => seq(
      'every',
      field('interval', $._expression),
      ':',
      field('body', $.block),
    ),

    on_render_handler: $ => seq(
      'on',
      'render',
      '(',
      ')',
      ':',
      field('body', $.block),
    ),

    on_update_handler: $ => seq(
      'on',
      'update',
      '(',
      field('parameter', $.identifier),
      ')',
      ':',
      field('body', $.block),
    ),

    on_key_press_handler: $ => seq(
      'on',
      'key_press',
      '.',
      field('key', $.identifier),
      ':',
      field('body', $.block),
    ),

    // Top-level `on update(dt):` — same shape as the state-scoped
    // handler but reachable as a script statement.
    on_update_statement: $ => seq(
      'on',
      'update',
      '(',
      field('parameter', $.identifier),
      ')',
      ':',
      field('body', $.block),
    ),

    spawn_statement: $ => seq(
      'spawn',
      field('class', $.identifier),
      optional(seq('at', field('at', $._expression))),
      $._newline,
    ),

    despawn_statement: $ => seq(
      'despawn',
      field('target', $._expression),
      $._newline,
    ),

    transition_statement: $ => seq(
      '->',
      field('target', $.identifier),
      $._newline,
    ),

    expression_statement: $ => seq(
      $._expression,
      $._newline,
    ),

    // Indented block: either a real indented region, or just the
    // line-terminator that closes the header (when the body is
    // comment-only or no-content — the scanner doesn't emit
    // INDENT in that case, matching the hand-written parser's
    // `parse_block` in src/parser.rs which returns Vec::new()
    // when it sees Newline-not-followed-by-Indent).
    block: $ => choice(
      seq($._indent, repeat1($._statement), $._dedent),
      $._newline,
    ),

    _type_annotation: $ => seq(':', $._type_expr),

    _type_expr: $ => $.identifier,

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

    // Float: required `.` plus fractional part, optional scientific
    // exponent (`e+3`, `e-12`, `E5`). Matches the lexer in
    // src/lexer.rs.
    float_literal: $ => /\d[\d_]*\.\d[\d_]*([eE][+-]?\d+)?/,

    percent_literal: $ => /\d[\d_]*(\.\d[\d_]*)?%/,

    // Duration / mass literals — number followed by a unit
    // suffix. Matches the lexer in src/lexer.rs.
    quantity_literal: $ => /\d[\d_]*(\.\d[\d_]*)?(ms|min|h|kg|s)/,

    boolean_literal: $ => choice('true', 'false'),
    nil_literal: $ => 'nil',

    // Strings: double-quoted with `{...}` interpolation and
    // backslash escapes. Triple-quoted multi-line strings live
    // in the same node — not structurally distinguished here
    // (editor highlighting renders them the same).
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

    identifier: $ => /[a-zA-Z_][a-zA-Z_0-9]*/,

    line_comment: $ => /#[^\n]*/,
  },
});

function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)));
}
