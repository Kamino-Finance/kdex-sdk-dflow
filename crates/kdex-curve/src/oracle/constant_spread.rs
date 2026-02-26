//! Constant Spread Oracle Curve
//!
//! Uses prices from an oracle feed with a fixed spread.
//! The pricing is simple: `oracle_price ± constant_spread` (in basis points)

use spl_math::uint::U256;

use crate::{
    math::{checked_add, checked_div, checked_mul, checked_sub},
    CurveError, Result, SwapResult, TradeDirection,
};

use super::BPS_DENOMINATOR;

/// Compute 10^exp as U256, supporting exponents up to 77
#[inline]
fn pow10_u256(exp: u64) -> U256 {
    // Max exp that fits in u128 is 38
    if exp <= 38 {
        U256::from(10u128.pow(exp as u32))
    } else {
        // For larger exponents, compute in steps
        let base = U256::from(10u128.pow(38));
        let remaining = exp.saturating_sub(38);
        if remaining <= 38 {
            base.saturating_mul(U256::from(10u128.pow(remaining as u32)))
        } else {
            // exp > 76, very unlikely but handle it
            let mid = U256::from(10u128.pow(38));
            let remaining2 = remaining.saturating_sub(38);
            base.saturating_mul(mid)
                .saturating_mul(U256::from(10u128.pow(remaining2.min(38) as u32)))
        }
    }
}

/// Multiply by scale then divide using U256
/// Computes (a * 10^exp) / c
#[inline]
fn mul_scale_div(a: u128, exp: u64, c: u128) -> Result<u128> {
    if c == 0 {
        return Err(CurveError::DivisionByZero);
    }
    let scale = pow10_u256(exp);
    let numerator = U256::from(a)
        .checked_mul(scale)
        .ok_or(CurveError::Overflow)?;
    let result = numerator
        .checked_div(U256::from(c))
        .ok_or(CurveError::DivisionByZero)?;
    result.try_into().map_err(|_| CurveError::Overflow)
}

/// Multiply then divide by scale using U256
/// Computes (a * b) / 10^exp
#[inline]
fn mul_div_scale(a: u128, b: u128, exp: u64) -> Result<u128> {
    let scale = pow10_u256(exp);
    if scale.is_zero() {
        return Err(CurveError::DivisionByZero);
    }
    let numerator = U256::from(a)
        .checked_mul(U256::from(b))
        .ok_or(CurveError::Overflow)?;
    let result = numerator
        .checked_div(scale)
        .ok_or(CurveError::DivisionByZero)?;
    result.try_into().map_err(|_| CurveError::Overflow)
}

