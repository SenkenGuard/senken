//! One balance row per asset, split into what can be spent and what is
//! held against open orders.

use std::collections::BTreeMap;

use senken_trade::TradeError;

/// One asset's balance, at a fixed scale the account keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssetBalance {
    /// What can be spent on a new order.
    pub free: i64,
    /// What is held against a resting order and cannot be spent twice.
    pub locked: i64,
}

impl AssetBalance {
    /// Everything held, spendable or not.
    #[must_use]
    pub fn total(self) -> i64 {
        self.free.saturating_add(self.locked)
    }
}

/// A spot account: balances, and nothing that resembles a position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpotBook {
    /// One row per asset the account has ever touched, ordered so two runs
    /// of the same fills report in the same order.
    pub assets: BTreeMap<String, AssetBalance>,
}

impl SpotBook {
    /// The balance for `asset`, zero when the account has never held it.
    #[must_use]
    pub fn get(&self, asset: &str) -> AssetBalance {
        self.assets.get(asset).copied().unwrap_or_default()
    }

    /// Credits `amount` of `asset` to what can be spent.
    pub fn credit(&mut self, asset: &str, amount: i64) {
        let entry = self.assets.entry(asset.to_owned()).or_default();
        entry.free = entry.free.saturating_add(amount);
    }

    /// Debits `amount` of `asset` from what can be spent.
    ///
    /// # Errors
    /// [`TradeError::InsufficientBalance`] when the account does not hold
    /// it. This is the rule that separates spot from every leveraged
    /// system: you cannot sell what you do not hold, and the answer is a
    /// refusal rather than a short position.
    pub fn debit(&mut self, asset: &str, amount: i64) -> Result<(), TradeError> {
        let entry = self.assets.entry(asset.to_owned()).or_default();
        if entry.free < amount {
            return Err(TradeError::InsufficientBalance(format!(
                "this account holds less {asset} than the order needs"
            )));
        }
        entry.free -= amount;
        Ok(())
    }

    /// Moves `amount` of `asset` from spendable into held.
    ///
    /// A resting order locks exactly what it could consume if it filled
    /// completely, and the lock stays until the order fills or is
    /// cancelled — which is what stops the same balance being spent twice.
    ///
    /// # Errors
    /// [`TradeError::InsufficientBalance`] when there is not enough free.
    pub fn lock(&mut self, asset: &str, amount: i64) -> Result<(), TradeError> {
        self.debit(asset, amount)?;
        let entry = self.assets.entry(asset.to_owned()).or_default();
        entry.locked = entry.locked.saturating_add(amount);
        Ok(())
    }

    /// Moves `amount` of `asset` back from held into spendable.
    ///
    /// Cancelling releases the whole remaining lock; a partial fill
    /// releases only the slice it consumed.
    ///
    /// # Errors
    /// [`TradeError::InvalidRequest`] when more is released than is held,
    /// which would conjure balance out of an accounting mistake.
    pub fn release(&mut self, asset: &str, amount: i64) -> Result<(), TradeError> {
        let entry = self.assets.entry(asset.to_owned()).or_default();
        if entry.locked < amount {
            return Err(TradeError::InvalidRequest(format!(
                "more {asset} was released than this account has locked"
            )));
        }
        entry.locked -= amount;
        entry.free = entry.free.saturating_add(amount);
        Ok(())
    }

    /// Spends `amount` of `asset` out of what a resting order locked.
    ///
    /// # Errors
    /// [`TradeError::InvalidRequest`] when more is spent than is locked.
    pub fn spend_locked(&mut self, asset: &str, amount: i64) -> Result<(), TradeError> {
        let entry = self.assets.entry(asset.to_owned()).or_default();
        if entry.locked < amount {
            return Err(TradeError::InvalidRequest(format!(
                "more {asset} was filled than this order locked"
            )));
        }
        entry.locked -= amount;
        Ok(())
    }
}
