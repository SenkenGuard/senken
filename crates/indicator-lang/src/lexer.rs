//! Turns indicator-lang source text into a flat stream of [`Token`]s.
//!
//! There is deliberately very little here: numbers, identifiers, the two
//! statement keywords, arithmetic operators, punctuation, and `//`
//! comments. Every token remembers where it started so a later parse or
//! type error can point at the exact spot a trader wrote, in the words
//! `CompileError` promises — a line and a column, never a byte offset or a
//! token index.

use crate::CompileError;

/// One lexical token, tagged with the one-based line and column its first
/// character sits at.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) line: u32,
    pub(crate) column: u32,
}

/// What a [`Token`] is. Numbers keep their original text rather than a
/// pre-parsed value: whether `20` must be a whole number or may carry a
/// decimal point depends on which built-in argument position it lands in,
/// which only the parser knows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Let,
    Plot,
    Ident(String),
    Number(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
    Dot,
    Eq,
    Newline,
    Eof,
}

/// Lexes `source` in full, or reports the first character that cannot
/// start a token.
pub(crate) fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    let mut lexer = Lexer {
        chars: source.chars().collect(),
        pos: 0,
        line: 1,
        column: 1,
    };
    lexer.run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    column: u32,
}

impl Lexer {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) {
        if self.chars[self.pos] == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        self.pos += 1;
    }

    fn run(&mut self) -> Result<Vec<Token>, CompileError> {
        let mut tokens = Vec::new();
        while let Some(c) = self.peek() {
            if c == '\n' {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    line: self.line,
                    column: self.column,
                });
                self.advance();
                continue;
            }
            if c.is_whitespace() {
                self.advance();
                continue;
            }
            if c == '/' && self.peek_at(1) == Some('/') {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.advance();
                }
                continue;
            }

            let start_line = self.line;
            let start_column = self.column;

            if c.is_ascii_digit()
                || (c == '.' && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()))
            {
                tokens.push(self.lex_number(start_line, start_column));
                continue;
            }
            if c.is_ascii_alphabetic() || c == '_' {
                tokens.push(self.lex_ident_or_keyword(start_line, start_column));
                continue;
            }

            tokens.push(self.lex_punctuation(c, start_line, start_column)?);
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            line: self.line,
            column: self.column,
        });
        Ok(tokens)
    }

    /// A numeric literal: digits, with at most one `.` — and only when
    /// that `.` is itself followed by a digit, so a call's field access
    /// (`macd(12, 26, 9).histogram`) is never mistaken for a decimal
    /// point.
    fn lex_number(&mut self, start_line: u32, start_column: u32) -> Token {
        let mut text = String::new();
        let mut seen_dot = false;
        while let Some(c) = self.peek() {
            if c == '.' {
                if seen_dot || !self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                    break;
                }
                seen_dot = true;
            } else if !c.is_ascii_digit() {
                break;
            }
            text.push(c);
            self.advance();
        }
        Token {
            kind: TokenKind::Number(text),
            line: start_line,
            column: start_column,
        }
    }

    fn lex_ident_or_keyword(&mut self, start_line: u32, start_column: u32) -> Token {
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if !c.is_ascii_alphanumeric() && c != '_' {
                break;
            }
            text.push(c);
            self.advance();
        }
        let kind = match text.as_str() {
            "let" => TokenKind::Let,
            "plot" => TokenKind::Plot,
            _ => TokenKind::Ident(text),
        };
        Token {
            kind,
            line: start_line,
            column: start_column,
        }
    }

    fn lex_punctuation(
        &mut self,
        c: char,
        start_line: u32,
        start_column: u32,
    ) -> Result<Token, CompileError> {
        let kind = match c {
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            '=' => TokenKind::Eq,
            other => {
                return Err(CompileError::Syntax {
                    line: start_line,
                    column: start_column,
                    message: format!("`{other}` is not something this language understands"),
                });
            }
        };
        self.advance();
        Ok(Token {
            kind,
            line: start_line,
            column: start_column,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_a_let_and_plot_program() {
        let tokens = lex("let fast = ema(close, 12)\nplot fast\n").unwrap();
        let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Let,
                &TokenKind::Ident("fast".to_string()),
                &TokenKind::Eq,
                &TokenKind::Ident("ema".to_string()),
                &TokenKind::LParen,
                &TokenKind::Ident("close".to_string()),
                &TokenKind::Comma,
                &TokenKind::Number("12".to_string()),
                &TokenKind::RParen,
                &TokenKind::Newline,
                &TokenKind::Plot,
                &TokenKind::Ident("fast".to_string()),
                &TokenKind::Newline,
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn a_dot_after_a_call_is_field_access_not_a_decimal() {
        let tokens = lex("macd(12,26,9).histogram").unwrap();
        assert!(matches!(tokens[7].kind, TokenKind::RParen));
        assert!(matches!(tokens[8].kind, TokenKind::Dot));
        assert!(matches!(tokens[9].kind, TokenKind::Ident(ref s) if s == "histogram"));
    }

    #[test]
    fn comments_are_skipped_to_end_of_line() {
        let tokens = lex("plot x // this is a comment\n").unwrap();
        let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Plot,
                &TokenKind::Ident("x".to_string()),
                &TokenKind::Newline,
                &TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn an_unknown_character_is_a_syntax_error_with_a_position() {
        let err = lex("plot x = 1 @ 2").unwrap_err();
        match err {
            CompileError::Syntax { line, column, .. } => {
                assert_eq!(line, 1);
                assert_eq!(column, 12);
            }
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }
}
