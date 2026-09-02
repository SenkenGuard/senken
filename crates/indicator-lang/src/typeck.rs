//! Name resolution, type checking, and lowering a [`Program`] into the
//! [`Checked`] program [`crate::codegen::module`] compiles directly.
//!
//! There is exactly one type in this language: a number. The only thing
//! `crate::typeck` calls a second "type" is the transient shape a
//! multi-valued built-in call produces before `.field` narrows it back to
//! a number — it exists only long enough to make `macd(12, 26, 9)` (with
//! no field picked) a clear error instead of a wrong one, and can never be
//! stored in a `let` or cross a call boundary. That is a deliberate choice
//! to keep this MVP small, not a limitation the grammar enforces; see this
//! crate's top-level report.

use std::collections::HashMap;

use crate::CompileError;
use crate::ast::{
    BinaryOp as AstBinaryOp, Expr, ExprKind, Program, Statement, UnaryOp as AstUnaryOp,
};
use crate::builtins::{self, BAR_FIELDS, Builtin, ParamKind, ResultShape};

/// A fully checked program, ready for [`crate::codegen::module`]: every
/// name resolved, every built-in argument validated against its
/// declared kind, and every stateful call assigned the slot that
/// identifies its host-side state.
#[derive(Debug)]
pub(crate) struct Checked {
    /// `let` bindings in source order. Each stores into the local at
    /// `CheckedLet::local`.
    pub(crate) lets: Vec<CheckedLet>,
    /// The program's single `plot` expression.
    pub(crate) plot: CheckedExpr,
    /// How many `let` locals were allocated (`f64`, one each) — the wasm
    /// function's own params (`open`..`volume`) occupy indices `0..=4`,
    /// so `let` locals start at index `5`.
    pub(crate) let_count: u32,
}

#[derive(Debug)]
pub(crate) struct CheckedLet {
    pub(crate) local: u32,
    pub(crate) value: CheckedExpr,
}

/// One built-in call, fully resolved: its host slot, which built-in it
/// calls, and its arguments in the built-in's own declared order.
#[derive(Debug)]
pub(crate) struct CheckedCall {
    pub(crate) slot: u32,
    pub(crate) builtin: &'static Builtin,
    pub(crate) args: Vec<CheckedArg>,
}

#[derive(Debug)]
pub(crate) enum CheckedArg {
    Series(Box<CheckedExpr>),
    /// A compile-time-constant whole number of bars.
    Period(u32),
    /// A compile-time-constant decimal.
    Number(f64),
}

#[derive(Debug)]
pub(crate) enum CheckedExpr {
    BarField(BarField),
    Local(u32),
    Number(f64),
    Unary(CheckedUnaryOp, Box<CheckedExpr>),
    Binary(CheckedBinaryOp, Box<CheckedExpr>, Box<CheckedExpr>),
    /// A built-in call used directly — only valid when the built-in
    /// reports a single value.
    Call(CheckedCall),
    /// A built-in call narrowed to one of its fields by index into
    /// [`ResultShape::Compound`]'s field list.
    Field(CheckedCall, usize),
}

pub(crate) type CheckedUnaryOp = AstUnaryOp;
pub(crate) type CheckedBinaryOp = AstBinaryOp;

/// A bar field, in the order `on-bar` receives them as parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarField {
    Open,
    High,
    Low,
    Close,
    Volume,
}

impl BarField {
    pub(crate) fn param_index(self) -> u32 {
        match self {
            BarField::Open => 0,
            BarField::High => 1,
            BarField::Low => 2,
            BarField::Close => 3,
            BarField::Volume => 4,
        }
    }
}

/// What an expression evaluates to: a number, or — transiently, only until
/// `.field` (or an error) resolves it — the still-unprojected result of a
/// multi-valued built-in.
enum Type {
    Number,
    Compound(&'static [&'static str]),
}

struct Env<'a> {
    locals: HashMap<&'a str, u32>,
    next_local: u32,
    next_slot: u32,
}

pub(crate) fn check(program: &Program) -> Result<Checked, CompileError> {
    let mut env = Env {
        locals: HashMap::new(),
        // `on-bar`'s own params occupy 0..=4.
        next_local: 5,
        next_slot: 0,
    };

    let mut lets = Vec::new();
    let mut plot = None;

    for statement in &program.statements {
        match statement {
            Statement::Let(let_stmt) => {
                if plot.is_some() {
                    return Err(CompileError::Type {
                        line: let_stmt.name_line,
                        column: let_stmt.name_column,
                        message: "`plot` must be the last line of the program".to_string(),
                    });
                }
                let (value, ty) = check_expr(&mut env, &let_stmt.value)?;
                let value = require_number(&let_stmt.value, value, &ty, "a `let` value")?;
                let local = env.next_local;
                env.next_local += 1;
                env.locals.insert(let_stmt.name.as_str(), local);
                lets.push(CheckedLet { local, value });
            }
            Statement::Plot(plot_stmt) => {
                if plot.is_some() {
                    return Err(CompileError::Type {
                        line: plot_stmt.line,
                        column: plot_stmt.column,
                        message: "a program may only have one `plot` line; combine what you \
                                  need into a single expression first"
                            .to_string(),
                    });
                }
                let (value, ty) = check_expr(&mut env, &plot_stmt.value)?;
                let value = require_number(&plot_stmt.value, value, &ty, "a `plot` value")?;
                plot = Some(value);
            }
        }
    }

    let Some(plot) = plot else {
        return Err(CompileError::Type {
            line: 1,
            column: 1,
            message: "a program needs exactly one `plot` line telling the chart what to draw"
                .to_string(),
        });
    };

    Ok(Checked {
        lets,
        plot,
        let_count: env.next_local - 5,
    })
}

