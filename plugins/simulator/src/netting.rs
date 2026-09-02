//! The netting rule: one position per instrument, merged and reversed
//! through zero.
//!
//! This is the whole of what makes this adapter a *netting* simulator
//! rather than a hedging one. Everything else it does — taking the order,
//! resting it, pricing the fill, charging the fee, writing the history —
//! is [`senken_sim_core`]'s, shared with every other simulated system.

use std::collections::BTreeMap;

use senken_core::decimal::Scaled;
use senken_marketdata::InstrumentId;
use senken_sim_core::money::{basis_points, notional, weighted_average};
use senken_sim_core::{FillContext, Settled, SettlementModel, Terms};
use senken_trade::{OrderSide, PositionSide, TradeError};

use crate::book::{BookPosition, close_pnl};

/// A netting book: at most one position per instrument.
///
/// Ordered rather than hashed, so two runs of the same sequence of fills
/// report their positions in the same order — a simulator whose output
/// depends on hash iteration is not one a strategy can be compared
/// against itself on.
pub type NettingPositions = BTreeMap<InstrumentId, BookPosition>;

/// Settles fills the way a netting account does.
///
/// Buying while short reduces the short rather than opening a second
/// position, and a buy larger than the short reverses through zero into a
/// long. This is what MetaTrader 5 calls `ACCOUNT_MARGIN_MODE_RETAIL_NETTING`
/// and what every spot-style exchange book does.
#[derive(Debug, Clone, Copy)]
pub struct Netting {
    /// The fee and slippage this account trades under.
    pub terms: Terms,
}

impl SettlementModel for Netting {
    type Book = NettingPositions;

    fn settle(
        &self,
        positions: &mut Self::Book,
        fill: &FillContext<'_>,
    ) -> Result<Settled, TradeError> {
        let fee = basis_points(notional(fill.price, fill.quantity)?, self.terms.fee_bps)?;
        let opening = match fill.side {
            OrderSide::Buy => PositionSide::Long,
            OrderSide::Sell => PositionSide::Short,
        };

        let Some(existing) = positions.get(fill.instrument).cloned() else {
            positions.insert(
                fill.instrument.clone(),
                BookPosition {
                    side: opening,
                    quantity: fill.quantity,
                    average_entry: fill.price,
                    realized: 0,
                    opened_at: fill.now,
                },
            );
            return Ok(Settled {
                fill_price: fill.price,
                fee,
                realized: 0,
            });
        };

        if existing.side == opening {
            let merged = BookPosition {
                side: opening,
                quantity: Scaled::new(
                    existing.quantity.scale,
                    existing.quantity.value.saturating_add(fill.quantity.value),
                ),
                average_entry: weighted_average(
                    existing.average_entry,
                    existing.quantity,
                    fill.price,
                    fill.quantity,
                )?,
                ..existing
            };
            positions.insert(fill.instrument.clone(), merged);
            return Ok(Settled {
                fill_price: fill.price,
                fee,
                realized: 0,
            });
        }

        // Opposing: close what overlaps, then open the remainder the other
        // way.
        let closed = fill.quantity.value.min(existing.quantity.value);
        let realized = close_pnl(
            existing.side,
            existing.average_entry,
            fill.price,
            Scaled::new(fill.quantity.scale, closed),
        )?;
        let remaining = fill.quantity.value - closed;

        if remaining > 0 {
            positions.insert(
                fill.instrument.clone(),
                BookPosition {
                    side: opening,
                    quantity: Scaled::new(fill.quantity.scale, remaining),
                    average_entry: fill.price,
                    realized: existing.realized.saturating_add(realized),
                    opened_at: fill.now,
                },
            );
        } else if existing.quantity.value == closed {
            positions.remove(fill.instrument);
        } else {
            positions.insert(
                fill.instrument.clone(),
                BookPosition {
                    quantity: Scaled::new(
                        existing.quantity.scale,
                        existing.quantity.value - closed,
                    ),
                    realized: existing.realized.saturating_add(realized),
                    ..existing
                },
            );
        }

        Ok(Settled {
            fill_price: fill.price,
            fee,
            realized,
        })
    }
}
