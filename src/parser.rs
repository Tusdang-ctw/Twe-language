use std::fmt;

use crate::ast::{Expr, Program, Stmt};
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
        match &self.peek().kind {
            TokenKind::Let => self.parse_let(),
            _ => {
                let expr = self.parse_expr()?;
                self.expect_stmt_end()?;
                Ok(Stmt::Expr(expr))
            }
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
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_primary()?;
        while matches!(self.peek().kind, TokenKind::LParen) {
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
            TokenKind::Ident(name) => Ok(Expr::Ident {
                name,
                line: tok.line,
                col: tok.col,
            }),
            TokenKind::LParen => {
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "expected ')' after parenthesized expression")?;
                Ok(inner)
            }
            other => Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: format!("expected expression, got {other:?}"),
                help: None,
            }),
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