fn require_number(
    expr: &Expr,
    checked: CheckedExpr,
    ty: &Type,
    context: &str,
) -> Result<CheckedExpr, CompileError> {
    match ty {
        Type::Number => Ok(checked),
        Type::Compound(fields) => Err(CompileError::Type {
            line: expr.line,
            column: expr.column,
            message: format!(
                "this built-in reports more than one value ({}); {context} must be a single \
                 number, so pick one first, e.g. add `.{}`",
                fields.join(", "),
                fields[0]
            ),
        }),
    }
}

fn check_expr(env: &mut Env<'_>, expr: &Expr) -> Result<(CheckedExpr, Type), CompileError> {
    match &expr.kind {
        ExprKind::Number(text) => {
            let value = parse_f64(text).ok_or_else(|| CompileError::Type {
                line: expr.line,
                column: expr.column,
                message: format!("`{text}` is not a valid number"),
            })?;
            Ok((CheckedExpr::Number(value), Type::Number))
        }
        ExprKind::Name(name) => {
            if let Some(field) = bar_field(name) {
                return Ok((CheckedExpr::BarField(field), Type::Number));
            }
            if let Some(&local) = env.locals.get(name.as_str()) {
                return Ok((CheckedExpr::Local(local), Type::Number));
            }
            Err(CompileError::Type {
                line: expr.line,
                column: expr.column,
                message: format!(
                    "`{name}` is not a bar field ({}) or an earlier `let`",
                    BAR_FIELDS.join(", ")
                ),
            })
        }
        ExprKind::Unary { op, operand } => {
            let (checked, ty) = check_expr(env, operand)?;
            let checked = require_number(operand, checked, &ty, "the operand of `-`")?;
            Ok((CheckedExpr::Unary(*op, Box::new(checked)), Type::Number))
        }
        ExprKind::Binary { op, left, right } => {
            let (left_checked, left_ty) = check_expr(env, left)?;
            let left_checked = require_number(
                left,
                left_checked,
                &left_ty,
                "the left side of this operator",
            )?;
            let (right_checked, right_ty) = check_expr(env, right)?;
            let right_checked = require_number(
                right,
                right_checked,
                &right_ty,
                "the right side of this operator",
            )?;
            Ok((
                CheckedExpr::Binary(*op, Box::new(left_checked), Box::new(right_checked)),
                Type::Number,
            ))
        }
        ExprKind::Call {
            name,
            name_line,
            name_column,
            args,
        } => check_call(env, name, *name_line, *name_column, args),
        ExprKind::Field {
            base,
            field,
            field_line,
            field_column,
        } => check_field(env, base, field, *field_line, *field_column),
    }
}

