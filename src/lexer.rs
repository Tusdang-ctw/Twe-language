use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Let,
    Var,
    On,
    If,
    Elif,
    Else,
    And,
    Or,
    Not,
    Entity,
    Item,
    Modifier,
    Inventory,
    Extends,
    KwSelf,
    Function,
    Return,
    While,
    For,
    In,
    Break,
    Continue,
    Ident(String),
    Int(i64),
    Float(f64),
    PercentLit(f64),
    UnitLit { value: f64, unit: String },
    Str(String),
    Eq,
    EqEq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    Plus,
    Minus,
    Star,
    Slash,
    Dot,
    Comma,
    Colon,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    DotDot,
    DotDotLt,
    Newline,
    Indent,
    Dedent,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub help: Option<String>,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LexError {}

pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(src).run()
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
    bracket_depth: u32,
    indent_stack: Vec<u32>,
    indent_char: Option<u8>,
    at_line_start: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            bracket_depth: 0,
            indent_stack: vec![0],
            indent_char: None,
            at_line_start: true,
        }
    }

    fn run(&mut self) -> Result<Vec<Token>, LexError> {
        let mut out = Vec::new();
        let mut content_since_newline = false;

        loop {
            if self.at_line_start && self.bracket_depth == 0 {
                self.handle_line_start(&mut out)?;
                self.at_line_start = false;
            }

            let Some(b) = self.peek() else { break };
            let line = self.line;
            let col = self.col;

            match b {
                b' ' | b'\t' | b'\r' => {
                    self.bump();
                }
                b'\n' => {
                    self.bump_newline();
                    if self.bracket_depth == 0 && content_since_newline {
                        out.push(Token { kind: TokenKind::Newline, line, col });
                        content_since_newline = false;
                    }
                    self.at_line_start = true;
                }
                b'#' => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                b'(' => {
                    self.bump();
                    self.bracket_depth += 1;
                    out.push(Token { kind: TokenKind::LParen, line, col });
                    content_since_newline = true;
                }
                b')' => {
                    self.bump();
                    if self.bracket_depth == 0 {
                        return Err(LexError {
                            line,
                            col,
                            message: "unmatched ')'".to_string(),
                            help: Some("remove this ')' or add a matching '('".to_string()),
                        });
                    }
                    self.bracket_depth -= 1;
                    out.push(Token { kind: TokenKind::RParen, line, col });
                    content_since_newline = true;
                }
                b'[' => {
                    self.bump();
                    self.bracket_depth += 1;
                    out.push(Token { kind: TokenKind::LBracket, line, col });
                    content_since_newline = true;
                }
                b']' => {
                    self.bump();
                    if self.bracket_depth == 0 {
                        return Err(LexError {
                            line,
                            col,
                            message: "unmatched ']'".to_string(),
                            help: Some("remove this ']' or add a matching '['".to_string()),
                        });
                    }
                    self.bracket_depth -= 1;
                    out.push(Token { kind: TokenKind::RBracket, line, col });
                    content_since_newline = true;
                }
                b'{' => {
                    self.bump();
                    self.bracket_depth += 1;
                    out.push(Token { kind: TokenKind::LBrace, line, col });
                    content_since_newline = true;
                }
                b'}' => {
                    self.bump();
                    if self.bracket_depth == 0 {
                        return Err(LexError {
                            line,
                            col,
                            message: "unmatched '}'".to_string(),
                            help: Some("remove this '}' or add a matching '{'".to_string()),
                        });
                    }
                    self.bracket_depth -= 1;
                    out.push(Token { kind: TokenKind::RBrace, line, col });
                    content_since_newline = true;
                }
                b'.' => {
                    self.bump();
                    let kind = if self.peek() == Some(b'.') {
                        self.bump();
                        if self.peek() == Some(b'<') {
                            self.bump();
                            TokenKind::DotDotLt
                        } else {
                            TokenKind::DotDot
                        }
                    } else {
                        TokenKind::Dot
                    };
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b',' => {
                    self.bump();
                    out.push(Token { kind: TokenKind::Comma, line, col });
                    content_since_newline = true;
                }
                b':' => {
                    self.bump();
                    out.push(Token { kind: TokenKind::Colon, line, col });
                    content_since_newline = true;
                }
                b'=' => {
                    self.bump();
                    let kind = self.maybe_eq(TokenKind::EqEq, TokenKind::Eq);
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b'!' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        out.push(Token { kind: TokenKind::NotEq, line, col });
                        content_since_newline = true;
                    } else {
                        return Err(LexError {
                            line,
                            col,
                            message: "stray '!' — Twe uses 'not' for boolean negation".to_string(),
                            help: Some(
                                "did you mean '!=' (inequality) or 'not' (logical not)?"
                                    .to_string(),
                            ),
                        });
                    }
                }
                b'<' => {
                    self.bump();
                    let kind = self.maybe_eq(TokenKind::LtEq, TokenKind::Lt);
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b'>' => {
                    self.bump();
                    let kind = self.maybe_eq(TokenKind::GtEq, TokenKind::Gt);
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b'+' => {
                    self.bump();
                    let kind = self.maybe_eq(TokenKind::PlusEq, TokenKind::Plus);
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b'-' => {
                    self.bump();
                    let kind = self.maybe_eq(TokenKind::MinusEq, TokenKind::Minus);
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b'*' => {
                    self.bump();
                    let kind = self.maybe_eq(TokenKind::StarEq, TokenKind::Star);
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b'/' => {
                    self.bump();
                    let kind = self.maybe_eq(TokenKind::SlashEq, TokenKind::Slash);
                    out.push(Token { kind, line, col });
                    content_since_newline = true;
                }
                b'"' => {
                    out.push(self.lex_string(line, col)?);
                    content_since_newline = true;
                }
                b'0'..=b'9' => {
                    out.push(self.lex_number(line, col)?);
                    content_since_newline = true;
                }
                b if is_ident_start(b) => {
                    out.push(self.lex_ident(line, col));
                    content_since_newline = true;
                }
                _ => {
                    return Err(LexError {
                        line,
                        col,
                        message: format!("unexpected character {:?}", b as char),
                        help: None,
                    });
                }
            }
        }

        if content_since_newline {
            out.push(Token {
                kind: TokenKind::Newline,
                line: self.line,
                col: self.col,
            });
        }
        while self.indent_stack.len() > 1 {
            self.indent_stack.pop();
            out.push(Token {
                kind: TokenKind::Dedent,
                line: self.line,
                col: self.col,
            });
        }
        out.push(Token {
            kind: TokenKind::Eof,
            line: self.line,
            col: self.col,
        });
        Ok(out)
    }

    fn handle_line_start(&mut self, out: &mut Vec<Token>) -> Result<(), LexError> {
        let line = self.line;
        let col = self.col;

        let mut spaces = 0u32;
        let mut tabs = 0u32;
        while let Some(b) = self.peek() {
            match b {
                b' ' => {
                    spaces += 1;
                    self.bump();
                }
                b'\t' => {
                    tabs += 1;
                    self.bump();
                }
                _ => break,
            }
        }

        // Blank line, comment-only line, or end-of-file: skip indent processing.
        match self.peek() {
            None | Some(b'\n') | Some(b'#') => return Ok(()),
            _ => {}
        }

        if spaces > 0 && tabs > 0 {
            return Err(LexError {
                line,
                col,
                message: "mixed tabs and spaces in indentation".to_string(),
                help: Some(
                    "use either tabs or spaces consistently within a file".to_string(),
                ),
            });
        }

        let level = spaces + tabs;
        let this_char = if spaces > 0 {
            Some(b' ')
        } else if tabs > 0 {
            Some(b'\t')
        } else {
            None
        };

        if let Some(c) = this_char {
            match self.indent_char {
                None => self.indent_char = Some(c),
                Some(set) if set != c => {
                    return Err(LexError {
                        line,
                        col,
                        message: "indentation switched between tabs and spaces".to_string(),
                        help: Some("pick one and use it throughout the file".to_string()),
                    });
                }
                Some(_) => {}
            }
        }

        let current = *self
            .indent_stack
            .last()
            .expect("indent_stack always has the 0 sentinel");
        if level > current {
            self.indent_stack.push(level);
            out.push(Token {
                kind: TokenKind::Indent,
                line,
                col,
            });
        } else if level < current {
            while self.indent_stack.len() > 1
                && *self.indent_stack.last().unwrap() > level
            {
                self.indent_stack.pop();
                out.push(Token {
                    kind: TokenKind::Dedent,
                    line,
                    col,
                });
            }
            if *self.indent_stack.last().unwrap() != level {
                return Err(LexError {
                    line,
                    col,
                    message: "dedent does not match any outer indentation level"
                        .to_string(),
                    help: Some("align this line with one of the outer levels".to_string()),
                });
            }
        }

        Ok(())
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
        self.col += 1;
    }

    fn bump_newline(&mut self) {
        self.pos += 1;
        self.line += 1;
        self.col = 1;
    }

    fn maybe_eq(&mut self, with_eq: TokenKind, plain: TokenKind) -> TokenKind {
        if self.peek() == Some(b'=') {
            self.bump();
            with_eq
        } else {
            plain
        }
    }

    fn lex_string(&mut self, line: u32, col: u32) -> Result<Token, LexError> {
        self.bump(); // opening "
        let mut out = String::new();
        loop {
            let chunk_start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\n' || b == b'\\' {
                    break;
                }
                self.bump();
            }
            let chunk = &self.src[chunk_start..self.pos];
            match std::str::from_utf8(chunk) {
                Ok(s) => out.push_str(s),
                Err(_) => {
                    return Err(LexError {
                        line,
                        col,
                        message: "invalid UTF-8 in string literal".to_string(),
                        help: None,
                    })
                }
            }
            match self.peek() {
                Some(b'"') => {
                    self.bump();
                    return Ok(Token {
                        kind: TokenKind::Str(out),
                        line,
                        col,
                    });
                }
                Some(b'\n') => {
                    return Err(LexError {
                        line,
                        col,
                        message: "unterminated string literal".to_string(),
                        help: Some(
                            "close the string with '\"' before the end of the line, \
                             or use a triple-quoted string for multi-line content"
                                .to_string(),
                        ),
                    });
                }
                Some(b'\\') => {
                    let esc_line = self.line;
                    let esc_col = self.col;
                    self.bump();
                    let esc = self.peek().ok_or_else(|| LexError {
                        line: esc_line,
                        col: esc_col,
                        message: "unterminated escape sequence at end of file".to_string(),
                        help: None,
                    })?;
                    let ch = match esc {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'\\' => '\\',
                        b'"' => '"',
                        b'0' => '\0',
                        other => {
                            return Err(LexError {
                                line: esc_line,
                                col: esc_col,
                                message: format!("unknown escape '\\{}'", other as char),
                                help: Some(
                                    "supported: \\n \\r \\t \\\\ \\\" \\0".to_string(),
                                ),
                            });
                        }
                    };
                    out.push(ch);
                    self.bump();
                }
                None => {
                    return Err(LexError {
                        line,
                        col,
                        message: "unterminated string literal at end of file".to_string(),
                        help: Some("add a closing '\"'".to_string()),
                    })
                }
                _ => unreachable!("loop only breaks on \", \\n, \\\\, or end-of-input"),
            }
        }
    }

    fn lex_number(&mut self, line: u32, col: u32) -> Result<Token, LexError> {
        // 0x / 0b prefix radix literals are parsed as ints with no
        // unit / percent suffix. They must be consumed before falling
        // through to the decimal path.
        if self.peek() == Some(b'0') {
            match self.src.get(self.pos + 1) {
                Some(b'x') | Some(b'X') => {
                    self.bump();
                    self.bump();
                    return self.lex_radix(line, col, 16, "0x");
                }
                Some(b'b') | Some(b'B') => {
                    self.bump();
                    self.bump();
                    return self.lex_radix(line, col, 2, "0b");
                }
                _ => {}
            }
        }

        let start = self.pos;
        loop {
            match self.peek() {
                Some(b) if b.is_ascii_digit() => self.bump(),
                Some(b'_')
                    if self
                        .src
                        .get(self.pos + 1)
                        .is_some_and(|b| b.is_ascii_digit()) =>
                {
                    self.bump();
                }
                _ => break,
            }
        }
        let mut is_float = false;
        if self.peek() == Some(b'.')
            && self
                .src
                .get(self.pos + 1)
                .is_some_and(|b| b.is_ascii_digit())
        {
            is_float = true;
            self.bump(); // dot
            loop {
                match self.peek() {
                    Some(b) if b.is_ascii_digit() => self.bump(),
                    Some(b'_')
                        if self
                            .src
                            .get(self.pos + 1)
                            .is_some_and(|b| b.is_ascii_digit()) =>
                    {
                        self.bump();
                    }
                    _ => break,
                }
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            let exp_start = self.pos;
            loop {
                match self.peek() {
                    Some(b) if b.is_ascii_digit() => self.bump(),
                    Some(b'_')
                        if self
                            .src
                            .get(self.pos + 1)
                            .is_some_and(|b| b.is_ascii_digit()) =>
                    {
                        self.bump();
                    }
                    _ => break,
                }
            }
            if self.pos == exp_start {
                return Err(LexError {
                    line,
                    col,
                    message: "expected digits after exponent marker".to_string(),
                    help: Some("write a digit (e.g. '1e5' or '1.5e-3')".to_string()),
                });
            }
        }
        let raw = std::str::from_utf8(&self.src[start..self.pos])
            .expect("ascii digits are valid utf-8");
        let cleaned: String = raw.chars().filter(|c| *c != '_').collect();
        let numeric_value: f64 = cleaned.parse().map_err(|_| LexError {
            line,
            col,
            message: format!("could not parse number '{raw}'"),
            help: None,
        })?;

        // Optional percent suffix.
        if self.peek() == Some(b'%') {
            self.bump();
            return Ok(Token {
                kind: TokenKind::PercentLit(numeric_value),
                line,
                col,
            });
        }

        // Optional unit suffix (contiguous, ASCII letters only). v0.1 supports
        // single-symbol units; compound forms like `5 m/s` ship in Phase 2.
        if self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
            let unit_start = self.pos;
            while let Some(b) = self.peek() {
                if b.is_ascii_alphabetic() {
                    self.bump();
                } else {
                    break;
                }
            }
            let unit = std::str::from_utf8(&self.src[unit_start..self.pos])
                .expect("ascii letters are valid utf-8")
                .to_string();
            if !is_known_unit(&unit) {
                return Err(LexError {
                    line,
                    col,
                    message: format!("unknown unit suffix '{unit}'"),
                    help: Some(
                        "valid suffixes: s ms min h, m cm mm km px, kg g mg, deg rad"
                            .to_string(),
                    ),
                });
            }
            return Ok(Token {
                kind: TokenKind::UnitLit {
                    value: numeric_value,
                    unit,
                },
                line,
                col,
            });
        }

        if is_float {
            Ok(Token {
                kind: TokenKind::Float(numeric_value),
                line,
                col,
            })
        } else {
            let value: i64 = cleaned.parse().map_err(|_| LexError {
                line,
                col,
                message: format!("integer literal '{raw}' is out of range for int (i64)"),
                help: Some("twe ints are 64-bit signed; pick a smaller value".to_string()),
            })?;
            Ok(Token {
                kind: TokenKind::Int(value),
                line,
                col,
            })
        }
    }

    fn lex_radix(
        &mut self,
        line: u32,
        col: u32,
        radix: u32,
        prefix: &str,
    ) -> Result<Token, LexError> {
        let start = self.pos;
        loop {
            match self.peek() {
                Some(b) if (b as char).is_digit(radix) => self.bump(),
                Some(b'_')
                    if self
                        .src
                        .get(self.pos + 1)
                        .is_some_and(|c| (*c as char).is_digit(radix)) =>
                {
                    self.bump();
                }
                _ => break,
            }
        }
        if self.pos == start {
            return Err(LexError {
                line,
                col,
                message: format!("expected digits after '{prefix}'"),
                help: Some(if radix == 16 {
                    "e.g. `0xFF` or `0xFF_FF_FF_FF`".to_string()
                } else {
                    "e.g. `0b1010` or `0b1111_0000`".to_string()
                }),
            });
        }
        let raw = std::str::from_utf8(&self.src[start..self.pos])
            .expect("ascii digits are valid utf-8");
        let cleaned: String = raw.chars().filter(|c| *c != '_').collect();
        let value = i64::from_str_radix(&cleaned, radix).map_err(|_| LexError {
            line,
            col,
            message: format!("{prefix}{cleaned} is out of range for int (i64)"),
            help: None,
        })?;
        Ok(Token {
            kind: TokenKind::Int(value),
            line,
            col,
        })
    }

    fn lex_ident(&mut self, line: u32, col: u32) -> Token {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if is_ident_continue(b) {
                self.bump();
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .expect("ascii ident chars are valid utf-8");
        let kind = match text {
            "let" => TokenKind::Let,
            "var" => TokenKind::Var,
            "on" => TokenKind::On,
            "if" => TokenKind::If,
            "elif" => TokenKind::Elif,
            "else" => TokenKind::Else,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "entity" => TokenKind::Entity,
            "item" => TokenKind::Item,
            "modifier" => TokenKind::Modifier,
            "inventory" => TokenKind::Inventory,
            "extends" => TokenKind::Extends,
            "self" => TokenKind::KwSelf,
            "function" => TokenKind::Function,
            "return" => TokenKind::Return,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            _ => TokenKind::Ident(text.to_string()),
        };
        Token { kind, line, col }
    }
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_continue(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

fn is_known_unit(s: &str) -> bool {
    matches!(
        s,
        "s" | "ms"
            | "min"
            | "h"
            | "m"
            | "cm"
            | "mm"
            | "km"
            | "px"
            | "kg"
            | "g"
            | "mg"
            | "deg"
            | "rad"
    )
}
