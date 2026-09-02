//! The leverage bracket table, and why it is data rather than code.
//!
//! Maintenance margin is not one rate: it steps up with position notional,
//! from a per-symbol table the venue publishes and changes without notice.
//! Writing one from memory of a venue's documentation is exactly what this
//! project forbids, so the table is **supplied** — an account that has not
//! been given one gets no liquidation price at all rather than a plausible
//! invention.

use senken_core::decimal::Scaled;

/// One tier of a symbol's leverage bracket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bracket {
    /// The largest position notional this tier covers.
    pub notional_cap: i64,
    /// Maintenance margin rate, in basis points of notional.
    pub maintenance_bps: i64,
    /// The maintenance amount this tier deducts, which is what makes the
    /// stepped table continuous at each boundary.
    pub maintenance_amount: i64,
    /// The most leverage this tier allows.
    pub max_leverage: i64,
}

/// A symbol's whole bracket table, in ascending notional order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BracketTable {
    /// The tiers, smallest cap first.
    pub tiers: Vec<Bracket>,
}

impl BracketTable {
    /// The tier covering `notional`.
    ///
    /// `None` when no table has been supplied, or when the notional is
    /// past the largest tier — both of which mean this simulator does not
    /// know the maintenance requirement and must say so rather than pick
    /// the nearest tier and hope.
    #[must_use]
    pub fn tier_for(&self, notional: i64) -> Option<Bracket> {
        self.tiers
            .iter()
            .find(|tier| notional <= tier.notional_cap)
            .copied()
    }

    /// Maintenance margin required for `notional`.
    ///
    /// `Position Notional × Maintenance Margin Rate − Maintenance Amount`,
    /// which is the venue's own shape.
    #[must_use]
    pub fn maintenance_margin(&self, notional: i64) -> Option<i64> {
        let tier = self.tier_for(notional)?;
        let raw = i128::from(notional) * i128::from(tier.maintenance_bps) / 10_000;
        i64::try_from(raw)
            .ok()
            .map(|required| required.saturating_sub(tier.maintenance_amount))
    }
}

/// A liquidation price, and how much to trust it.
///
/// The formula this crate carries is **derived** from the venue's own
/// equity-versus-maintenance-margin identity, not transcribed from its
/// published expression — that expression is published as an image the
/// research pass could not read. So a price computed here is labelled as
/// derived, and the label travels with the number rather than being left
/// in a comment nobody reading a receipt will see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liquidation {
    /// The price itself.
    pub price: Scaled,
    /// `true` while the formula behind it is a derivation awaiting
    /// confirmation against a live venue.
    pub derived: bool,
}
