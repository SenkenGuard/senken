//! The parsed shape of an indicator-lang program — no types, no slots, no
//! stack layout, none of which exist until [`crate::typeck`] and
//! [`crate::codegen`] run. Every node carries the line/column its first
//! token started at, so a type error can point at it precisely.

/// A full program: zero or more `let` bindings and exactly one `plot`
/// statement, in source order.
///
/// Exactly one `plot` (rather than any number) is this MVP's own choice,
/// not something the language's stated constraints force — see this
/// crate's top-level report for why: it keeps `on-bar`'s exported shape
/// (and therefore `wit/senken.wit`'s `compiled-indicator` world) fixed and single-valued instead of
/// varying per compiled program, and every one of the ten built-ins can
/// still be exercised and proven equivalent to its `senken_indicators`
/// counterpart through it.
#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) statements: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub(crate) enum Statement {
    Let(LetStatement),
    Plot(PlotStatement),
}

#[derive(Debug, Clone)]
pub(crate) struct LetStatement {
    pub(crate) name: String,
    pub(crate) name_line: u32,
    pub(crate) name_column: u32,
    pub(crate) value: Expr,
}

#[derive(Debug, Clone)]
pub(crate) struct PlotStatement {
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) value: Expr,
}

/// One expression, with its own start position for error reporting.
#[derive(Debug, Clone)]
pub(crate) struct Expr {
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) kind: ExprKind,
}

#[derive(Debug, Clone)]
pub(crate) enum ExprKind {
    /// A numeric literal, kept as text until [`crate::typeck`] parses it —
    /// the same text can be a plain number in an expression or a
    /// compile-time-only built-in argument.
    Number(String),
    /// A bar field (`open`/`high`/`low`/`close`/`volume`) or a name bound
    /// by an earlier `let`.
    Name(String),
    /// A built-in call, e.g. `ema(close, 20)`.
    ///
    /// Every argument parses as a full expression — the grammar does not
    /// distinguish a `Series` position from a `Period` one. `crate::typeck`
    /// is what requires a `Period`/`Number` argument to be literally a
    /// numeric constant, because that is a semantic constraint (this
    /// built-in's state is constructed once, at compile time) rather than
    /// a syntactic one.
    Call {
        name: String,
        name_line: u32,
        name_column: u32,
        args: Vec<Expr>,
    },
    /// `.field` immediately following a call that reports more than one
    /// value, e.g. the `.histogram` in `macd(12, 26, 9).histogram`.
    Field {
        base: Box<Expr>,
        field: String,
        field_line: u32,
        field_column: u32,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}
