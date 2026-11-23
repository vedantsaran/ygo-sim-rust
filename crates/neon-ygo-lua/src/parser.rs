use thiserror::Error;
use crate::ast::*;
use crate::lexer::{tokenize, LexError, Token, TokenKind};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ParseError {
    #[error("lex error: {0}")]
    Lex(#[from] LexError),
    #[error("parse error at line {line}: {msg}")]
    Syntax { line: usize, msg: String },
}

pub fn parse(src: &str) -> Result<Vec<Stmt>, ParseError> {
    let tokens = tokenize(src)?;
    let mut p = Parser { tokens, pos: 0 };
    let stmts = p.parse_block(&[TokenKind::Eof])?;
    Ok(stmts)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or_else(|| self.tokens.last().unwrap())
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn line(&self) -> usize {
        self.peek().line
    }

    fn advance(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(ParseError::Syntax {
                line: self.line(),
                msg: format!("expected {:?}, found {:?}", kind, self.peek_kind()),
            })
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.peek_kind().clone() {
            TokenKind::Ident(s) => {
                self.advance();
                Ok(s)
            }
            other => Err(ParseError::Syntax {
                line: self.line(),
                msg: format!("expected identifier, found {:?}", other),
            }),
        }
    }

    fn parse_block(&mut self, terminators: &[TokenKind]) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while !terminators.contains(self.peek_kind()) && !self.check(&TokenKind::Eof) {
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            if self.check(&TokenKind::Return) {
                stmts.push(self.parse_return()?);
                self.eat(&TokenKind::Semi);
                break;
            }
            let mut s = self.parse_stmt()?;
            stmts.append(&mut s);
        }
        Ok(stmts)
    }

    fn parse_return(&mut self) -> Result<Stmt, ParseError> {
        self.expect(&TokenKind::Return)?;
        let mut vals = Vec::new();
        if !matches!(
            self.peek_kind(),
            TokenKind::End | TokenKind::Else | TokenKind::ElseIf | TokenKind::Until | TokenKind::Semi | TokenKind::Eof
        ) {
            vals = self.parse_expr_list()?;
        }
        Ok(Stmt::Return(vals))
    }

    fn parse_stmt(&mut self) -> Result<Vec<Stmt>, ParseError> {
        match self.peek_kind() {
            TokenKind::Local => self.parse_local(),
            TokenKind::Function => Ok(vec![self.parse_func_stmt()?]),
            TokenKind::If => Ok(vec![self.parse_if()?]),
            TokenKind::While => Ok(vec![self.parse_while()?]),
            TokenKind::Repeat => Ok(vec![self.parse_repeat()?]),
            TokenKind::For => Ok(vec![self.parse_for()?]),
            TokenKind::Do => {
                self.advance();
                let body = self.parse_block(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(vec![Stmt::Do(body)])
            }
            TokenKind::Break => {
                self.advance();
                Ok(vec![Stmt::Break])
            }
            TokenKind::ColonColon => {
                // Label `::name::` (skip cleanly)
                self.advance();
                let _ = self.expect_ident()?;
                self.expect(&TokenKind::ColonColon)?;
                Ok(vec![])
            }
            TokenKind::Goto => {
                self.advance();
                let _ = self.expect_ident()?;
                Ok(vec![])
            }
            _ => {
                let first = self.parse_suffixed_expr()?;
                if self.check(&TokenKind::Comma) || self.check(&TokenKind::Eq) {
                    let mut targets = vec![first];
                    while self.eat(&TokenKind::Comma) {
                        targets.push(self.parse_suffixed_expr()?);
                    }
                    self.expect(&TokenKind::Eq)?;
                    let values = self.parse_expr_list()?;
                    Ok(vec![Stmt::Assign { targets, values }])
                } else {
                    Ok(vec![Stmt::ExprStmt(first)])
                }
            }
        }
    }

    fn parse_local(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(&TokenKind::Local)?;
        if self.eat(&TokenKind::Function) {
            let name = self.expect_ident()?;
            let func = self.parse_func_body(false)?;
            // Declare local first, then assign so recursion works!
            return Ok(vec![
                Stmt::Local {
                    names: vec![name.clone()],
                    values: vec![Expr::Nil],
                },
                Stmt::Assign {
                    targets: vec![Expr::Ident(name)],
                    values: vec![func],
                },
            ]);
        }
        let mut names = vec![self.expect_ident()?];
        // Optional Lua 5.4 attribute `<const>` / `<close>`
        if self.eat(&TokenKind::Lt) {
            let _ = self.expect_ident()?;
            self.expect(&TokenKind::Gt)?;
        }
        while self.eat(&TokenKind::Comma) {
            names.push(self.expect_ident()?);
            if self.eat(&TokenKind::Lt) {
                let _ = self.expect_ident()?;
                self.expect(&TokenKind::Gt)?;
            }
        }
        let values = if self.eat(&TokenKind::Eq) {
            self.parse_expr_list()?
        } else {
            Vec::new()
        };
        Ok(vec![Stmt::Local { names, values }])
    }

    fn parse_func_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.expect(&TokenKind::Function)?;
        let base = self.expect_ident()?;
        let mut target = Expr::Ident(base);
        while self.eat(&TokenKind::Dot) {
            let field = self.expect_ident()?;
            target = Expr::Field {
                target: Box::new(target),
                name: field,
            };
        }
        let is_method = if self.eat(&TokenKind::Colon) {
            let method = self.expect_ident()?;
            target = Expr::Field {
                target: Box::new(target),
                name: method,
            };
            true
        } else {
            false
        };
        let func = self.parse_func_body(is_method)?;
        Ok(Stmt::Assign {
            targets: vec![target],
            values: vec![func],
        })
    }

    fn parse_func_body(&mut self, is_method: bool) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        if is_method {
            params.push("self".to_string());
        }
        let mut vararg = false;
        if !self.check(&TokenKind::RParen) {
            loop {
                if self.eat(&TokenKind::DotDotDot) {
                    vararg = true;
                    break;
                }
                params.push(self.expect_ident()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen)?;
        let body = self.parse_block(&[TokenKind::End])?;
        self.expect(&TokenKind::End)?;
        Ok(Expr::Func { params, vararg, body })
    }

    fn parse_if(&mut self) -> Result<Stmt, ParseError> {
        self.expect(&TokenKind::If)?;
        let mut branches = Vec::new();
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::Then)?;
        let body = self.parse_block(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End])?;
        branches.push((cond, body));

        while self.eat(&TokenKind::ElseIf) {
            let c = self.parse_expr()?;
            self.expect(&TokenKind::Then)?;
            let b = self.parse_block(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End])?;
            branches.push((c, b));
        }

        let else_body = if self.eat(&TokenKind::Else) {
            Some(self.parse_block(&[TokenKind::End])?)
        } else {
            None
        };
        self.expect(&TokenKind::End)?;
        Ok(Stmt::If { branches, else_body })
    }

    fn parse_while(&mut self) -> Result<Stmt, ParseError> {
        self.expect(&TokenKind::While)?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::Do)?;
        let body = self.parse_block(&[TokenKind::End])?;
        self.expect(&TokenKind::End)?;
        Ok(Stmt::While { cond, body })
    }

    fn parse_repeat(&mut self) -> Result<Stmt, ParseError> {
        self.expect(&TokenKind::Repeat)?;
        let body = self.parse_block(&[TokenKind::Until])?;
        self.expect(&TokenKind::Until)?;
        let cond = self.parse_expr()?;
        Ok(Stmt::Repeat { body, cond })
    }

    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        self.expect(&TokenKind::For)?;
        let first = self.expect_ident()?;
        if self.eat(&TokenKind::Eq) {
            let start = self.parse_expr()?;
            self.expect(&TokenKind::Comma)?;
            let stop = self.parse_expr()?;
            let step = if self.eat(&TokenKind::Comma) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::Do)?;
            let body = self.parse_block(&[TokenKind::End])?;
            self.expect(&TokenKind::End)?;
            Ok(Stmt::ForNum { var: first, start, stop, step, body })
        } else {
            let mut vars = vec![first];
            while self.eat(&TokenKind::Comma) {
                vars.push(self.expect_ident()?);
            }
            self.expect(&TokenKind::In)?;
            let iters = self.parse_expr_list()?;
            self.expect(&TokenKind::Do)?;
            let body = self.parse_block(&[TokenKind::End])?;
            self.expect(&TokenKind::End)?;
            Ok(Stmt::ForIn { vars, iters, body })
        }
    }

    fn parse_expr_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut exprs = vec![self.parse_expr()?];
        while self.eat(&TokenKind::Comma) {
            exprs.push(self.parse_expr()?);
        }
        Ok(exprs)
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_binary(1)
    }

    fn binop_prec(kind: &TokenKind) -> Option<(BinOp, u8, u8)> {
        // (op, left_prec, right_prec) — Lua 5.4 exact operator precedence
        match kind {
            TokenKind::Or         => Some((BinOp::Or, 1, 2)),
            TokenKind::And        => Some((BinOp::And, 3, 4)),
            TokenKind::Lt         => Some((BinOp::Lt, 5, 6)),
            TokenKind::Gt         => Some((BinOp::Gt, 5, 6)),
            TokenKind::LtEq       => Some((BinOp::Le, 5, 6)),
            TokenKind::GtEq       => Some((BinOp::Ge, 5, 6)),
            TokenKind::TildeEq    => Some((BinOp::Ne, 5, 6)),
            TokenKind::EqEq       => Some((BinOp::Eq, 5, 6)),
            TokenKind::Pipe       => Some((BinOp::BitOr, 7, 8)),
            TokenKind::Tilde      => Some((BinOp::BitXor, 9, 10)),
            TokenKind::Amp        => Some((BinOp::BitAnd, 11, 12)),
            TokenKind::LtLt       => Some((BinOp::Shl, 13, 14)),
            TokenKind::GtGt       => Some((BinOp::Shr, 13, 14)),
            TokenKind::DotDot     => Some((BinOp::Concat, 16, 15)), // right-associative
            TokenKind::Plus       => Some((BinOp::Add, 17, 18)),
            TokenKind::Minus      => Some((BinOp::Sub, 17, 18)),
            TokenKind::Star       => Some((BinOp::Mul, 19, 20)),
            TokenKind::Slash      => Some((BinOp::Div, 19, 20)),
            TokenKind::SlashSlash => Some((BinOp::FloorDiv, 19, 20)),
            TokenKind::Percent    => Some((BinOp::Mod, 19, 20)),
            TokenKind::Caret      => Some((BinOp::Pow, 23, 22)), // right-associative
            _ => None,
        }
    }

    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        while let Some((op, lprec, rprec)) = Self::binop_prec(self.peek_kind()) {
            if lprec < min_prec {
                break;
            }
            self.advance();
            let right = self.parse_binary(rprec)?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek_kind() {
            TokenKind::Minus => Some(UnOp::Neg),
            TokenKind::Not => Some(UnOp::Not),
            TokenKind::Hash => Some(UnOp::Len),
            TokenKind::Tilde => Some(UnOp::BitNot),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let expr = self.parse_binary(21)?;
            return Ok(Expr::Unary { op, expr: Box::new(expr) });
        }
        self.parse_suffixed_expr()
    }

    fn parse_suffixed_expr(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek_kind() {
                TokenKind::Dot => {
                    self.advance();
                    let name = self.expect_ident()?;
                    expr = Expr::Field {
                        target: Box::new(expr),
                        name,
                    };
                }
                TokenKind::LBracket => {
                    self.advance();
                    let key = self.parse_expr()?;
                    self.expect(&TokenKind::RBracket)?;
                    expr = Expr::Index {
                        target: Box::new(expr),
                        key: Box::new(key),
                    };
                }
                TokenKind::Colon => {
                    self.advance();
                    let method = self.expect_ident()?;
                    let args = self.parse_call_args()?;
                    expr = Expr::MethodCall {
                        target: Box::new(expr),
                        method,
                        args,
                    };
                }
                TokenKind::LParen | TokenKind::LBrace | TokenKind::Str(_) => {
                    let args = self.parse_call_args()?;
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        match self.peek_kind().clone() {
            TokenKind::LParen => {
                self.advance();
                let args = if self.check(&TokenKind::RParen) {
                    Vec::new()
                } else {
                    self.parse_expr_list()?
                };
                self.expect(&TokenKind::RParen)?;
                Ok(args)
            }
            TokenKind::LBrace => Ok(vec![self.parse_table()?]),
            TokenKind::Str(s) => {
                self.advance();
                Ok(vec![Expr::Str(s)])
            }
            other => Err(ParseError::Syntax {
                line: self.line(),
                msg: format!("expected function call arguments, found {:?}", other),
            }),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek_kind().clone() {
            TokenKind::Nil => { self.advance(); Ok(Expr::Nil) }
            TokenKind::True => { self.advance(); Ok(Expr::Bool(true)) }
            TokenKind::False => { self.advance(); Ok(Expr::Bool(false)) }
            TokenKind::Int(v) => { self.advance(); Ok(Expr::Int(v)) }
            TokenKind::Float(v) => { self.advance(); Ok(Expr::Float(v)) }
            TokenKind::Str(s) => { self.advance(); Ok(Expr::Str(s)) }
            TokenKind::DotDotDot => { self.advance(); Ok(Expr::VarArg) }
            TokenKind::Ident(s) => { self.advance(); Ok(Expr::Ident(s)) }
            TokenKind::Function => {
                self.advance();
                self.parse_func_body(false)
            }
            TokenKind::LBrace => self.parse_table(),
            TokenKind::LParen => {
                self.advance();
                let e = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(e)
            }
            other => Err(ParseError::Syntax {
                line: self.line(),
                msg: format!("unexpected token {:?}", other),
            }),
        }
    }

    fn parse_table(&mut self) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            if self.eat(&TokenKind::LBracket) {
                let k = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                self.expect(&TokenKind::Eq)?;
                let v = self.parse_expr()?;
                fields.push(TableField::Indexed(k, v));
            } else if matches!(self.peek_kind(), TokenKind::Ident(_))
                && self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Eq)
            {
                let name = self.expect_ident()?;
                self.expect(&TokenKind::Eq)?;
                let v = self.parse_expr()?;
                fields.push(TableField::Named(name, v));
            } else {
                let v = self.parse_expr()?;
                fields.push(TableField::Array(v));
            }
            if !self.eat(&TokenKind::Comma) && !self.eat(&TokenKind::Semi) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr::Table(fields))
    }
}
