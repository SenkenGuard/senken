//! The spot account as a [`SettlementModel`](senken_sim_core::SettlementModel).

use senken_sim_core::money::{BPS_DIVISOR, rescale};
use senken_sim_core::{FillContext, Settled, SettlementModel};
use senken_trade::{OrderSide, TradeError};

use crate::balances::SpotBook;

/// Which asset a fee comes out of.
///
/// Closed on purpose: an adapter that assumes "base on a buy" would
/// misreport every fill while a native-token discount is on, and that
/// should be a compile error at every site that charges a fee rather than
/// a silently wrong currency on a receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeeAsset {
    /// The asset the trade produces — base on a buy, quote on a sell.
    /// The default on every venue in this family.
    Produced,
    /// A separate asset absorbs the fee instead, at a discount. Binance's
    /// BNB setting, OKX's OKB, Bitget's BGB.
    Discount {
        /// The asset the fee is taken from.
        asset: String,
    },
}

/// A spot exchange account's settlement rule.
#[derive(Debug, Clone)]
pub struct Spot {
    /// The base asset of the pair being traded.
    pub base: String,
    /// The quote asset of the pair being traded.
    pub quote: String,
    /// Fee charged on every fill, in basis points.
    pub fee_bps: i64,
    /// Decimal places the account keeps every asset balance at.
    ///
    /// One scale for every asset, because a balance is compared against
    /// an order's requirement and two scales cannot be compared without a
    /// conversion nobody asked for.
    pub asset_scale: u8,
    /// Which asset the fee comes out of.
    pub fee_asset: FeeAsset,
}

impl Spot {
    /// The asset a fee is actually charged in for `side`.
    #[must_use]
    pub fn fee_currency(&self, side: OrderSide) -> &str {
        match &self.fee_asset {
            FeeAsset::Discount { asset } => asset,
            FeeAsset::Produced => match side {
                OrderSide::Buy => &self.base,
                OrderSide::Sell => &self.quote,
            },
        }
    }
}

impl SettlementModel for Spot {
    type Book = SpotBook;

    /// A buy moves quote into base; a sell moves base into quote.
    ///
    /// No position is opened either way, and nothing is realised: the
    /// account holds different assets, which is not the same event as
    /// closing an exposure.
    fn settle(&self, book: &mut Self::Book, fill: &FillContext<'_>) -> Result<Settled, TradeError> {
        // At the account's own asset scale, not the kernel's cash scale:
        // these numbers are quantities of an asset, and a balance the
        // order is checked against is held in those same units.
        let base_amount = rescale(
            i128::from(fill.quantity.value),
            fill.quantity.scale,
            self.asset_scale,
        )?;
        let quote_amount = rescale(
            i128::from(fill.price.value) * i128::from(fill.quantity.value),
            fill.price.scale.saturating_add(fill.quantity.scale),
            self.asset_scale,
        )?;

        // The fee comes out of what the trade produces, so it is charged
        // in base units on a buy and quote units on a sell — unless a
        // discount asset absorbs it, in which case neither leg is touched
        // by it.
        let produced = match fill.side {
            OrderSide::Buy => base_amount,
            OrderSide::Sell => quote_amount,
        };
        let fee = i64::try_from(i128::from(produced) * i128::from(self.fee_bps) / BPS_DIVISOR)
            .map_err(|_| TradeError::InvalidRequest("the fee does not fit".to_owned()))?;

        match fill.side {
            OrderSide::Buy => {
                book.debit(&self.quote, quote_amount)?;
                book.credit(&self.base, base_amount);
            }
            OrderSide::Sell => {
                book.debit(&self.base, base_amount)?;
                book.credit(&self.quote, quote_amount);
            }
        }
        match &self.fee_asset {
            FeeAsset::Produced => {
                let asset = match fill.side {
                    OrderSide::Buy => &self.base,
                    OrderSide::Sell => &self.quote,
                };
                book.debit(asset, fee)?;
            }
            FeeAsset::Discount { asset } => book.debit(asset, fee)?,
        }

        Ok(Settled {
            fill_price: fill.price,
            fee,
            // Spot realises nothing on a fill. A position that never
            // existed cannot be closed, and reporting a realised profit
            // here would be inventing an event.
            realized: 0,
        })
    }

    // `risk`, `enforce` and `accrue` are deliberately left at their
    // defaults. A spot account borrows nothing, so it has no margin level
    // to measure, nothing a venue would close on its behalf, and no cost
    // for the passage of time. Writing empty implementations of those
    // would be the same as claiming it has them.
}