fn check_call(
    env: &mut Env<'_>,
    name: &str,
    name_line: u32,
    name_column: u32,
    args: &[Expr],
) -> Result<(CheckedExpr, Type), CompileError> {
    let Some(builtin) = builtins::lookup(name) else {
        return Err(CompileError::Type {
            line: name_line,
            column: name_column,
            message: format!(
                "`{name}` is not a built-in indicator; the available ones are: {}",
                builtins::BUILTINS
                    .iter()
                    .map(|b| b.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    };
    if args.len() != builtin.params.len() {
        return Err(CompileError::Type {
            line: name_line,
            column: name_column,
            message: format!(
                "`{name}` takes {} argument{}, found {}",
                builtin.params.len(),
                if builtin.params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        });
    }
    let mut checked_args = Vec::with_capacity(args.len());
    for (arg, param) in args.iter().zip(builtin.params) {
        checked_args.push(check_arg(env, name, arg, *param)?);
    }
    let slot = env.next_slot;
    env.next_slot += 1;
    let call = CheckedCall {
        slot,
        builtin,
        args: checked_args,
    };
    match builtin.result {
        ResultShape::Scalar => Ok((CheckedExpr::Call(call), Type::Number)),
        ResultShape::Compound(fields) => Ok((CheckedExpr::Call(call), Type::Compound(fields))),
    }
}

fn check_field(
    env: &mut Env<'_>,
    base: &Expr,
    field: &str,
    field_line: u32,
    field_column: u32,
) -> Result<(CheckedExpr, Type), CompileError> {
    let (checked, ty) = check_expr(env, base)?;
    let Type::Compound(fields) = ty else {
        return Err(CompileError::Type {
            line: field_line,
            column: field_column,
            message: format!(
                "this built-in reports a single number; it has no `.{field}` to access"
            ),
        });
    };
    let CheckedExpr::Call(call) = checked else {
        unreachable!("only a Call expression can have Type::Compound")
    };
    let Some(index) = fields.iter().position(|f| *f == field) else {
        return Err(CompileError::Type {
            line: field_line,
            column: field_column,
            message: format!(
                "`{field}` is not one of this built-in's values ({})",
                fields.join(", ")
            ),
        });
    };
    Ok((CheckedExpr::Field(call, index), Type::Number))
}

/// Checks one call argument against the parameter kind its position
/// declares. A `Period`/`Number` argument must be a bare numeric literal —
/// the underlying `senken_indicators` type constructs this state once, so
/// there is no incremental "current value" for it the way there is for a
/// `Series` argument.
fn check_arg(
    env: &mut Env<'_>,
    builtin_name: &str,
    arg: &Expr,
    kind: ParamKind,
) -> Result<CheckedArg, CompileError> {
    match kind {
        ParamKind::Series => {
            let (checked, ty) = check_expr(env, arg)?;
            let checked = require_number(arg, checked, &ty, "a series argument")?;
            Ok(CheckedArg::Series(Box::new(checked)))
        }
        ParamKind::Period => {
            let ExprKind::Number(text) = &arg.kind else {
                return Err(CompileError::Type {
                    line: arg.line,
                    column: arg.column,
                    message: format!(
                        "`{builtin_name}`'s period must be a whole number written directly, \
                         like `20`, not a computed value"
                    ),
                });
            };
            let value: u32 = text.parse().map_err(|_| CompileError::Type {
                line: arg.line,
                column: arg.column,
                message: format!(
                    "`{builtin_name}`'s period must be a whole number of bars, found `{text}`"
                ),
            })?;
            if value == 0 {
                return Err(CompileError::Type {
                    line: arg.line,
                    column: arg.column,
                    message: format!("`{builtin_name}`'s period must be at least 1, found `0`"),
                });
            }
            Ok(CheckedArg::Period(value))
        }
        ParamKind::Number => {
            let ExprKind::Number(text) = &arg.kind else {
                return Err(CompileError::Type {
                    line: arg.line,
                    column: arg.column,
                    message: format!(
                        "`{builtin_name}`'s argument must be a number written directly, like \
                         `2.0`, not a computed value"
                    ),
                });
            };
            let value = parse_f64(text).ok_or_else(|| CompileError::Type {
                line: arg.line,
                column: arg.column,
                message: format!("`{text}` is not a valid number"),
            })?;
            Ok(CheckedArg::Number(value))
        }
    }
}

fn bar_field(name: &str) -> Option<BarField> {
    match name {
        "open" => Some(BarField::Open),
        "high" => Some(BarField::High),
        "low" => Some(BarField::Low),
        "close" => Some(BarField::Close),
        "volume" => Some(BarField::Volume),
        _ => None,
    }
}

fn parse_f64(text: &str) -> Option<f64> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn check_source(source: &str) -> Result<Checked, CompileError> {
        check(&parse(lex(source).unwrap()).unwrap())
    }

    #[test]
    fn resolves_bar_fields_and_lets() {
        let checked = check_source("let fast = ema(close, 12)\nplot fast\n").unwrap();
        assert_eq!(checked.lets.len(), 1);
        assert_eq!(checked.let_count, 1);
    }

    #[test]
    fn a_bare_compound_result_is_a_type_error() {
        let err = check_source("plot macd(12, 26, 9)\n").unwrap_err();
        match err {
            CompileError::Type { message, .. } => {
                assert!(message.contains("macd, signal, histogram"), "{message}");
            }
            other => panic!("expected a type error, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_field_lists_the_valid_ones() {
        let err = check_source("plot macd(12, 26, 9).nonsense\n").unwrap_err();
        match err {
            CompileError::Type { message, .. } => {
                assert!(message.contains("macd, signal, histogram"), "{message}");
            }
            other => panic!("expected a type error, got {other:?}"),
        }
    }

    #[test]
    fn a_computed_period_is_rejected() {
        let err = check_source("let p = 20\nplot ema(close, p)\n").unwrap_err();
        match err {
            CompileError::Type { message, .. } => {
                assert!(message.contains("whole number"), "{message}");
            }
            other => panic!("expected a type error, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_name_is_a_type_error() {
        let err = check_source("plot nonsense\n").unwrap_err();
        assert!(matches!(err, CompileError::Type { .. }));
    }

    #[test]
    fn a_missing_plot_is_a_type_error() {
        let err = check_source("let x = close\n").unwrap_err();
        match err {
            CompileError::Type { message, .. } => assert!(message.contains("plot")),
            other => panic!("expected a type error, got {other:?}"),
        }
    }

    #[test]
    fn field_access_on_a_scalar_builtin_is_rejected() {
        let err = check_source("plot ema(close, 20).histogram\n").unwrap_err();
        assert!(matches!(err, CompileError::Type { .. }));
    }
}
