//! The futures account as a [`SettlementModel`](senken_sim_core::SettlementModel).

use std::collections::BTreeMap;

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_sim_core::risk::{ForcedClose, RiskBreach, RiskState};
use senken_sim_core::{FillContext, Marks, Settled, SettlementModel};
use senken_trade::{MarginMode, OrderSide, PositionSide, TradeError};

use crate::bracket::{BracketTable, Liquidation};
use crate::funding::{FundingTerms, funding_for, intervals_crossed};

/// One open perpetual position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerpPosition {
    /// The instrument.
    pub instrument: InstrumentId,
    /// Long or short. Both can be open at once in hedge mode.
    pub side: PositionSide,
    /// Size held, in contracts.
    pub quantity: Scaled,
    /// Volume-weighted entry.
    pub entry: Scaled,
    /// Margin posted against it, at [`CASH_SCALE`].
    pub margin: i64,
    /// Whether that margin is this position's own or the account's.
    pub margin_mode: MarginMode,
    /// Funding paid or received so far, at [`CASH_SCALE`].
    pub funding: i64,
    /// When it opened.
    pub opened_at: UnixNanos,
}

/// A perpetual futures book.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FuturesBook {
    /// Wallet balance, at [`CASH_SCALE`].
    pub wallet: i64,
    /// Open positions, keyed by instrument and side so hedge mode can hold
    /// both at once and one-way mode simply never holds two.
    ///
    /// Keyed by a string rather than by `PositionSide` because ordering a
    /// domain enum is not this plugin's to decide — a settlement model
    /// works within the shared vocabulary rather than widening it.
    pub positions: BTreeMap<String, PerpPosition>,
}

/// Whether the account holds one position per symbol or one per side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionMode {
    /// One position per symbol; an opposite fill reduces or reverses it.
    OneWay,
    /// A long and a short on one symbol coexist.
    Hedge,
}

/// A crypto perpetual futures account's settlement rules.
#[derive(Debug, Clone)]
pub struct Futures {
    /// One-way or hedge.
    pub position_mode: PositionMode,
    /// Isolated or cross.
    pub margin_mode: MarginMode,
    /// Leverage applied when posting margin.
    pub leverage: i64,
    /// Taker fee, in basis points of notional.
    pub fee_bps: i64,
    /// The symbol's bracket table. Empty means no liquidation price is
    /// reported at all, which is the honest answer when the venue's own
    /// table has not been supplied.
    pub brackets: BracketTable,
    /// The symbol's funding configuration.
    pub funding: FundingTerms,
}

impl Futures {
    /// Notional of `quantity` at `price`, at [`CASH_SCALE`].
    fn notional(quantity: Scaled, price: Scaled) -> Result<i64, TradeError> {
        rescale(
            i128::from(quantity.value) * i128::from(price.value),
            quantity.scale.saturating_add(price.scale),
            CASH_SCALE,
        )
    }

    /// Where the venue would close `position`, when a bracket table says
    /// what its maintenance requirement is.
    ///
    /// `None` when no table has been supplied. A trader shown a
    /// liquidation price believes it, so an unknown one is reported as
    /// unknown rather than estimated.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    pub fn liquidation_price(
        &self,
        position: &PerpPosition,
    ) -> Result<Option<Liquidation>, TradeError> {
        let notional = Self::notional(position.quantity, position.entry)?;
        let Some(tier) = self.brackets.tier_for(notional) else {
            return Ok(None);
        };
        let quantity = i128::from(position.quantity.value);
        if quantity == 0 {
            return Ok(None);
        }
        let entry = i128::from(position.entry.value);
        let margin = i128::from(position.margin);
        let cum = i128::from(tier.maintenance_amount);
        let mmr = i128::from(tier.maintenance_bps);

        // P_liq = [s·Q·E ∓ (M + cum)] / [Q·(s − MMR)], with the rate in
        // basis points and the margin folded back to the entry's scale.
        let scale_factor = 10_i128.pow(u32::from(CASH_SCALE.saturating_sub(position.entry.scale)));
        let margin_at_entry_scale = (margin + cum) / scale_factor.max(1);
        // The basis-point divisor is folded into the numerator rather than
        // applied to the denominator first: dividing `Q × (10000 − mmr)`
        // by 10 000 before the outer division truncates a small quantity
        // straight to zero, and the whole price with it.
        let numerator = 10_000
            * match position.side {
                PositionSide::Long => quantity * entry - margin_at_entry_scale,
                PositionSide::Short => quantity * entry + margin_at_entry_scale,
            };
        let denominator = match position.side {
            PositionSide::Long => quantity * (10_000 - mmr),
            PositionSide::Short => quantity * (10_000 + mmr),
        };
        if denominator == 0 {
            return Ok(None);
        }
        let price = numerator / denominator;
        i64::try_from(price).map_or(Ok(None), |price| {
            Ok(Some(Liquidation {
                price: Scaled::new(position.entry.scale, price),
                // Derived from the venue's equity-versus-maintenance
                // identity, not transcribed from its published expression,
                // and the label travels with the number.
                derived: true,
            }))
        })
    }

    fn key(&self, instrument: &InstrumentId, side: PositionSide) -> String {
        match self.position_mode {
            // One-way holds a single position per symbol, so both sides
            // land on the same key and an opposite fill meets it.
            PositionMode::OneWay => instrument.to_string(),
            PositionMode::Hedge => format!(
                "{instrument}:{}",
                match side {
                    PositionSide::Long => "long",
                    PositionSide::Short => "short",
                }
            ),
        }
    }
}

