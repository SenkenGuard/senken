//! The one thing four simulated trading systems genuinely disagree about.

use std::collections::BTreeMap;

use senken_core::Scaled;
use senken_core::time::UnixNanos;
use senken_trade::{OrderSide, TradeError};

use crate::risk::{ForcedClose, RiskState};

/// The marks a settlement model measures against, keyed by instrument.
///
/// Handed in rather than fetched, because a model must measure every
/// position against **one** set of prices: asking per position mid-loop
/// would let a stop out close the second ticket against a price the first
/// was never judged by.
pub type Marks = BTreeMap<String, Scaled>;

/// What applying one fill did to a book.
///
/// Returned as a single value rather than three out-parameters so a caller
/// cannot use half of it — the fee and the realised amount move cash
/// together or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settled {
    /// The price the fill actually happened at, after slippage.
    pub fill_price: Scaled,
    /// The fee charged, at the book's own cash scale.
    pub fee: i64,
    /// Profit or loss realised by whatever this fill closed, at the book's
    /// own cash scale. Zero when the fill only opened or added.
    pub realized: i64,
}

/// One fill, as the kernel hands it to a settlement model.
#[derive(Debug, Clone)]
pub struct FillContext<'a> {
    /// The instrument being filled.
    pub instrument: &'a senken_marketdata::InstrumentId,
    /// Which way the fill goes.
    pub side: OrderSide,
    /// How much of it filled.
    pub quantity: Scaled,
    /// The price it filled at, slippage already applied.
    pub price: Scaled,
    /// When it happened.
    pub now: senken_core::UnixNanos,
}

/// How one trading system settles a fill into its own book.
///
/// This is the whole difference between the four systems Senken simulates.
/// Everything else a simulator does — taking an order, resting it,
/// triggering it, pricing the fill, charging the fee, recording history —
/// is identical across all four and lives in the kernel, written and
/// tested once.
///
/// A netting book merges into one position per instrument and reverses
/// through zero; a hedging book opens a ticket per deal and never merges;
/// a spot book has no positions at all and moves two asset balances.
/// Those are different data structures, not different settings, which is
/// why this is a trait with an associated book type rather than a flag.
pub trait SettlementModel {
    /// The book this model keeps — a map of positions, a list of tickets,
    /// or a set of asset balances, as the system actually works.
    type Book;

    /// Folds one fill into `book`, returning what it cost and what it
    /// realised.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic overflows or two scales cannot
    /// be reconciled.
    fn settle(&self, book: &mut Self::Book, fill: &FillContext<'_>) -> Result<Settled, TradeError>;

    /// The account's risk right now, as this system measures it.
    ///
    /// Every system measures something; what differs is what. MetaTrader
    /// has one margin level for the whole account, a futures venue has a
    /// liquidation price per isolated position, and a spot account has no
    /// risk state at all because nothing is borrowed — which is why the
    /// default is "no margin, no breach" rather than a required method
    /// spot would have to write an empty answer for.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    fn risk(&self, book: &Self::Book, marks: &Marks) -> Result<RiskState, TradeError> {
        let _ = (book, marks);
        Ok(RiskState {
            balance: 0,
            equity: 0,
            margin_used: 0,
            margin_level: None,
            breach: None,
        })
    }

    /// Closes whatever this system closes when risk is breached.
    ///
    /// MetaTrader's stop out closes the biggest loser and repeats; a
    /// futures venue liquidates the position whose maintenance margin is
    /// gone. A system with nothing to force closes nothing, which is the
    /// default.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    fn enforce(
        &self,
        book: &mut Self::Book,
        marks: &Marks,
        now: UnixNanos,
    ) -> Result<Vec<ForcedClose>, TradeError> {
        let _ = (book, marks, now);
        Ok(Vec::new())
    }

    /// Charges what time itself costs: MetaTrader's swap, a perpetual's
    /// funding.
    ///
    /// Takes a range rather than an instant, so a book that has not been
    /// read for a week accrues the week rather than one night. Returns
    /// what was charged, at the book's own cash scale.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    fn accrue(
        &self,
        book: &mut Self::Book,
        marks: &Marks,
        from: UnixNanos,
        to: UnixNanos,
    ) -> Result<i64, TradeError> {
        let _ = (book, marks, from, to);
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use senken_core::decimal::Scaled;
    use senken_core::time::UnixNanos;
    use senken_marketdata::InstrumentId;
    use senken_trade::OrderSide;

    use super::{FillContext, Settled, SettlementModel, TradeError};

    /// A spot book: asset balances, and not one position anywhere.
    ///
    /// This exists to answer the only question that decides whether the
    /// seam is in the right place — whether a system that has no positions
    /// at all can implement the same trait as one built entirely out of
    /// them. It settles a buy as "more base, less quote", which is what a
    /// spot account actually does.
    #[derive(Debug, Default)]
    struct SpotBalances {
        assets: BTreeMap<String, i64>,
    }

    struct Spot;

    impl SettlementModel for Spot {
        type Book = SpotBalances;

        fn settle(
            &self,
            book: &mut Self::Book,
            fill: &FillContext<'_>,
        ) -> Result<Settled, TradeError> {
            let quantity = fill.quantity.value;
            let quote = quantity.saturating_mul(fill.price.value);
            let (base_delta, quote_delta) = match fill.side {
                OrderSide::Buy => (quantity, -quote),
                OrderSide::Sell => (-quantity, quote),
            };
            *book.assets.entry("BTC".to_owned()).or_default() += base_delta;
            *book.assets.entry("USDT".to_owned()).or_default() += quote_delta;
            Ok(Settled {
                fill_price: fill.price,
                fee: 0,
                // Spot realises nothing on a fill: the account holds
                // different assets, not a closed position.
                realized: 0,
            })
        }
    }

    fn instrument() -> InstrumentId {
        InstrumentId::parse("okx-spot:BTCUSDT").unwrap()
    }

    #[test]
    fn a_system_with_no_positions_at_all_still_implements_the_settlement_seam() {
        let mut book = SpotBalances::default();
        let instrument = instrument();
        let settled = Spot
            .settle(
                &mut book,
                &FillContext {
                    instrument: &instrument,
                    side: OrderSide::Buy,
                    quantity: Scaled::new(0, 2),
                    price: Scaled::new(0, 30_000),
                    now: UnixNanos::from_secs(1_700_000_000).unwrap(),
                },
            )
            .unwrap();

        assert_eq!(book.assets["BTC"], 2, "the buy is held as the base asset");
        assert_eq!(
            book.assets["USDT"], -60_000,
            "and paid for out of the quote one"
        );
        assert_eq!(
            settled.realized, 0,
            "a spot fill closes nothing, so it realises nothing"
        );
    }
}
