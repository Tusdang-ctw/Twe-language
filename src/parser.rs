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
            TokenKind::Let | TokenKind::Var | TokenKind::Actor => return self.parse_let(),
            TokenKind::If => return self.parse_if(),
            TokenKind::On => return self.parse_on(),
            TokenKind::Entity => return self.parse_decl(DeclKind::Entity),
            TokenKind::Item => return self.parse_decl(DeclKind::Item),
            TokenKind::Modifier => return self.parse_decl(DeclKind::Modifier),
            TokenKind::Inventory => return self.parse_decl(DeclKind::Inventory),
            TokenKind::Scene => return self.parse_decl(DeclKind::Scene),
            TokenKind::Particles => return self.parse_decl(DeclKind::Particles),
            TokenKind::Visual => return self.parse_decl(DeclKind::Visual),
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
            TokenKind::Wait => return self.parse_wait(),
            TokenKind::Dialogue => return self.parse_dialogue_decl(),
            TokenKind::Say => return self.parse_say(),
            TokenKind::Choice => return self.parse_choice(),
            TokenKind::Import => return self.parse_import(),
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
                    "the left side of `=` must be a name or a field like `obj.field`".to_string(),
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
                    help: Some(
                        "e.g. `on update(dt):`, `on render():`, or `on Enemy.death(e):`"
                            .to_string(),
                    ),
                })
            }
        };
        // Phase 9 session 7b: `on <ClassName>.<event>(<param>):` —
        // class-event handler. Currently only `death` is supported.
        // The first ident is the class name; the next token must be
        // `.` to disambiguate from the existing `on update(dt):` /
        // `on render():` shapes.
        if matches!(self.peek().kind, TokenKind::Dot) {
            self.bump();
            let event_kind_tok = self.bump().clone();
            let event_kind = match event_kind_tok.kind {
                TokenKind::Ident(s) => s,
                other => {
                    return Err(ParseError {
                        line: event_kind_tok.line,
                        col: event_kind_tok.col,
                        message: format!("expected event name after `on {event_name}.`, got {other:?}"),
                        help: Some("v0.3 supports `on <Class>.death(param):`".to_string()),
                    });
                }
            };
            if event_kind != "death" {
                return Err(ParseError {
                    line: event_kind_tok.line,
                    col: event_kind_tok.col,
                    message: format!(
                        "unknown class event `{event_kind}` on `{event_name}`"
                    ),
                    help: Some(
                        "v0.3 supports `death`; `spawn` / `collide` / etc. ride a follow-on session"
                            .to_string(),
                    ),
                });
            }
            self.expect(
                TokenKind::LParen,
                "expected '(' after `on <Class>.death`",
            )?;
            let param_tok = self.bump().clone();
            let param = match param_tok.kind {
                TokenKind::Ident(s) => s,
                other => {
                    return Err(ParseError {
                        line: param_tok.line,
                        col: param_tok.col,
                        message: format!("expected parameter name, got {other:?}"),
                        help: Some(
                            "`on Enemy.death(e):` binds e as the dying entity".to_string(),
                        ),
                    });
                }
            };
            self.expect(TokenKind::RParen, "expected ')' to close parameter list")?;
            self.expect(
                TokenKind::Colon,
                "expected ':' after `on <Class>.death(...)`",
            )?;
            let body = self.parse_block()?;
            return Ok(Stmt::OnClassEvent {
                class: event_name,
                event: event_kind,
                param,
                body,
                line: kw.line,
                col: kw.col,
            });
        }
        match event_name.as_str() {
            "update" => {
                self.expect(TokenKind::LParen, "expected '(' after `on update`")?;
                let param_tok = self.bump().clone();
                let param = match param_tok.kind {
                    TokenKind::Ident(s) => s,
                    other => {
                        return Err(ParseError {
                            line: param_tok.line,
                            col: param_tok.col,
                            message: format!("expected parameter name, got {other:?}"),
                            help: Some(
                                "`on update(dt):` binds dt as the frame delta".to_string(),
                            ),
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
            "render" => {
                // Top-level `on render():` — fires per rendered frame in
                // `twec play3d`. No params (no dt — render is "compose
                // the next frame," not a tick).
                self.expect(TokenKind::LParen, "expected '(' after `on render`")?;
                self.expect(TokenKind::RParen, "expected ')' after `on render(`")?;
                self.expect(TokenKind::Colon, "expected ':' after `on render()`")?;
                let body = self.parse_block()?;
                Ok(Stmt::OnRender {
                    body,
                    line: kw.line,
                    col: kw.col,
                })
            }
            other => Err(ParseError {
                line: event_tok.line,
                col: event_tok.col,
                message: format!(
                    "top-level handler `on {other}` is not supported in v0.1"
                ),
                help: Some(
                    "v0.1 supports `on update(dt):` and `on render():` at the top level; predicate / named events live inside states."
                        .to_string(),
                ),
            }),
        }
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

    /// `import "<path>" [as <alias>]` — Phase 13 session 1 grammar:
    ///   import_stmt := "import" string_literal ("as" identifier)?
    /// The path is a string literal so module names need not be valid
    /// identifiers (forward-slash subpaths, hyphens, etc. all work).
    /// `as` is matched contextually as `Ident("as")` to avoid burning
    /// a reserved keyword the rest of the language never needs.
    fn parse_import(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let path_tok = self.bump().clone();
        let path = match path_tok.kind {
            TokenKind::Str(s) => s,
            other => {
                return Err(ParseError {
                    line: path_tok.line,
                    col: path_tok.col,
                    message: format!("expected string literal after `import`, got {other:?}"),
                    help: Some("`import \"math/vec2\"` brings the module into scope".to_string()),
                });
            }
        };
        let alias = if let TokenKind::Ident(name) = &self.peek().kind {
            if name == "as" {
                self.bump();
                Some(self.expect_ident("expected identifier after `as`")?)
            } else {
                None
            }
        } else {
            None
        };
        self.expect_stmt_end()?;
        Ok(Stmt::Import {
            path,
            alias,
            line: kw.line,
            col: kw.col,
        })
    }

    /// `wait <duration>`. The duration expression must evaluate to a
    /// quantity with a time unit (`s`, `ms`, etc.) — checked at
    /// runtime, not at parse time. Mirrors the `every <duration>:`
    /// rule for that reason. Phase 5 task 2.
    fn parse_wait(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let duration = self.parse_expr()?;
        self.expect_stmt_end()?;
        Ok(Stmt::Wait {
            duration,
            line: kw.line,
            col: kw.col,
        })
    }

    /// `dialogue <Name>:` followed by an indented body of statements.
    /// Phase 5 task 3. The body can include any statement plus the
    /// dialogue-only forms (`say`, `choice`, `wait`).
    fn parse_dialogue_decl(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        let name = self.expect_ident("expected dialogue name after `dialogue`")?;
        self.expect(TokenKind::Colon, "expected ':' after dialogue name")?;
        let body = self.parse_block()?;
        Ok(Stmt::DialogueDecl {
            name,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    /// `say [<actor> :] <text-expr>`. The actor form is detected by
    /// peeking past an expression for a colon — to avoid backtracking
    /// pain we use a simpler heuristic: if the next token after `say`
    /// is an identifier and the token after that is a colon, treat
    /// it as `say <actor>: <text>`. Otherwise it's `say <text>`.
    fn parse_say(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        // Parse the first expression, then peek: if a colon follows,
        // it was the actor and the next expression is the text;
        // otherwise the first expression *is* the text. Works for
        // any actor expression (identifier `merchant`, string
        // literal `"Merchant"`, field access `scene.npc("...")`,
        // etc.) without relying on a token-shape heuristic.
        let first = self.parse_expr()?;
        if matches!(self.peek().kind, TokenKind::Colon) {
            self.bump();
            let text = self.parse_expr()?;
            self.expect_stmt_end()?;
            Ok(Stmt::Say {
                actor: Some(first),
                text,
                line: kw.line,
                col: kw.col,
            })
        } else {
            self.expect_stmt_end()?;
            Ok(Stmt::Say {
                actor: None,
                text: first,
                line: kw.line,
                col: kw.col,
            })
        }
    }

    /// `choice:` followed by an indented list of branches. Each
    /// branch is `<label-expr>:` then an indented body. Phase 5
    /// task 3. Labels are typically string literals but we accept
    /// any expression for forward compatibility (e.g., a condition
    /// could gate a branch in a future version).
    fn parse_choice(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump().clone();
        self.expect(TokenKind::Colon, "expected ':' after `choice`")?;
        // The body of a `choice:` is an indented list of branches.
        // Match parse_block's framing: skip the trailing Newline,
        // expect Indent, walk branches until Dedent.
        if matches!(self.peek().kind, TokenKind::Newline) {
            self.bump();
        }
        if !matches!(self.peek().kind, TokenKind::Indent) {
            return Err(ParseError {
                line: kw.line,
                col: kw.col,
                message: "`choice:` requires at least one indented branch".to_string(),
                help: Some("indent under `choice:` and add `\"<label>\":` branches".to_string()),
            });
        }
        self.bump();
        let mut branches = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek().kind, TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            let label = self.parse_expr()?;
            self.expect(TokenKind::Colon, "expected ':' after choice label")?;
            let body = self.parse_block()?;
            branches.push((label, body));
        }
        if matches!(self.peek().kind, TokenKind::Dedent) {
            self.bump();
        }
        if branches.is_empty() {
            return Err(ParseError {
                line: kw.line,
                col: kw.col,
                message: "`choice:` requires at least one branch".to_string(),
                help: Some(
                    "add at least one `\"<label>\":` branch with an indented body".to_string(),
                ),
            });
        }
        Ok(Stmt::Choice {
            branches,
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
        let mut params: Vec<crate::ast::Param> = Vec::new();
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
        let mut ret: Option<crate::types::Type> = None;
        if matches!(self.peek().kind, TokenKind::Minus) {
            // peek ahead one — we only consume both if the second is `>`.
            if self.pos + 1 < self.tokens.len()
                && matches!(self.tokens[self.pos + 1].kind, TokenKind::Gt)
            {
                self.bump();
                self.bump();
                ret = self.parse_type()?;
            }
        }
        self.expect(TokenKind::Colon, "expected ':' after function signature")?;
        let body = self.parse_block()?;
        Ok(Stmt::FunctionDecl {
            name,
            params,
            ret,
            body,
            line: kw.line,
            col: kw.col,
        })
    }

    fn parse_param(&mut self) -> Result<crate::ast::Param, ParseError> {
        let name = self.expect_ident("expected parameter name")?;
        let mut ty: Option<crate::types::Type> = None;
        if matches!(self.peek().kind, TokenKind::Colon) {
            self.bump();
            ty = self.parse_type()?;
        }
        Ok(crate::ast::Param { name, ty })
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
                    message: format!("expected name after `{}`, got {other:?}", kind.as_str()),
                    help: Some(
                        "declarative block names use PascalCase, e.g. `item Sword:`".to_string(),
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
            let mut field_ty: Option<crate::types::Type> = None;
            if matches!(self.peek().kind, TokenKind::Colon) {
                self.bump();
                field_ty = self.parse_type()?;
            }
            self.expect(TokenKind::Eq, "expected '=' in field declaration")?;
            let value = self.parse_expr()?;
            self.expect_stmt_end()?;
            return Ok(DeclMember::Field {
                name,
                value,
                ty: field_ty,
                line: name_tok.line,
                col: name_tok.col,
            });
        }
        match self.peek().kind {
            TokenKind::LParen => {
                self.bump();
                let mut params: Vec<crate::ast::Param> = Vec::new();
                if !matches!(self.peek().kind, TokenKind::RParen) {
                    params.push(self.parse_param()?);
                    while matches!(self.peek().kind, TokenKind::Comma) {
                        self.bump();
                        params.push(self.parse_param()?);
                    }
                }
                self.expect(TokenKind::RParen, "expected ')' after parameter list")?;
                // Phase 6 session 4: keep the return-type annotation
                // on the AST so strict mode can enforce it. Same
                // shape as `parse_function`'s `-> type` parsing.
                let mut ret: Option<crate::types::Type> = None;
                if matches!(self.peek().kind, TokenKind::Minus)
                    && self.pos + 1 < self.tokens.len()
                    && matches!(self.tokens[self.pos + 1].kind, TokenKind::Gt)
                {
                    self.bump();
                    self.bump();
                    ret = self.parse_type()?;
                }
                self.expect(TokenKind::Colon, "expected ':' after parameter list")?;
                let body = self.parse_block()?;
                Ok(DeclMember::Method {
                    name,
                    params,
                    ret,
                    body,
                    line: name_tok.line,
                    col: name_tok.col,
                })
            }
            TokenKind::Colon => {
                self.bump();
                // Optional type annotation `field: type = value` per docs/06 §3.3.
                // Phase 6 session 4: keep the parsed type on the AST
                // so strict mode can enforce it.
                let mut field_ty: Option<crate::types::Type> = None;
                if self.looks_like_typed_field() {
                    field_ty = self.parse_type()?;
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
                    ty: field_ty,
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
        // `on update(dt):` — state-scoped frame handler. Closes F5.
        if matches!(self.peek().kind, TokenKind::On)
            && self.pos + 1 < self.tokens.len()
            && matches!(&self.tokens[self.pos + 1].kind, TokenKind::Ident(s) if s == "update")
        {
            let kw = self.bump().clone();
            self.bump(); // ident "update"
            self.expect(TokenKind::LParen, "expected '(' after `on update`")?;
            let param =
                self.expect_ident("expected param name (usually `dt`) after `on update(`")?;
            self.expect(TokenKind::RParen, "expected ')' after `on update(<param>`")?;
            self.expect(TokenKind::Colon, "expected ':' after `on update(<param>)`")?;
            let body = self.parse_block()?;
            return Ok(StateMember::OnUpdate {
                param,
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
        // `on <predicate>: body` — predicate event handler. Falls
        // through to here after the three name-shaped on-handlers
        // above. The predicate is any expression; the runtime
        // evaluates it each frame and fires the body on a
        // false→true transition. Phase 5 task 4 (Example 4 surface).
        if matches!(self.peek().kind, TokenKind::On) {
            let kw = self.bump().clone();
            let predicate = self.parse_expr()?;
            self.expect(TokenKind::Colon, "expected ':' after `on <predicate>`")?;
            let body = self.parse_block()?;
            return Ok(StateMember::OnPredicate {
                predicate,
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
        // Phase 6 session 2: kept on the AST so strict mode can
        // unify the value's inferred type against it. Non-strict
        // still ignores.
        let mut ty: Option<crate::types::Type> = None;
        if matches!(self.peek().kind, TokenKind::Colon) {
            self.bump();
            ty = self.parse_type()?;
        }
        self.expect(TokenKind::Eq, "expected '=' after `let <name>`")?;
        let value = self.parse_expr()?;
        self.expect_stmt_end()?;
        Ok(Stmt::Let {
            name,
            value,
            ty,
            line: kw.line,
            col: kw.col,
        })
    }

    /// Parse a type expression. Returns `Some(Type)` for primitive
    /// names that the inferer recognises (`int`, `float`, `bool`,
    /// `string`, `nil`), `None` for everything else (user class
    /// names, qualified identifiers like `vector.x`, or types we
    /// don't yet model). Non-strict mode ignores annotations
    /// either way; strict mode (Phase 6 session 2) only enforces
    /// when a recognised primitive was annotated, so unmapped
    /// names degrade gracefully — they parse, they just don't
    /// constrain.
    fn parse_type(&mut self) -> Result<Option<crate::types::Type>, ParseError> {
        // Minimal type grammar covering the v0.1 examples:
        //   type := identifier ("." identifier)*
        //         | "list" "of" type
        //         | "map" "of" type "=>" type
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Ident(name) => {
                self.bump();
                let mut qualified = false;
                while matches!(self.peek().kind, TokenKind::Dot) {
                    self.bump();
                    self.expect_ident("expected qualified type segment after '.'")?;
                    qualified = true;
                }
                if qualified {
                    return Ok(None);
                }
                Ok(primitive_name_to_type(&name))
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
                    // After `.` the token is unambiguously a field
                    // name — no possible parse conflict with the
                    // surrounding statement grammar — so accept the
                    // source spelling of any keyword too. Without
                    // this, adding a new keyword would silently
                    // break existing programs that use it as a
                    // method name (e.g., `random.choice(list)`
                    // after `choice` became a Phase 5 keyword).
                    let name = match name_tok.kind {
                        TokenKind::Ident(s) => s,
                        ref other => match keyword_spelling(other) {
                            Some(s) => s.to_string(),
                            None => {
                                return Err(ParseError {
                                    line: name_tok.line,
                                    col: name_tok.col,
                                    message: format!(
                                        "expected field name after '.', got {other:?}"
                                    ),
                                    help: Some(
                                        "field names must be identifiers or keywords".to_string(),
                                    ),
                                });
                            }
                        },
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
                        help: Some("each keyword argument may appear at most once".to_string()),
                    });
                }
                kwargs.push((name, value));
            } else {
                if !kwargs.is_empty() {
                    let tok = self.peek().clone();
                    return Err(ParseError {
                        line: tok.line,
                        col: tok.col,
                        message: "positional argument cannot follow keyword arguments".to_string(),
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
            // Single-line ternary `if cond: a [elif d: e]* else: b`. The
            // statement-form `if cond:` followed by an indented block is
            // parsed by `parse_if` from `parse_stmt`; this branch only
            // fires when an `if` appears in expression position (RHS of
            // `let`, function arg, return value, etc.). The else-arm is
            // mandatory — expressions can't have a missing branch.
            TokenKind::If => self.parse_if_expr(tok.line, tok.col),
            other => Err(ParseError {
                line: tok.line,
                col: tok.col,
                message: format!("expected expression, got {other:?}"),
                help: None,
            }),
        }
    }

    fn parse_if_expr(&mut self, line: u32, col: u32) -> Result<Expr, ParseError> {
        let cond = Box::new(self.parse_expr()?);
        self.expect(
            TokenKind::Colon,
            "expected ':' after `if` condition in expression",
        )?;
        let then_expr = Box::new(self.parse_expr()?);
        let mut elifs = Vec::new();
        while matches!(self.peek().kind, TokenKind::Elif) {
            self.bump();
            let elif_cond = self.parse_expr()?;
            self.expect(
                TokenKind::Colon,
                "expected ':' after `elif` condition in expression",
            )?;
            let elif_expr = self.parse_expr()?;
            elifs.push((elif_cond, elif_expr));
        }
        if !matches!(self.peek().kind, TokenKind::Else) {
            let here = self.peek();
            return Err(ParseError {
                line: here.line,
                col: here.col,
                message: "expected `else:` to complete `if`-expression".to_string(),
                help: Some(
                    "expression-form `if` requires both branches; \
                     write `if cond: a else: b` (or use a statement-form `if` for blocks)"
                        .to_string(),
                ),
            });
        }
        self.bump();
        self.expect(
            TokenKind::Colon,
            "expected ':' after `else` in if-expression",
        )?;
        let else_expr = Box::new(self.parse_expr()?);
        Ok(Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            line,
            col,
        })
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
                    "use `nil` for an absent value, or put an expression inside".to_string(),
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
                    help: Some("twe ends each statement at a newline; no semicolons".to_string()),
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
        Expr::Interp { parts, exprs, .. } => Expr::Interp {
            parts,
            exprs,
            line,
            col,
        },
        Expr::Int { value, .. } => Expr::Int { value, line, col },
        Expr::Float { value, .. } => Expr::Float { value, line, col },
        Expr::Bool { value, .. } => Expr::Bool { value, line, col },
        Expr::Percent { value, .. } => Expr::Percent { value, line, col },
        Expr::Quantity { value, unit, .. } => Expr::Quantity {
            value,
            unit,
            line,
            col,
        },
        Expr::Ident { name, .. } => Expr::Ident { name, line, col },
        Expr::SelfRef { .. } => Expr::SelfRef { line, col },
        Expr::Tuple { elems, .. } => Expr::Tuple {
            elems: elems
                .into_iter()
                .map(|e| shift_expr(e, line, col))
                .collect(),
            line,
            col,
        },
        Expr::List { elems, .. } => Expr::List {
            elems: elems
                .into_iter()
                .map(|e| shift_expr(e, line, col))
                .collect(),
            line,
            col,
        },
        Expr::Range {
            start,
            end,
            exclusive,
            ..
        } => Expr::Range {
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
        Expr::Call {
            callee,
            args,
            kwargs,
            ..
        } => Expr::Call {
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
        Expr::Binary {
            op, left, right, ..
        } => Expr::Binary {
            op,
            left: Box::new(shift_expr(*left, line, col)),
            right: Box::new(shift_expr(*right, line, col)),
            line,
            col,
        },
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            ..
        } => Expr::IfExpr {
            cond: Box::new(shift_expr(*cond, line, col)),
            then_expr: Box::new(shift_expr(*then_expr, line, col)),
            elifs: elifs
                .into_iter()
                .map(|(c, e)| (shift_expr(c, line, col), shift_expr(e, line, col)))
                .collect(),
            else_expr: Box::new(shift_expr(*else_expr, line, col)),
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

/// Map a primitive type name to the corresponding `Type` variant.
/// Returns `None` for names we don't model — user class names,
/// qualified types, generic forms — so strict mode degrades
/// gracefully (no enforcement, but no spurious error either).
/// Phase 6 session 2.
fn primitive_name_to_type(name: &str) -> Option<crate::types::Type> {
    use crate::types::Type;
    match name {
        "int" => Some(Type::Int),
        "float" => Some(Type::Float),
        "bool" => Some(Type::Bool),
        "string" | "str" => Some(Type::Str),
        "nil" => Some(Type::Nil),
        "range" => Some(Type::Range),
        // Class names, `Hero`, `vector`, etc. don't map to a
        // built-in primitive. The inferer's class-shape registry
        // (built from `entity` / `item` / `scene` decls) handles
        // those when the value-side type appears as
        // `Type::Instance(name)`.
        _ => None,
    }
}

/// Return the source spelling of a keyword `TokenKind`, or `None`
/// for non-keyword tokens. Used by field access (`obj.<name>`) so
/// keywords can also serve as field/method names — necessary for
/// stdlib calls like `random.choice(list)` not to break when a
/// new statement keyword (`choice`, `wait`, …) gets added.
fn keyword_spelling(t: &TokenKind) -> Option<&'static str> {
    Some(match t {
        TokenKind::Let => "let",
        TokenKind::Var => "var",
        TokenKind::On => "on",
        TokenKind::If => "if",
        TokenKind::Elif => "elif",
        TokenKind::Else => "else",
        TokenKind::And => "and",
        TokenKind::Or => "or",
        TokenKind::Not => "not",
        TokenKind::Entity => "entity",
        TokenKind::Item => "item",
        TokenKind::Modifier => "modifier",
        TokenKind::Inventory => "inventory",
        TokenKind::Scene => "scene",
        TokenKind::Particles => "particles",
        TokenKind::State => "state",
        TokenKind::Every => "every",
        TokenKind::Extends => "extends",
        TokenKind::KwSelf => "self",
        TokenKind::Function => "function",
        TokenKind::Return => "return",
        TokenKind::While => "while",
        TokenKind::For => "for",
        TokenKind::In => "in",
        TokenKind::Break => "break",
        TokenKind::Continue => "continue",
        TokenKind::Spawn => "spawn",
        TokenKind::Despawn => "despawn",
        TokenKind::Wait => "wait",
        TokenKind::Dialogue => "dialogue",
        TokenKind::Say => "say",
        TokenKind::Choice => "choice",
        TokenKind::Actor => "actor",
        _ => return None,
    })
}
