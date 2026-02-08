//! Oracle-based Curve Family
//!
//! This module contains oracle-based pricing curves that use external price feeds
//! for swap pricing. Each curve variant implements different spread strategies.
//!
//! ## Curve Variants
//!
//! - **ConstantSpread**: Fixed spread from oracle price
//! - **InventorySkew**: Dynamic spreads based on inventory imbalance and trade size

pub mod constant_spread;
pub mod inventory_skew;

pub use constant_spread::swap as constant_spread_swap;
pub use inventory_skew::{swap as inventory_skew_swap, InventorySkewParams};

use crate::error::CurveError;

/// Basis points denominator (10,000 = 100%)
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Maximum basis points value (100%)
pub const MAX_BPS: u64 = 10_000;

/// Maximum allowed price offset in basis points (±50% = ±5000 bps)
pub const MAX_PRICE_OFFSET_BPS: i64 = 5000;

/// Apply a price offset in basis points to an oracle price
///
/// This adjusts the oracle price by a small percentage (positive or negative).
/// The formula is: `adjusted_price = price * (10000 + offset_bps) / 10000`
///
/// # Arguments
/// * `price_value` - The raw oracle price value (u128 to support multiplied price chains)
/// * `price_offset_bps` - Offset in basis points (can be negative)
///
/// # Returns
/// The adjusted price value as u128
///
/// # Example
/// ```
/// use kdex_curve::oracle::apply_price_offset;
///
/// // Shift price down by 5 basis points (0.05%)
/// let adjusted = apply_price_offset(1000000, -5).unwrap();
/// assert_eq!(adjusted, 999500);
///
/// // Shift price up by 10 basis points (0.10%)
/// let adjusted = apply_price_offset(1000000, 10).unwrap();
/// assert_eq!(adjusted, 1001000);
///
/// // No offset
/// let adjusted = apply_price_offset(1000000, 0).unwrap();
/// assert_eq!(adjusted, 1000000);
/// ```
pub fn apply_price_offset(price_value: u128, price_offset_bps: i64) -> crate::Result<u128> {
    if price_offset_bps == 0 {
        return Ok(price_value);
    }

    // Use larger type to handle potential overflow during calculation
    let multiplier = (BPS_DENOMINATOR as i128)
        .checked_add(price_offset_bps as i128)
        .ok_or(CurveError::Overflow)?;

    // Ensure multiplier is positive
    if multiplier <= 0 {
        return Err(CurveError::CalculationFailure);
    }

    // adjusted = price * (10000 + offset) / 10000
    let adjusted = price_value
        .checked_mul(multiplier as u128)
        .ok_or(CurveError::Overflow)?
        .checked_div(BPS_DENOMINATOR)
        .ok_or(CurveError::DivisionByZero)?;

    Ok(adjusted)
}
