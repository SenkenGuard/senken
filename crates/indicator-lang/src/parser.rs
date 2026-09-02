//! A small recursive-descent parser from [`Token`]s to a [`Program`].
//!
//! The grammar, top to bottom:
//!
//! ```text
//! program    := (statement NEWLINE+)*
//! statement  := "let" IDENT "=" expr
//!             | "plot" expr
//! expr       := term (("+" | "-") term)*
//! term       := unary (("*" | "/") unary)*
//! unary      := "-" unary | postfix
//! postfix    := primary ("." IDENT)?
//! primary    := NUMBER | IDENT ("(" (expr ("," expr)*)? ")")? | "(" expr ")"
//! ```
//!
//! `postfix` allows at most one `.field` — this language has no nested
//! projection because no built-in call returns something with a field of
//! its own to project again. Whether a given `.field` is valid at all
//! (only after a built-in that reports more than one value, only naming
//! one of *that* built-in's own fields) is a semantic question `crate::typeck`
//! answers, not this grammar.

use crate::CompileError;
use crate::ast::{
    BinaryOp, Expr, ExprKind, LetStatement, PlotStatement, Program, Statement, UnaryOp,
};
use crate::lexer::{Token, TokenKind};

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

pub(crate) fn parse(tokens: Vec<Token>) -> Result<Program, CompileError> {
    let mut parser = Parser { tokens, pos: 0 };
    parser.program()
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek().kind, TokenKind::Newline) {
            self.advance();
        }
    }

    fn error(&self, message: impl Into<String>) -> CompileError {
        let token = self.peek();
        CompileError::Syntax {
            line: token.line,
            column: token.column,
            message: message.into(),
        }
    }

    fn expect(&mut self, kind: &TokenKind, what: &str) -> Result<Token, CompileError> {
        if std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind) {
            Ok(self.advance())
        } else {
            Err(self.error(format!(
                "expected {what}, found {}",
                describe(&self.peek().kind)
            )))
        }
    }

    fn program(&mut self) -> Result<Program, CompileError> {
        let mut statements = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek().kind, TokenKind::Eof) {
            statements.push(self.statement()?);
            if !matches!(self.peek().kind, TokenKind::Newline | TokenKind::Eof) {
                return Err(self.error(format!(
                    "expected the line to end here, found {}",
                    describe(&self.peek().kind)
                )));
            }
            self.skip_newlines();
        }
        Ok(Program { statements })
    }

    fn statement(&mut self) -> Result<Statement, CompileError> {
        match &self.peek().kind {
            TokenKind::Let => {
                self.advance();
                let name_token =
                    self.expect(&TokenKind::Ident(String::new()), "a name for this `let`")?;
                let TokenKind::Ident(name) = name_token.kind else {
                    unreachable!("expect() already checked the token kind");
                };
                self.expect(&TokenKind::Eq, "`=` after the `let` name")?;
                let value = self.expr()?;
                Ok(Statement::Let(LetStatement {
                    name,
                    name_line: name_token.line,
                    name_column: name_token.column,
                    value,
                }))
            }
            TokenKind::Plot => {
                let plot_token = self.advance();
                let value = self.expr()?;
                Ok(Statement::Plot(PlotStatement {
                    line: plot_token.line,
                    column: plot_token.column,
                    value,
                }))
            }
            other => Err(self.error(format!(
                "expected `let` or `plot` to start a line, found {}",
                describe(other)
            ))),
        }
    }

    fn expr(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            let op_token = self.advance();
            let right = self.term()?;
            left = Expr {
                line: op_token.line,
                column: op_token.column,
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                _ => break,
            };
            let op_token = self.advance();
            let right = self.unary()?;
            left = Expr {
                line: op_token.line,
                column: op_token.column,
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, CompileError> {
        if let TokenKind::Minus = self.peek().kind {
            let token = self.advance();
            let operand = self.unary()?;
            return Ok(Expr {
                line: token.line,
                column: token.column,
                kind: ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                },
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, CompileError> {
        let mut expr = self.primary()?;
        if matches!(self.peek().kind, TokenKind::Dot) {
            self.advance();
            let field_token =
                self.expect(&TokenKind::Ident(String::new()), "a field name after `.`")?;
            let TokenKind::Ident(field) = field_token.kind else {
                unreachable!("expect() already checked the token kind");
            };
            expr = Expr {
                line: expr.line,
                column: expr.column,
                kind: ExprKind::Field {
                    base: Box::new(expr),
                    field,
                    field_line: field_token.line,
                    field_column: field_token.column,
                },
            };
        }
        Ok(expr)
    }

    fn primary(&mut self) -> Result<Expr, CompileError> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Number(text) => {
                self.advance();
                Ok(Expr {
                    line: token.line,
                    column: token.column,
                    kind: ExprKind::Number(text),
                })
            }
            TokenKind::Ident(name) => {
                self.advance();
                if matches!(self.peek().kind, TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !matches!(self.peek().kind, TokenKind::RParen) {
                        args.push(self.expr()?);
                        while matches!(self.peek().kind, TokenKind::Comma) {
                            self.advance();
                            args.push(self.expr()?);
                        }
                    }
                    self.expect(&TokenKind::RParen, "`)` to close this call")?;
                    Ok(Expr {
                        line: token.line,
                        column: token.column,
                        kind: ExprKind::Call {
                            name,
                            name_line: token.line,
                            name_column: token.column,
                            args,
                        },
                    })
                } else {
                    Ok(Expr {
                        line: token.line,
                        column: token.column,
                        kind: ExprKind::Name(name),
                    })
                }
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.expr()?;
                self.expect(&TokenKind::RParen, "`)` to close this group")?;
                Ok(inner)
            }
            other => Err(self.error(format!(
                "expected a number, a name, or `(`, found {}",
                describe(&other)
            ))),
        }
    }
}

/// Describes a token kind the way a trader reading an error message would,
/// never a lexer-internal term.
fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Let => "`let`".to_string(),
        TokenKind::Plot => "`plot`".to_string(),
        TokenKind::Ident(name) => format!("`{name}`"),
        TokenKind::Number(text) => format!("`{text}`"),
        TokenKind::Plus => "`+`".to_string(),
        TokenKind::Minus => "`-`".to_string(),
        TokenKind::Star => "`*`".to_string(),
        TokenKind::Slash => "`/`".to_string(),
        TokenKind::LParen => "`(`".to_string(),
        TokenKind::RParen => "`)`".to_string(),
        TokenKind::Comma => "`,`".to_string(),
        TokenKind::Dot => "`.`".to_string(),
        TokenKind::Eq => "`=`".to_string(),
        TokenKind::Newline => "the end of the line".to_string(),
        TokenKind::Eof => "the end of the program".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn parse_source(source: &str) -> Program {
        parse(lex(source).unwrap()).unwrap()
    }

    #[test]
    fn parses_a_let_and_plot_program() {
        let program = parse_source("let fast = ema(close, 12)\nplot fast\n");
        assert_eq!(program.statements.len(), 2);
        assert!(matches!(program.statements[0], Statement::Let(_)));
        assert!(matches!(program.statements[1], Statement::Plot(_)));
    }

    #[test]
    fn parses_field_access_after_a_call() {
        let program = parse_source("plot macd(12, 26, 9).histogram\n");
        let Statement::Plot(plot) = &program.statements[0] else {
            panic!("expected a plot statement");
        };
        assert!(matches!(plot.value.kind, ExprKind::Field { .. }));
    }

    #[test]
    fn parses_arithmetic_with_correct_precedence() {
        // `2 + 3 * 4` must parse as `2 + (3 * 4)`, not `(2 + 3) * 4`.
        let program = parse_source("plot 2 + 3 * 4\n");
        let Statement::Plot(plot) = &program.statements[0] else {
            panic!("expected a plot statement");
        };
        let ExprKind::Binary {
            op: BinaryOp::Add,
            right,
            ..
        } = &plot.value.kind
        else {
            panic!("expected the top-level operator to be `+`");
        };
        assert!(matches!(
            right.kind,
            ExprKind::Binary {
                op: BinaryOp::Mul,
                ..
            }
        ));
    }

    #[test]
    fn a_missing_closing_paren_is_a_syntax_error_with_a_position() {
        let err = parse(lex("plot ema(close, 20\n").unwrap()).unwrap_err();
        match err {
            // The missing `)` is discovered at the newline that follows
            // `20`, still on the line the call started on.
            CompileError::Syntax { line, .. } => assert_eq!(line, 1),
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }
}
