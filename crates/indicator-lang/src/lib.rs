//! The indicator language: lexed, parsed, checked, and compiled to
//! WebAssembly inside the application.
//!
//! See this crate's `README.md` for what the language deliberately cannot
//! express, and why that list is the security model rather than a limitation
//! to be lifted later.

mod ast;
mod builtins;
mod codegen;
mod lexer;
mod parser;
mod typeck;

/// What went wrong turning indicator source into a compiled artifact.
///
/// Every variant carries enough to point at the offending source, because
/// these messages are shown verbatim to someone writing an indicator — not
/// to a compiler engineer reading a log.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CompileError {
    /// The source could not be read as this language at all.
    #[error("line {line}, column {column}: {message}")]
    Syntax {
        /// One-based line the problem starts on.
        line: u32,
        /// One-based column the problem starts on.
        column: u32,
        /// What was wrong, in the words someone writing an indicator uses.
        message: String,
    },
    /// The source parsed, but says something that cannot mean anything.
    #[error("line {line}, column {column}: {message}")]
    Type {
        /// One-based line the problem starts on.
        line: u32,
        /// One-based column the problem starts on.
        column: u32,
        /// What was wrong, in the words someone writing an indicator uses.
        message: String,
    },
    /// Wrapping a correctly checked program into a component failed. This
    /// is never a mistake in the source — `codegen::module::emit` only
    /// ever constructs a core module built to satisfy
    /// `wit/senken.wit`'s `compiled-indicator` world — so it carries no
    /// line or column and indicates a bug in this crate rather than in
    /// anything a trader wrote.
    #[error("internal compiler error: {0}")]
    Internal(String),
}

/// Compiles indicator-lang `source` into a component implementing
/// `wit/senken.wit`'s `compiled-indicator` world.
///
/// # Errors
///
/// Returns [`CompileError::Syntax`] or [`CompileError::Type`] for a mistake
/// in `source`, both naming the exact line and column and describing the
/// problem in the language a trader writing an indicator uses rather than
/// compiler terminology. See this crate's `README.md` for what the
/// language can and cannot express.
pub fn compile(source: &str) -> Result<Vec<u8>, CompileError> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse(tokens)?;
    let checked = typeck::check(&program)?;
    let core_module = codegen::module::emit(&checked);
    codegen::component::wrap_core_module(core_module)
        .map_err(|error| CompileError::Internal(format!("{error:?}")))
}
