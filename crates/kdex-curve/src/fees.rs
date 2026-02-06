//! Fee calculation utilities for KDEX pools
//!
//! This module provides fee calculation methods that match the on-chain program logic.

use crate::{CurveError, Result, RoundDirection};

/// Encapsulates all fee information and calculations for swap operations
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fees {
    /// Trade fee numerator
    pub trade_fee_numerator: u64,
    /// Trade fee denominator
    pub trade_fee_denominator: u64,
    /// Owner trade fee numerator
    pub owner_trade_fee_numerator: u64,
    /// Owner trade fee denominator
    pub owner_trade_fee_denominator: u64,
    /// Owner withdraw fee numerator
    pub owner_withdraw_fee_numerator: u64,
    /// Owner withdraw fee denominator
    pub owner_withdraw_fee_denominator: u64,
    /// Host trading fee numerator
    pub host_fee_numerator: u64,
    /// Host trading fee denominator
    pub host_fee_denominator: u64,
}

/// Helper function for calculating swap fee
pub fn calculate_fee(
    token_amount: u128,
    fee_numerator: u128,
    fee_denominator: u128,
    round_direction: RoundDirection,
) -> Result<u128> {
    if fee_numerator == 0 || token_amount == 0 {
        Ok(0)
    } else {
        let fee = token_amount
            .checked_mul(fee_numerator)
            .ok_or(CurveError::Overflow)?
            .checked_div(fee_denominator)
            .ok_or(CurveError::DivisionByZero)?;
        if fee == 0 {
            let rounded_fee = match round_direction {
                RoundDirection::Floor => 0,
                RoundDirection::Ceiling => 1,
            };
            Ok(rounded_fee)
        } else {
            Ok(fee)
        }
    }
}

fn ceil_div(dividend: u128, divisor: u128) -> Result<u128> {
    dividend
        .checked_add(divisor)
        .ok_or(CurveError::Overflow)?
        .checked_sub(1)
        .ok_or(CurveError::Overflow)?
        .checked_div(divisor)
        .ok_or(CurveError::DivisionByZero)
}

fn pre_fee_amount(
    post_fee_amount: u128,
    fee_numerator: u128,
    fee_denominator: u128,
) -> Result<u128> {
    if fee_numerator == 0 || fee_denominator == 0 {
        Ok(post_fee_amount)
    } else if fee_numerator == fee_denominator || post_fee_amount == 0 {
        Ok(0)
    } else {
        let numerator = post_fee_amount
            .checked_mul(fee_denominator)
            .ok_or(CurveError::Overflow)?;
        let denominator = fee_denominator
            .checked_sub(fee_numerator)
            .ok_or(CurveError::Overflow)?;
        ceil_div(numerator, denominator)
    }
}