/// Calculate swap amounts using oracle price with constant spread
///
/// # Arguments
/// * `source_amount` - Amount of source tokens to swap
/// * `price_value` - Oracle price value as u128 (B per A ratio, supports multiplied price chains)
/// * `price_exp` - Oracle price exponent (e.g., 8 means price = value * 10^-8)
/// * `bps_from_oracle` - Spread in basis points (e.g., 50 = 0.5%)
/// * `trade_direction` - Direction of the trade (AtoB or BtoA)
///
/// # Returns
/// A `SwapResult` containing the amounts swapped
///
/// # Price Interpretation
/// The oracle provides `price = price_value * 10^(-price_exp)`, representing
/// the amount of token B per unit of token A (e.g., USDC per SOL).
///
/// For example:
/// - price_value = 100_00000000, price_exp = 8 → price = 100.0 B per A
/// - price_value = 6462236900000, price_exp = 8 → price = 64622.369 B per A
///
/// # Example
/// ```
/// use kdex_curve::{oracle::constant_spread, TradeDirection};
///
/// // Sell 100 token A at price 100 B per A, 0.5% spread
/// let result = constant_spread::swap(
///     100_000000,       // 100 token A (6 decimals)
///     100_00000000_u128, // price = 100 B per A
///     8,                // 8 decimal exponent
///     50,               // 0.5% spread
///     TradeDirection::AtoB,
/// ).unwrap();
///
/// // Get ~9950 tokens B (slightly less than 10000 due to spread)
/// assert!(result.destination_amount_swapped > 9900_000000);
/// ```
pub fn swap(
    source_amount: u128,
    price_value: u128,
    price_exp: u64,
    bps_from_oracle: u64,
    trade_direction: TradeDirection,
) -> Result<SwapResult> {
    if source_amount == 0 {
        return Err(CurveError::ZeroAmount);
    }

    let bps = bps_from_oracle as u128;

    let (source_amount_swapped, destination_amount_swapped) = match trade_direction {
        // User selling A for B — receives bid price (lower)
        // Oracle price = B per A (e.g., USDC per SOL)
        // dest_B = src_A * effective_price / 10^exp
        TradeDirection::AtoB => {
            // Bid: lower effective price → user receives less B per A
            let effective_price_value = checked_div(
                checked_mul(price_value, checked_sub(BPS_DENOMINATOR, bps)?)?,
                BPS_DENOMINATOR,
            )?;

            // destination = source_amount * effective_price / scale
            let destination_amount =
                mul_div_scale(source_amount, effective_price_value, price_exp)?;

            (source_amount, destination_amount)
        }
        // User buying A with B — pays ask price (higher)
        // dest_A = src_B * 10^exp / effective_price
        TradeDirection::BtoA => {
            // Ask: higher effective price → user receives less A per B
            let effective_price_value = checked_div(
                checked_mul(price_value, checked_add(BPS_DENOMINATOR, bps)?)?,
                BPS_DENOMINATOR,
            )?;

            // destination = source_amount * scale / effective_price
            let destination_amount =
                mul_scale_div(source_amount, price_exp, effective_price_value)?;

            (source_amount, destination_amount)
        }
    };

    if source_amount_swapped == 0 || destination_amount_swapped == 0 {
        return Err(CurveError::ZeroTradingTokens);
    }

    Ok(SwapResult::new(
        source_amount_swapped,
        destination_amount_swapped,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_a_to_b() {
        // Oracle price: 100 B per A (e.g., 1 token A = 100 USDC)
        // price_value = 100_00000000, price_exp = 8
        // bps = 50 (0.5% spread)
        let source_amount = 100_000000u128; // 100 token A (6 decimals)

        let result = swap(
            source_amount,
            100_00000000_u128,
            8,
            50,
            TradeDirection::AtoB,
        )
        .unwrap();

        // AtoB (sell A for B): effective_price = 100 * (1 - 0.005) = 99.5
        // destination = 100 * 99.5 = 9950 tokens B
        assert_eq!(result.source_amount_swapped, source_amount);
        assert!(result.destination_amount_swapped > 9900_000000);
        assert!(result.destination_amount_swapped < 10000_000000);
    }

    #[test]
    fn test_swap_b_to_a() {
        // Oracle price: 100 B per A
        let source_amount = 1000_000000u128; // 1000 token B

        let result = swap(
            source_amount,
            100_00000000_u128,
            8,
            50,
            TradeDirection::BtoA,
        )
        .unwrap();

        // BtoA (buy A with B): effective_price = 100 * (1 + 0.005) = 100.5
        // destination = 1000 / 100.5 = 9.950248... tokens A
        assert_eq!(result.source_amount_swapped, source_amount);
        assert!(result.destination_amount_swapped > 9_000000);
        assert!(result.destination_amount_swapped < 10_000000);
    }

    #[test]
    fn test_swap_zero_spread() {
        // With zero spread, 100 A at price 100 B/A → 10000 B
        let result = swap(100_000000, 100_00000000_u128, 8, 0, TradeDirection::AtoB).unwrap();
        // 100 * 100 = 10000
        assert_eq!(result.destination_amount_swapped, 10000_000000);
    }

    #[test]
    fn test_swap_different_decimals() {
        // Price: 0.001 B per A (i.e., 1 A is worth 0.001 B)
        // price_value = 1_000000, price_exp = 9 → price = 0.001
        // Swapping 1 A should give 0.001 B
        // destination = source * price / 10^exp = 1_000000 * 1_000000 / 10^9 = 1000
        let result = swap(1_000000, 1_000000_u128, 9, 0, TradeDirection::AtoB).unwrap();
        assert_eq!(result.destination_amount_swapped, 1000);
    }

    #[test]
    fn test_swap_zero_amount() {
        assert!(swap(0, 100_00000000_u128, 8, 50, TradeDirection::AtoB).is_err());
    }

    #[test]
    fn test_swap_high_spread() {
        // 50% spread (5000 bps), price = 100 B per A
        // AtoB: effective_price = 100 * (1 - 0.5) = 50
        // destination = 100 * 50 = 5000 tokens B
        let result = swap(100_000000, 100_00000000_u128, 8, 5000, TradeDirection::AtoB).unwrap();
        assert!(result.destination_amount_swapped > 4900_000000);
        assert!(result.destination_amount_swapped < 5100_000000);
    }

    #[test]
    fn test_swap_large_price_value() {
        // Test with a large price value that previously would overflow u64
        // This simulates a multiplied price chain
        let large_price = 10_165_542_217_535_919_058_620_280_u128;
        let price_exp = 25;
        let source_amount = 200_000000_u128; // 200 tokens with 6 decimals

        let result = swap(
            source_amount,
            large_price,
            price_exp,
            50,
            TradeDirection::AtoB,
        )
        .unwrap();

        // Should produce a valid result without overflow
        assert!(result.destination_amount_swapped > 0);
        assert_eq!(result.source_amount_swapped, source_amount);
    }
}
