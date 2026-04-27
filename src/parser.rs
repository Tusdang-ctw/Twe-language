use std::fmt;

use crate::ast::{AssignOp, AssignTarget, BinOp, Expr, Program, Stmt, UnOp};
use crate::lexer::{Token, TokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub help: Option<String>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}

pub fn parse(tokens: &[Token]) -> Result<Program, ParseError> {
    Parser::new(tokens).parse_program()
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn bump(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek().kind, TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        Ok(Program { stmts })
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek().kind, TokenKind::Newline) {
            self.bump();
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        if matches!(self.peek().kind, TokenKind::Let) {
            return self.parse_let();
        }
        let expr = self.parse_expr()?;
        if let Some(op) = self.peek_assign_op() {
            let tok = self.bump().clone();
            let value = self.parse_expr()?;
            self.expect_stmt_end()?;
            let target = expr_to_target(&expr).ok_or_else(|| ParseError {
                line: expr.line(),
                col: expr.col(),
                message: "invalid assignment target".to_string(),
                help: Some(
                    "the left side of `=` must be a name or a field like `obj.field`"
                        .to_string(),
                ),
            })?;
            Ok(Stmt::Assign {
                target,
                op,
                value,
                line: tok.line,
                col: tok.col,
            })
        } else {
            self.expect_stmt_end()?;
            Ok(Stmt::Expr(expr))
        }
    }

    fn peek_assign_op(&self) -> Option<AssignOp> {
        match self.peek().kind {
            TokenKind::Eq => Some(AssignOp::Set),
            TokenKind::PlusEq => Some(AssignOp::AddAssign),
            TokenKind::MinusEq => Some(AssignOp::SubAssign),
            TokenKind::StarEq => Some(AssignOp::MulAssign),
            TokenKind::SlashEq => Some(AssignOp::DivAssign),
            _ => None,
        }
    }

    fn parse_let(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let name_tok = self.bump().clone();
        let name = match name_tok.kind {
            TokenKind::Ident(s) => s,
            other => {
                return Err(ParseError {
                    line: name_tok.line,
                    col: name_tok.col,
                    message: format!("expected identifier after `let`, got {other:?}"),
                    help: Some("`let <name> = <expr>` introduces a binding".to_string()),
                })
            }
        };
        self.expect(TokenKind::Eq, "expected '=' after `let <name>`")?;
        let value = self.parse_expr()?;
        self.expect_stmt_end()?;
        Ok(Stmt::Let {
            name,
            value,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek().kind, TokenKind::Or) {
            let tok = self.bump().clone();
            let right = self.parse_and()?;
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
                line: tok.line,
                col: tok.col,
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_compare()?;
        while matches!(self.peek().kind, TokenKind::And) {
            let tok = self.bump().clone();
            let right = self.parse_compare()?;
            left = Expr::Binary {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
                line: tok.line,
                col: tok.col,
            };
        }
        Ok(left)
    }

    fn parse_compare(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        let op = match self.peek().kind {
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::NotEq => BinOp::Neq,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::LtEq => BinOp::Lte,
            TokenKind::GtEq => BinOp::Gte,
            _ => return Ok(left),
        };
        let tok = self.bump().clone();
        let right = self.parse_add()?;
        if matches!(
            self.peek().kind,
            TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::LtEq
                | TokenKind::GtEq
        ) {
            let next = self.peek().clone();
            return Err(ParseError {
                line: next.line,
                col: next.col,
                message: "comparison operators do not chain in twe".to_string(),
                help: Some(
                    "split into two comparisons joined by `and`, e.g. `a < b and b < c`"
                        .to_string(),
                ),
            });
        }
        Ok(Expr::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
            line: tok.line,
            col: tok.col,
        })
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => return Ok(left),
            };
            let tok = self.bump().clone();
            let right = self.parse_mul()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                line: tok.line,
                col: tok.col,
            };
        }
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => return Ok(left),
            };
            let tok = self.bump().clone();
            let right = self.parse_unary()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                line: tok.line,
                col: tok.col,
            };
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek().kind {
            TokenKind::Minus => Some(UnOp::Neg),
            TokenKind::Not => Some(UnOp::Not),
            _ => None,
        };
        if let Some(op) = op {
            let tok = self.bump().clone();
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                op,
                operand: Box::new(operand),
                line: tok.line,
                col: tok.col,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_primary()?;
        loop {
            match self.peek().kind {
                TokenKind::LParen => {
                    let lp = self.bump().clone();
                    let mut args = Vec::new();
                    if !matches!(self.peek().kind, TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        while matches!(self.peek().kind, TokenKind::Comma) {
                            self.bump();
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.expect(TokenKind::RParen, "expected ')' to close call")?;
                    left = Expr::Call {
                        callee: Box::new(left),
                        args,
                        line: lp.line,
                        col: lp.col,
                    };
                }
                TokenKind::Dot => {
                    self.bump();
                    let name_tok = self.bump().clone();
                    let name = match name_tok.kind {
                        TokenKind::Ident(s) => s,
                        other => {
                            return Err(ParseError {
                                line: name_tok.line,
                                col: name_tok.col,
                                message: format!(
                                    "expected field name after '.', got {other:?}"
                                ),
                                help: Some(
                                    "field names must be identifiers; keywords are reserved"
                                        .to_string(),
                                ),
                            })
                        }
                    };
                    left = Expr::Field {
                        object: Box::new(left),
                        name,
                        line: name_tok.line,
                        col: name_tok.col,
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.bump().clone();
        match tok.kind {
            TokenKind::Str(value) => Ok(Expr::Str {
                value,
                line: tok.line,
                col: tok.col,
            }),
            TokenKind::Int(value) => Ok(Expr::Int {
                value,
                line: tok.line,
                col: tok.col,
            }),
            TokenKind::Float(value) => Ok(Expr::Float {
                value,
                line: tok.line,
                col: tok.col,
            }),
            TokenKind::Ident(name) => match name.as_str() {
                "true" => Ok(Expr::Bool {
                    value: true,
                    line: tok.line,
                    col: tok.col,
                }),
                "false" => Ok(Expr::Bool {
                    value: false,
                    line: tok.line,
                    col: tok.col,
                }),
                _ => Ok(Expr::Ident {
                    name,
                    line: tok.line,
                    col: tok.col,
                }),
            },
            TokenKind::LParen => self.parse_paren_or_tuple(tok.line, tok.col),
            other => Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: format!("expected expression, got {other:?}"),
                help: None,
            }),
        }
    }

    fn parse_paren_or_tuple(&mut self, lp_line: u32, lp_col: u32) -> Result<Expr, ParseError> {
        if matches!(self.peek().kind, TokenKind::RParen) {
            let tok = self.peek().clone();
            self.bump();
            return Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: "empty parens '()' have no value".to_string(),
                help: Some(
                    "use `nil` for an absent value, or put an expression inside"
                        .to_string(),
                ),
            });
        }
        let first = self.parse_expr()?;
        match self.peek().kind {
            TokenKind::RParen => {
                self.bump();
                Ok(first)
            }
            TokenKind::Comma => {
                let mut elems = vec![first];
                while matches!(self.peek().kind, TokenKind::Comma) {
                    self.bump();
                    if matches!(self.peek().kind, TokenKind::RParen) {
                        break;
                    }
                    elems.push(self.parse_expr()?);
                }
                self.expect(TokenKind::RParen, "expected ')' to close tuple")?;
                Ok(Expr::Tuple {
                    elems,
                    line: lp_line,
                    col: lp_col,
                })
            }
            _ => {
                let next = self.peek().clone();
                Err(ParseError {
                    line: next.line,
                    col: next.col,
                    message: format!(
                        "expected ',' or ')' in parenthesized expression, got {:?}",
                        next.kind
                    ),
                    help: None,
                })
            }
        }
    }

    fn expect(&mut self, kind: TokenKind, message: &str) -> Result<Token, ParseError> {
        if std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(&kind) {
            Ok(self.bump().clone())
        } else {
            let tok = self.peek();
            Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: format!("{message} (got {:?})", tok.kind),
                help: None,
            })
        }
    }

    fn expect_stmt_end(&mut self) -> Result<(), ParseError> {
        match self.peek().kind {
            TokenKind::Newline => {
                self.bump();
                Ok(())
            }
            TokenKind::Eof => Ok(()),
            ref other => {
                let tok = self.peek().clone();
                Err(ParseError {
                    line: tok.line,
                    col: tok.col,
                    message: format!(
                        "expected end of statement (newline or end of file), got {other:?}"
                    ),
                    help: Some(
                        "twe ends each statement at a newline; no semicolons".to_string(),
                    ),
                })
            }
        }
    }
}

fn expr_to_target(e: &Expr) -> Option<AssignTarget> {
    match e {
        Expr::Ident { name, .. } => Some(AssignTarget::Name(name.clone())),
        Expr::Field { object, name, .. } => Some(AssignTarget::Field {
            object: object.clone(),
            name: name.clone(),
        }),
        _ => None,
    }
}