impl Fees {
    /// Create a new Fees instance
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        trade_fee_numerator: u64,
        trade_fee_denominator: u64,
        owner_trade_fee_numerator: u64,
        owner_trade_fee_denominator: u64,
        owner_withdraw_fee_numerator: u64,
        owner_withdraw_fee_denominator: u64,
        host_fee_numerator: u64,
        host_fee_denominator: u64,
    ) -> Self {
        Self {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        }
    }

    /// Calculate the withdraw fee in trading tokens
    pub fn owner_withdraw_fee(&self, trading_tokens: u128) -> Result<u128> {
        calculate_fee(
            trading_tokens,
            u128::from(self.owner_withdraw_fee_numerator),
            u128::from(self.owner_withdraw_fee_denominator),
            RoundDirection::Ceiling,
        )
    }

    /// Calculate the trading fee in trading tokens
    pub fn trading_fee(&self, trading_tokens: u128) -> Result<u128> {
        calculate_fee(
            trading_tokens,
            u128::from(self.trade_fee_numerator),
            u128::from(self.trade_fee_denominator),
            RoundDirection::Ceiling,
        )
    }

    /// Calculate the owner trading fee in trading tokens
    pub fn owner_trading_fee(&self, trading_tokens: u128) -> Result<u128> {
        calculate_fee(
            trading_tokens,
            u128::from(self.owner_trade_fee_numerator),
            u128::from(self.owner_trade_fee_denominator),
            RoundDirection::Ceiling,
        )
    }

    /// Calculate the inverse trading amount, how much input is needed to give the
    /// provided output
    pub fn pre_trading_fee_amount(&self, post_fee_amount: u128) -> Result<u128> {
        if self.trade_fee_numerator == 0 || self.trade_fee_denominator == 0 {
            pre_fee_amount(
                post_fee_amount,
                u128::from(self.owner_trade_fee_numerator),
                u128::from(self.owner_trade_fee_denominator),
            )
        } else if self.owner_trade_fee_numerator == 0 || self.owner_trade_fee_denominator == 0 {
            pre_fee_amount(
                post_fee_amount,
                u128::from(self.trade_fee_numerator),
                u128::from(self.trade_fee_denominator),
            )
        } else {
            let trade_fee_num = u128::from(self.trade_fee_numerator);
            let trade_fee_den = u128::from(self.trade_fee_denominator);
            let owner_fee_num = u128::from(self.owner_trade_fee_numerator);
            let owner_fee_den = u128::from(self.owner_trade_fee_denominator);

            let numerator = trade_fee_num
                .checked_mul(owner_fee_den)
                .ok_or(CurveError::Overflow)?
                .checked_add(
                    owner_fee_num
                        .checked_mul(trade_fee_den)
                        .ok_or(CurveError::Overflow)?,
                )
                .ok_or(CurveError::Overflow)?;
            let denominator = trade_fee_den
                .checked_mul(owner_fee_den)
                .ok_or(CurveError::Overflow)?;

            pre_fee_amount(post_fee_amount, numerator, denominator)
        }
    }

    /// Calculate the host fee based on the owner fee
    pub fn host_fee(&self, owner_fee: u128) -> Result<u128> {
        calculate_fee(
            owner_fee,
            u128::from(self.host_fee_numerator),
            u128::from(self.host_fee_denominator),
            RoundDirection::Floor,
        )
    }

    /// Validate that the fees are reasonable (numerator < denominator)
    pub fn validate(&self) -> Result<()> {
        validate_fraction(self.trade_fee_numerator, self.trade_fee_denominator)?;
        validate_fraction(
            self.owner_trade_fee_numerator,
            self.owner_trade_fee_denominator,
        )?;
        validate_fraction(
            self.owner_withdraw_fee_numerator,
            self.owner_withdraw_fee_denominator,
        )?;
        validate_fraction(self.host_fee_numerator, self.host_fee_denominator)?;
        Ok(())
    }
}

fn validate_fraction(numerator: u64, denominator: u64) -> Result<()> {
    if denominator == 0 && numerator == 0 {
        Ok(())
    } else if numerator >= denominator {
        Err(CurveError::InvalidFee)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trading_fee() {
        let fees = Fees {
            trade_fee_numerator: 1,
            trade_fee_denominator: 100,
            ..Default::default()
        };
        // 1% of 1000 = 10
        assert_eq!(fees.trading_fee(1000).unwrap(), 10);
    }

    #[test]
    fn test_zero_fee() {
        let fees = Fees::default();
        assert_eq!(fees.trading_fee(1000).unwrap(), 0);
    }

    #[test]
    fn test_validate_valid_fees() {
        let fees = Fees {
            trade_fee_numerator: 1,
            trade_fee_denominator: 100,
            owner_trade_fee_numerator: 1,
            owner_trade_fee_denominator: 100,
            owner_withdraw_fee_numerator: 0,
            owner_withdraw_fee_denominator: 0,
            host_fee_numerator: 0,
            host_fee_denominator: 0,
        };
        assert!(fees.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_fees() {
        let fees = Fees {
            trade_fee_numerator: 100,
            trade_fee_denominator: 1, // numerator >= denominator is invalid
            ..Default::default()
        };
        assert!(fees.validate().is_err());
    }
}