impl SettlementModel for Futures {
    type Book = FuturesBook;

    fn settle(&self, book: &mut Self::Book, fill: &FillContext<'_>) -> Result<Settled, TradeError> {
        let notional = Self::notional(fill.quantity, fill.price)?;
        let fee = i64::try_from(i128::from(notional) * i128::from(self.fee_bps) / 10_000)
            .map_err(|_| TradeError::InvalidRequest("the fee does not fit".to_owned()))?;
        let margin = notional / self.leverage.max(1);
        let opening = match fill.side {
            OrderSide::Buy => PositionSide::Long,
            OrderSide::Sell => PositionSide::Short,
        };
        let key = self.key(fill.instrument, opening);

        let Some(existing) = book.positions.get(&key).cloned() else {
            book.positions.insert(
                key,
                PerpPosition {
                    instrument: fill.instrument.clone(),
                    side: opening,
                    quantity: fill.quantity,
                    entry: fill.price,
                    margin,
                    margin_mode: self.margin_mode,
                    funding: 0,
                    opened_at: fill.now,
                },
            );
            book.wallet = book.wallet.saturating_sub(fee);
            return Ok(Settled {
                fill_price: fill.price,
                fee,
                realized: 0,
            });
        };

        if existing.side == opening {
            let total = existing.quantity.value.saturating_add(fill.quantity.value);
            let entry = if total == 0 {
                existing.entry
            } else {
                Scaled::new(
                    existing.entry.scale,
                    i64::try_from(
                        (i128::from(existing.entry.value) * i128::from(existing.quantity.value)
                            + i128::from(fill.price.value) * i128::from(fill.quantity.value))
                            / i128::from(total),
                    )
                    .unwrap_or(existing.entry.value),
                )
            };
            book.positions.insert(
                key,
                PerpPosition {
                    quantity: Scaled::new(existing.quantity.scale, total),
                    entry,
                    margin: existing.margin.saturating_add(margin),
                    ..existing
                },
            );
            book.wallet = book.wallet.saturating_sub(fee);
            return Ok(Settled {
                fill_price: fill.price,
                fee,
                realized: 0,
            });
        }

        // Opposite: reduce, close or reverse, exactly as a one-way book
        // does. In hedge mode this branch is unreachable, because the two
        // sides have different keys and never meet.
        let closed = fill.quantity.value.min(existing.quantity.value);
        let realized = close_pnl(
            existing.side,
            existing.entry,
            fill.price,
            Scaled::new(fill.quantity.scale, closed),
        )?;
        let remaining = fill.quantity.value - closed;
        if remaining > 0 {
            book.positions.insert(
                key,
                PerpPosition {
                    side: opening,
                    quantity: Scaled::new(fill.quantity.scale, remaining),
                    entry: fill.price,
                    margin,
                    opened_at: fill.now,
                    ..existing
                },
            );
        } else if existing.quantity.value == closed {
            book.positions.remove(&key);
        } else {
            let left = existing.quantity.value - closed;
            book.positions.insert(
                key,
                PerpPosition {
                    quantity: Scaled::new(existing.quantity.scale, left),
                    ..existing
                },
            );
        }
        book.wallet = book.wallet.saturating_add(realized).saturating_sub(fee);
        Ok(Settled {
            fill_price: fill.price,
            fee,
            realized,
        })
    }

