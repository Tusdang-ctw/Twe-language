use std::fmt;

use crate::ast::{
    AssignOp, AssignTarget, BinOp, DeclKind, DeclMember, Expr, Program, StateMember, Stmt, UnOp,
};
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
        match self.peek().kind {
            TokenKind::Let | TokenKind::Var => return self.parse_let(),
            TokenKind::If => return self.parse_if(),
            TokenKind::On => return self.parse_on(),
            TokenKind::Entity => return self.parse_decl(DeclKind::Entity),
            TokenKind::Item => return self.parse_decl(DeclKind::Item),
            TokenKind::Modifier => return self.parse_decl(DeclKind::Modifier),
            TokenKind::Inventory => return self.parse_decl(DeclKind::Inventory),
            TokenKind::Scene => return self.parse_decl(DeclKind::Scene),
            TokenKind::Particles => return self.parse_decl(DeclKind::Particles),
            TokenKind::Function => return self.parse_function(),
            TokenKind::Return => return self.parse_return(),
            TokenKind::While => return self.parse_while(),
            TokenKind::For => return self.parse_for(),
            TokenKind::Break => {
                let tok = self.bump().clone();
                self.expect_stmt_end()?;
                return Ok(Stmt::Break {
                    line: tok.line,
                    col: tok.col,
                });
            }
            TokenKind::Continue => {
                let tok = self.bump().clone();
                self.expect_stmt_end()?;
                return Ok(Stmt::Continue {
                    line: tok.line,
                    col: tok.col,
                });
            }
            TokenKind::Minus
                if self.pos + 1 < self.tokens.len()
                    && matches!(self.tokens[self.pos + 1].kind, TokenKind::Gt) =>
            {
                return self.parse_transition();
            }
            TokenKind::Spawn => return self.parse_spawn(),
            TokenKind::Despawn => return self.parse_despawn(),
            _ => {}
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

    fn parse_if(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let cond = self.parse_expr()?;
        self.expect(TokenKind::Colon, "expected ':' after if condition")?;
        let then_body = self.parse_block()?;
        let mut elifs = Vec::new();
        while matches!(self.peek().kind, TokenKind::Elif) {
            self.bump();
            let elif_cond = self.parse_expr()?;
            self.expect(TokenKind::Colon, "expected ':' after elif condition")?;
            let elif_body = self.parse_block()?;
            elifs.push((elif_cond, elif_body));
        }
        let else_body = if matches!(self.peek().kind, TokenKind::Else) {
            self.bump();
            self.expect(TokenKind::Colon, "expected ':' after `else`")?;
            Some(self.parse_block()?)
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then_body,
            elifs,
            else_body,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_on(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let event_tok = self.bump().clone();
        let event_name = match event_tok.kind {
            TokenKind::Ident(s) => s,
            other => {
                return Err(ParseError {
                    line: event_tok.line,
                    col: event_tok.col,
                    message: format!("expected event name after `on`, got {other:?}"),
                    help: Some("e.g. `on update(dt):`".to_string()),
                })
            }
        };
        if event_name != "update" {
            return Err(ParseError {
                line: event_tok.line,
                col: event_tok.col,
                message: format!(
                    "only `on update(dt):` is supported in v0.1, got `on {event_name}`"
                ),
                help: Some(
                    "named, predicate, and lifecycle events ship in Phase 2".to_string(),
                ),
            });
        }
        self.expect(TokenKind::LParen, "expected '(' after `on update`")?;
        let param_tok = self.bump().clone();
        let param = match param_tok.kind {
            TokenKind::Ident(s) => s,
            other => {
                return Err(ParseError {
                    line: param_tok.line,
                    col: param_tok.col,
                    message: format!("expected parameter name, got {other:?}"),
                    help: Some("`on update(dt):` binds dt as the frame delta".to_string()),
                })
            }
        };
        self.expect(TokenKind::RParen, "expected ')' to close parameter list")?;
        self.expect(TokenKind::Colon, "expected ':' after `on update(...)`")?;
        let body = self.parse_block()?;
        Ok(Stmt::OnUpdate {
            param,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_spawn(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let class = self.expect_ident("expected class name after `spawn`")?;
        // Optional `at <expr>` — `at` is a contextual keyword here, lexed
        // as an Ident.
        let at = if matches!(&self.peek().kind, TokenKind::Ident(s) if s == "at") {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect_stmt_end()?;
        Ok(Stmt::Spawn {
            class,
            at,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_despawn(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let target = self.parse_expr()?;
        self.expect_stmt_end()?;
        Ok(Stmt::Despawn {
            target,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_transition(&mut self) -> Result<Stmt, ParseError> {
        let arrow_line = self.peek().line;
        let arrow_col = self.peek().col;
        self.bump(); // -
        self.bump(); // >
        let target = self.expect_ident("expected state name after `->`")?;
        self.expect_stmt_end()?;
        Ok(Stmt::Transition {
            target,
            line: arrow_line,
            col: arrow_col,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let cond = self.parse_expr()?;
        self.expect(TokenKind::Colon, "expected ':' after while condition")?;
        let body = self.parse_block()?;
        Ok(Stmt::While {
            cond,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let var = self.expect_ident("expected loop variable name after `for`")?;
        self.expect(TokenKind::In, "expected `in` after for-loop variable")?;
        let iter = self.parse_expr()?;
        self.expect(TokenKind::Colon, "expected ':' after for-loop iterable")?;
        let body = self.parse_block()?;
        Ok(Stmt::For {
            var,
            iter,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_function(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let name = self.expect_ident("expected function name after `function`")?;
        self.expect(TokenKind::LParen, "expected '(' after function name")?;
        let mut params = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            params.push(self.parse_param()?);
            while matches!(self.peek().kind, TokenKind::Comma) {
                self.bump();
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RParen, "expected ')' to close parameter list")?;
        // Optional return type `-> type`. Currently no Arrow token; the
        // design uses `->` which lexes as Minus + Gt. Recognise that pair.
        if matches!(self.peek().kind, TokenKind::Minus) {
            // peek ahead one — we only consume both if the second is `>`.
            if self.pos + 1 < self.tokens.len()
                && matches!(self.tokens[self.pos + 1].kind, TokenKind::Gt)
            {
                self.bump();
                self.bump();
                self.parse_type()?;
            }
        }
        self.expect(TokenKind::Colon, "expected ':' after function signature")?;
        let body = self.parse_block()?;
        Ok(Stmt::FunctionDecl {
            name,
            params,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_param(&mut self) -> Result<String, ParseError> {
        let name = self.expect_ident("expected parameter name")?;
        if matches!(self.peek().kind, TokenKind::Colon) {
            self.bump();
            self.parse_type()?;
        }
        Ok(name)
    }

    fn parse_return(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let value = if matches!(self.peek().kind, TokenKind::Newline | TokenKind::Eof) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect_stmt_end()?;
        Ok(Stmt::Return {
            value,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_decl(&mut self, kind: DeclKind) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let name_tok = self.bump().clone();
        let name = match name_tok.kind {
            TokenKind::Ident(s) => s,
            other => {
                return Err(ParseError {
                    line: name_tok.line,
                    col: name_tok.col,
                    message: format!(
                        "expected name after `{}`, got {other:?}",
                        kind.as_str()
                    ),
                    help: Some(
                        "declarative block names use PascalCase, e.g. `item Sword:`"
                            .to_string(),
                    ),
                })
            }
        };
        let parent = if matches!(self.peek().kind, TokenKind::Extends) {
            self.bump();
            let p_tok = self.bump().clone();
            match p_tok.kind {
                TokenKind::Ident(s) => Some(s),
                other => {
                    return Err(ParseError {
                        line: p_tok.line,
                        col: p_tok.col,
                        message: format!("expected parent name after `extends`, got {other:?}"),
                        help: Some(
                            "single inheritance: `item FlameBlade extends Sword:`".to_string(),
                        ),
                    })
                }
            }
        } else {
            None
        };
        self.expect(
            TokenKind::Colon,
            &format!("expected ':' after `{} {}`", kind.as_str(), name),
        )?;
        let members = self.parse_decl_body()?;
        Ok(Stmt::Decl {
            kind,
            name,
            parent,
            members,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_decl_body(&mut self) -> Result<Vec<DeclMember>, ParseError> {
        if !matches!(self.peek().kind, TokenKind::Newline) {
            let tok = self.peek().clone();
            return Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: "expected indented block body after declarative block header".to_string(),
                help: Some(
                    "declarative blocks must have an indented body, even if empty".to_string(),
                ),
            });
        }
        self.bump();
        if !matches!(self.peek().kind, TokenKind::Indent) {
            let tok = self.peek().clone();
            return Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: "expected indent for declarative block body".to_string(),
                help: None,
            });
        }
        self.bump();
        let mut members = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek().kind, TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            members.push(self.parse_decl_member()?);
        }
        if matches!(self.peek().kind, TokenKind::Dedent) {
            self.bump();
        }
        Ok(members)
    }

    fn parse_decl_member(&mut self) -> Result<DeclMember, ParseError> {
        if matches!(self.peek().kind, TokenKind::State) {
            return self.parse_state_member();
        }
        // Explicit `function` keyword inside a declarative-block body.
        // Same shape as the implicit `name(params): body` method form.
        if matches!(self.peek().kind, TokenKind::Function) {
            self.bump();
        }
        // `var name ...` / `let name ...` are explicit-mutability prefixes.
        // After them, the syntax follows binding form: `name [: type] = value`,
        // mirroring top-level `let`/`var`. v0.1 ignores the var/let
        // distinction (all fields are mutable) but accepts the keyword for
        // examples like docs/example-11-snake.md.
        let mutability_prefixed = matches!(self.peek().kind, TokenKind::Var | TokenKind::Let);
        if mutability_prefixed {
            self.bump();
        }
        let name_tok = self.bump().clone();
        let name = match name_tok.kind {
            TokenKind::Ident(s) => s,
            other => {
                return Err(ParseError {
                    line: name_tok.line,
                    col: name_tok.col,
                    message: format!("expected member name, got {other:?}"),
                    help: Some(
                        "members are fields (`name: value`) or methods (`name(params): body`)"
                            .to_string(),
                    ),
                })
            }
        };
        // After a var/let prefix, accept binding-form `name [: type] = value`.
        if mutability_prefixed {
            if matches!(self.peek().kind, TokenKind::Colon) {
                self.bump();
                self.parse_type()?;
            }
            self.expect(TokenKind::Eq, "expected '=' in field declaration")?;
            let value = self.parse_expr()?;
            self.expect_stmt_end()?;
            return Ok(DeclMember::Field {
                name,
                value,
                line: name_tok.line,
                col: name_tok.col,
            });
        }
        match self.peek().kind {
            TokenKind::LParen => {
                self.bump();
                let mut params = Vec::new();
                if !matches!(self.peek().kind, TokenKind::RParen) {
                    params.push(self.parse_param()?);
                    while matches!(self.peek().kind, TokenKind::Comma) {
                        self.bump();
                        params.push(self.parse_param()?);
                    }
                }
                self.expect(TokenKind::RParen, "expected ')' after parameter list")?;
                // Optional return type `-> type` parsed-and-ignored.
                if matches!(self.peek().kind, TokenKind::Minus)
                    && self.pos + 1 < self.tokens.len()
                    && matches!(self.tokens[self.pos + 1].kind, TokenKind::Gt)
                {
                    self.bump();
                    self.bump();
                    self.parse_type()?;
                }
                self.expect(TokenKind::Colon, "expected ':' after parameter list")?;
                let body = self.parse_block()?;
                Ok(DeclMember::Method {
                    name,
                    params,
                    body,
                    line: name_tok.line,
                    col: name_tok.col,
                })
            }
            TokenKind::Colon => {
                self.bump();
                // Optional type annotation `field: type = value` per docs/06 §3.3.
                // We don't have type-only fields yet; v0.1 always expects a value.
                // If the next-next is `=`, drop the type. Otherwise treat the
                // expression after ':' as the value.
                if self.looks_like_typed_field() {
                    self.parse_type()?;
                    self.expect(TokenKind::Eq, "expected '=' after field type")?;
                }
                // Special case: `initial: <state_name>` is a state-machine
                // declaration, not a regular field. Recognise it before
                // parsing as expression so the state name isn't required to
                // be a defined identifier.
                if name == "initial" {
                    let state_tok = self.peek().clone();
                    if let TokenKind::Ident(state_name) = &state_tok.kind {
                        let state_name = state_name.clone();
                        self.bump();
                        self.expect_stmt_end()?;
                        return Ok(DeclMember::InitialState {
                            name: state_name,
                            line: name_tok.line,
                            col: name_tok.col,
                        });
                    }
                }
                let value = self.parse_expr()?;
                self.expect_stmt_end()?;
                Ok(DeclMember::Field {
                    name,
                    value,
                    line: name_tok.line,
                    col: name_tok.col,
                })
            }
            ref other => Err(ParseError {
                line: name_tok.line,
                col: name_tok.col,
                message: format!(
                    "expected ':' for a field or '(' for a method after member name, got {other:?}"
                ),
                help: None,
            }),
        }
    }

    /// Heuristic: after seeing `name:`, decide if what follows is
    /// `<type> = <expr>` (typed field) vs. `<expr>` (untyped field).
    /// We say it's typed if the first identifier-or-keyword is followed
    /// by `=`. This is cheap and covers the v0.1 examples.
    fn looks_like_typed_field(&self) -> bool {
        let mut depth = 0_i32;
        for i in self.pos..self.tokens.len() {
            match &self.tokens[i].kind {
                TokenKind::Newline | TokenKind::Eof => return false,
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth -= 1,
                TokenKind::Eq if depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    fn parse_state_member(&mut self) -> Result<DeclMember, ParseError> {
        let kw = self.bump().clone(); // `state`
        let name = self.expect_ident("expected state name after `state`")?;
        self.expect(TokenKind::Colon, "expected ':' after state name")?;
        let members = self.parse_state_body()?;
        Ok(DeclMember::State {
            name,
            members,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_state_body(&mut self) -> Result<Vec<StateMember>, ParseError> {
        if !matches!(self.peek().kind, TokenKind::Newline) {
            let tok = self.peek().clone();
            return Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: "expected indented body after `state <name>:`".to_string(),
                help: None,
            });
        }
        self.bump();
        // Empty state body is fine — comment-only or no-content states
        // don't trigger an Indent token from the lexer.
        if !matches!(self.peek().kind, TokenKind::Indent) {
            return Ok(Vec::new());
        }
        self.bump();
        let mut members = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek().kind, TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            members.push(self.parse_state_inner_member()?);
        }
        if matches!(self.peek().kind, TokenKind::Dedent) {
            self.bump();
        }
        Ok(members)
    }

    fn parse_state_inner_member(&mut self) -> Result<StateMember, ParseError> {
        if matches!(self.peek().kind, TokenKind::Every) {
            let kw = self.bump().clone();
            let interval = self.parse_expr()?;
            self.expect(TokenKind::Colon, "expected ':' after every <duration>")?;
            let body = self.parse_block()?;
            return Ok(StateMember::Every {
                interval,
                body,
                line: kw.line,
                col: kw.col,
            });
        }
        // `on render():` — special-cased inside states.
        if matches!(self.peek().kind, TokenKind::On)
            && self.pos + 1 < self.tokens.len()
            && matches!(&self.tokens[self.pos + 1].kind, TokenKind::Ident(s) if s == "render")
        {
            let kw = self.bump().clone();
            self.bump(); // ident "render"
            self.expect(TokenKind::LParen, "expected '(' after `on render`")?;
            self.expect(TokenKind::RParen, "expected ')' after `on render(`")?;
            self.expect(TokenKind::Colon, "expected ':' after `on render()`")?;
            let body = self.parse_block()?;
            return Ok(StateMember::OnRender {
                body,
                line: kw.line,
                col: kw.col,
            });
        }
        // `on key_press.<key>: body` per docs/example-11-snake.md NP1.
        if matches!(self.peek().kind, TokenKind::On)
            && self.pos + 1 < self.tokens.len()
            && matches!(&self.tokens[self.pos + 1].kind, TokenKind::Ident(s) if s == "key_press")
        {
            let kw = self.bump().clone();
            self.bump(); // ident "key_press"
            self.expect(TokenKind::Dot, "expected '.<key>' after `on key_press`")?;
            let key = self.expect_ident("expected key name after `on key_press.`")?;
            self.expect(TokenKind::Colon, "expected ':' after `on key_press.<key>`")?;
            let body = self.parse_block()?;
            return Ok(StateMember::OnKeyPress {
                key,
                body,
                line: kw.line,
                col: kw.col,
            });
        }
        let stmt = self.parse_stmt()?;
        Ok(StateMember::Stmt(stmt))
    }

    fn expect_ident(&mut self, message: &str) -> Result<String, ParseError> {
        let tok = self.bump().clone();
        match tok.kind {
            TokenKind::Ident(s) => Ok(s),
            other => Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: format!("{message} (got {other:?})"),
                help: None,
            }),
        }
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if matches!(self.peek().kind, TokenKind::Newline) {
            self.bump();
            // An empty body is allowed — the lexer doesn't emit Indent
            // for comment-only or no-content body lines, so just bail
            // out with no statements.
            if !matches!(self.peek().kind, TokenKind::Indent) {
                return Ok(Vec::new());
            }
            self.bump();
            let mut stmts = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek().kind, TokenKind::Dedent | TokenKind::Eof) {
                    break;
                }
                stmts.push(self.parse_stmt()?);
            }
            if matches!(self.peek().kind, TokenKind::Dedent) {
                self.bump();
            }
            Ok(stmts)
        } else {
            Ok(vec![self.parse_stmt()?])
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
        // Optional type annotation `: type` per docs/06 §3.2.
        // v0.1 non-strict mode parses but ignores types (docs/02 §5.2.1).
        if matches!(self.peek().kind, TokenKind::Colon) {
            self.bump();
            self.parse_type()?;
        }
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

    /// Parses a type expression and discards it. Non-strict mode does no
    /// type checking; strict / verified modes (v0.2+) will replace this
    /// with a real type tree on the AST.
    fn parse_type(&mut self) -> Result<(), ParseError> {
        // Minimal type grammar covering the v0.1 examples:
        //   type := identifier ("." identifier)*    # qualified names like Hero, vector
        //         | "list" "of" type
        //         | "map" "of" type "=>" type
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Ident(_) => {
                self.bump();
                while matches!(self.peek().kind, TokenKind::Dot) {
                    self.bump();
                    self.expect_ident("expected qualified type segment after '.'")?;
                }
                Ok(())
            }
            other => Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: format!("expected type, got {other:?}"),
                help: Some(
                    "v0.1 type annotations accept identifiers like `int`, `Hero`, or `vector`"
                        .to_string(),
                ),
            }),
        }
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
        let left = self.parse_range()?;
        // `in` / `not in` are lower-precedence than the comparisons but
        // share the chain-rejection rule.
        if matches!(self.peek().kind, TokenKind::In) {
            let tok = self.bump().clone();
            let right = self.parse_range()?;
            return Ok(Expr::Binary {
                op: BinOp::In,
                left: Box::new(left),
                right: Box::new(right),
                line: tok.line,
                col: tok.col,
            });
        }
        if matches!(self.peek().kind, TokenKind::Not)
            && self.pos + 1 < self.tokens.len()
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::In)
        {
            let tok = self.bump().clone(); // not
            self.bump(); // in
            let right = self.parse_range()?;
            return Ok(Expr::Binary {
                op: BinOp::NotIn,
                left: Box::new(left),
                right: Box::new(right),
                line: tok.line,
                col: tok.col,
            });
        }
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
        let right = self.parse_range()?;
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

    fn parse_range(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        let exclusive = match self.peek().kind {
            TokenKind::DotDot => false,
            TokenKind::DotDotLt => true,
            _ => return Ok(left),
        };
        let tok = self.bump().clone();
        let right = self.parse_add()?;
        Ok(Expr::Range {
            start: Box::new(left),
            end: Box::new(right),
            exclusive,
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
                TokenKind::LBracket => {
                    let lb = self.bump().clone();
                    let index = self.parse_expr()?;
                    self.expect(TokenKind::RBracket, "expected ']' to close index")?;
                    left = Expr::Index {
                        object: Box::new(left),
                        index: Box::new(index),
                        line: lb.line,
                        col: lb.col,
                    };
                }
                TokenKind::LParen => {
                    let lp = self.bump().clone();
                    let (args, kwargs) = self.parse_call_args(lp.line, lp.col)?;
                    self.expect(TokenKind::RParen, "expected ')' to close call")?;
                    left = Expr::Call {
                        callee: Box::new(left),
                        args,
                        kwargs,
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

    /// Parse the contents of `(...)` at a call site. Supports a mix of
    /// positional args followed by keyword args (`name: value`). Empty
    /// arg list returns two empty vecs. Positional-after-keyword is a
    /// hard error — same rule as Python.
    #[allow(clippy::type_complexity)]
    fn parse_call_args(
        &mut self,
        call_line: u32,
        call_col: u32,
    ) -> Result<(Vec<Expr>, Vec<(String, Expr)>), ParseError> {
        let mut args = Vec::new();
        let mut kwargs: Vec<(String, Expr)> = Vec::new();
        if matches!(self.peek().kind, TokenKind::RParen) {
            return Ok((args, kwargs));
        }
        loop {
            // Lookahead: `Ident :` (and not `Ident :: ...`) means keyword arg.
            // Anything else is parsed as a positional expression.
            let is_kw = matches!(self.peek().kind, TokenKind::Ident(_))
                && self.pos + 1 < self.tokens.len()
                && matches!(self.tokens[self.pos + 1].kind, TokenKind::Colon);
            if is_kw {
                let name_tok = self.bump().clone();
                let name = match name_tok.kind {
                    TokenKind::Ident(s) => s,
                    _ => unreachable!(),
                };
                self.bump(); // ':'
                let value = self.parse_expr()?;
                if kwargs.iter().any(|(n, _)| n == &name) {
                    return Err(ParseError {
                        line: name_tok.line,
                        col: name_tok.col,
                        message: format!("duplicate keyword argument `{name}:`"),
                        help: Some(
                            "each keyword argument may appear at most once".to_string(),
                        ),
                    });
                }
                kwargs.push((name, value));
            } else {
                if !kwargs.is_empty() {
                    let tok = self.peek().clone();
                    return Err(ParseError {
                        line: tok.line,
                        col: tok.col,
                        message: "positional argument cannot follow keyword arguments"
                            .to_string(),
                        help: Some(
                            "put all positional args before any `name: value` args, \
                             same as Python"
                                .to_string(),
                        ),
                    });
                }
                args.push(self.parse_expr()?);
            }
            if matches!(self.peek().kind, TokenKind::Comma) {
                self.bump();
                if matches!(self.peek().kind, TokenKind::RParen) {
                    // Trailing comma is allowed.
                    break;
                }
                continue;
            }
            break;
        }
        let _ = (call_line, call_col);
        Ok((args, kwargs))
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.bump().clone();
        match tok.kind {
            TokenKind::Str(value) => Ok(Expr::Str {
                value,
                line: tok.line,
                col: tok.col,
            }),
            TokenKind::InterpStr { parts, exprs } => {
                let mut parsed_exprs = Vec::with_capacity(exprs.len());
                for src in &exprs {
                    parsed_exprs.push(parse_embedded_expr(src, tok.line, tok.col)?);
                }
                Ok(Expr::Interp {
                    parts,
                    exprs: parsed_exprs,
                    line: tok.line,
                    col: tok.col,
                })
            }
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
            TokenKind::PercentLit(value) => Ok(Expr::Percent {
                value,
                line: tok.line,
                col: tok.col,
            }),
            TokenKind::UnitLit { value, unit } => Ok(Expr::Quantity {
                value,
                unit,
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
            TokenKind::KwSelf => Ok(Expr::SelfRef {
                line: tok.line,
                col: tok.col,
            }),
            TokenKind::LParen => self.parse_paren_or_tuple(tok.line, tok.col),
            TokenKind::LBracket => self.parse_list(tok.line, tok.col),
            other => Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: format!("expected expression, got {other:?}"),
                help: None,
            }),
        }
    }

    fn parse_list(&mut self, lb_line: u32, lb_col: u32) -> Result<Expr, ParseError> {
        let mut elems = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RBracket) {
            elems.push(self.parse_expr()?);
            while matches!(self.peek().kind, TokenKind::Comma) {
                self.bump();
                if matches!(self.peek().kind, TokenKind::RBracket) {
                    break;
                }
                elems.push(self.parse_expr()?);
            }
        }
        self.expect(TokenKind::RBracket, "expected ']' to close list literal")?;
        Ok(Expr::List {
            elems,
            line: lb_line,
            col: lb_col,
        })
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

fn parse_embedded_expr(src: &str, line: u32, col: u32) -> Result<Expr, ParseError> {
    let tokens = crate::lexer::lex(src).map_err(|e| ParseError {
        line,
        col,
        message: format!("error in interpolation: {}", e.message),
        help: e.help,
    })?;
    let mut sub = Parser::new(&tokens);
    let expr = sub.parse_expr()?;
    Ok(shift_expr(expr, line, col))
}

/// Move every line/col on an expression tree to (line, col), since the
/// embedded source has its own coordinate system.
fn shift_expr(expr: Expr, line: u32, col: u32) -> Expr {
    match expr {
        Expr::Str { value, .. } => Expr::Str { value, line, col },
        Expr::Interp { parts, exprs, .. } => Expr::Interp { parts, exprs, line, col },
        Expr::Int { value, .. } => Expr::Int { value, line, col },
        Expr::Float { value, .. } => Expr::Float { value, line, col },
        Expr::Bool { value, .. } => Expr::Bool { value, line, col },
        Expr::Percent { value, .. } => Expr::Percent { value, line, col },
        Expr::Quantity { value, unit, .. } => Expr::Quantity { value, unit, line, col },
        Expr::Ident { name, .. } => Expr::Ident { name, line, col },
        Expr::SelfRef { .. } => Expr::SelfRef { line, col },
        Expr::Tuple { elems, .. } => Expr::Tuple {
            elems: elems.into_iter().map(|e| shift_expr(e, line, col)).collect(),
            line,
            col,
        },
        Expr::List { elems, .. } => Expr::List {
            elems: elems.into_iter().map(|e| shift_expr(e, line, col)).collect(),
            line,
            col,
        },
        Expr::Range { start, end, exclusive, .. } => Expr::Range {
            start: Box::new(shift_expr(*start, line, col)),
            end: Box::new(shift_expr(*end, line, col)),
            exclusive,
            line,
            col,
        },
        Expr::Index { object, index, .. } => Expr::Index {
            object: Box::new(shift_expr(*object, line, col)),
            index: Box::new(shift_expr(*index, line, col)),
            line,
            col,
        },
        Expr::Field { object, name, .. } => Expr::Field {
            object: Box::new(shift_expr(*object, line, col)),
            name,
            line,
            col,
        },
        Expr::Call { callee, args, kwargs, .. } => Expr::Call {
            callee: Box::new(shift_expr(*callee, line, col)),
            args: args.into_iter().map(|e| shift_expr(e, line, col)).collect(),
            kwargs: kwargs
                .into_iter()
                .map(|(n, e)| (n, shift_expr(e, line, col)))
                .collect(),
            line,
            col,
        },
        Expr::Unary { op, operand, .. } => Expr::Unary {
            op,
            operand: Box::new(shift_expr(*operand, line, col)),
            line,
            col,
        },
        Expr::Binary { op, left, right, .. } => Expr::Binary {
            op,
            left: Box::new(shift_expr(*left, line, col)),
            right: Box::new(shift_expr(*right, line, col)),
            line,
            col,
        },
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