    fn risk(&self, book: &Self::Book, marks: &Marks) -> Result<RiskState, TradeError> {
        let mut unrealized = 0_i64;
        let mut maintenance = 0_i64;
        let mut margin_used = 0_i64;
        for position in book.positions.values() {
            margin_used = margin_used.saturating_add(position.margin);
            unrealized = unrealized.saturating_add(position.funding);
            let Some(mark) = marks.get(&position.instrument.to_string()).copied() else {
                continue;
            };
            unrealized = unrealized.saturating_add(close_pnl(
                position.side,
                position.entry,
                mark,
                position.quantity,
            )?);
            let notional = Self::notional(position.quantity, mark)?;
            if let Some(required) = self.brackets.maintenance_margin(notional) {
                maintenance = maintenance.saturating_add(required);
            }
        }
        let equity = book.wallet.saturating_add(unrealized);
        let margin_level = (margin_used != 0).then(|| {
            Scaled::new(
                2,
                i64::try_from(i128::from(equity) * 10_000 / i128::from(margin_used)).unwrap_or(0),
            )
        });
        Ok(RiskState {
            balance: book.wallet,
            equity,
            margin_used,
            margin_level,
            // The trigger both venues state the same way: equity below
            // maintenance margin. Not a margin-level percentage, and not a
            // margin call — a futures venue has no step between "fine" and
            // "closed".
            breach: (maintenance > 0 && equity < maintenance).then_some(RiskBreach::ForcedClosure),
        })
    }

    fn enforce(
        &self,
        book: &mut Self::Book,
        marks: &Marks,
        now: UnixNanos,
    ) -> Result<Vec<ForcedClose>, TradeError> {
        let mut closed = Vec::new();
        loop {
            let risk = self.risk(book, marks)?;
            if risk.breach != Some(RiskBreach::ForcedClosure) {
                break;
            }
            // The deepest loser goes first, then the account is measured
            // again — a liquidation that took everything at once would
            // close positions the first close had already rescued.
            let worst = book
                .positions
                .iter()
                .filter_map(|(key, position)| {
                    let mark = marks.get(&position.instrument.to_string()).copied()?;
                    let pnl =
                        close_pnl(position.side, position.entry, mark, position.quantity).ok()?;
                    Some((key.clone(), pnl, mark))
                })
                .min_by_key(|(_, pnl, _)| *pnl);
            let Some((key, realized, mark)) = worst else {
                break;
            };
            let Some(position) = book.positions.remove(&key) else {
                break;
            };
            book.wallet = book
                .wallet
                .saturating_add(realized)
                .saturating_add(position.funding);
            closed.push(ForcedClose {
                position: key,
                price: mark,
                realized,
                reason: RiskBreach::ForcedClosure,
            });
        }
        let _ = now;
        Ok(closed)
    }

    fn accrue(
        &self,
        book: &mut Self::Book,
        marks: &Marks,
        from: UnixNanos,
        to: UnixNanos,
    ) -> Result<i64, TradeError> {
        let intervals = intervals_crossed(self.funding, from, to);
        if intervals == 0 {
            return Ok(0);
        }
        let mut total = 0_i64;
        for position in book.positions.values_mut() {
            let price = marks
                .get(&position.instrument.to_string())
                .copied()
                .unwrap_or(position.entry);
            let notional = Self::notional(position.quantity, price)?;
            let amount = funding_for(self.funding, position.side, notional, intervals)?;
            position.funding = position.funding.saturating_add(amount);
            total = total.saturating_add(amount);
        }
        // Funding settles straight against the balance: no order, no fill,
        // no fee.
        book.wallet = book.wallet.saturating_add(total);
        Ok(total)
    }
}

fn close_pnl(
    side: PositionSide,
    entry: Scaled,
    exit: Scaled,
    quantity: Scaled,
) -> Result<i64, TradeError> {
    let Some(exit) = exit.rescale(entry.scale) else {
        return Err(TradeError::InvalidRequest(
            "the entry and exit prices cannot be compared at one scale".to_owned(),
        ));
    };
    let difference = i128::from(exit.value) - i128::from(entry.value);
    let directed = match side {
        PositionSide::Long => difference,
        PositionSide::Short => -difference,
    };
    rescale(
        directed * i128::from(quantity.value),
        entry.scale.saturating_add(quantity.scale),
        CASH_SCALE,
    )
}
