//! Offset Curve (Constant Product with Virtual Offset)
//!
//! Uses the constant product invariant but adds a virtual offset to token B:
//! `token_a * (token_b + offset) = k`
//!
//! This allows trading to continue even when the actual token B balance is zero,
//! as long as the offset provides virtual liquidity.

use crate::{constant_product, math::checked_add, CurveError, Result, SwapResult, TradeDirection};

/// Calculate an offset curve swap
///
/// The offset is added to the token B side, creating virtual liquidity.
///
/// # Arguments
/// * `source_amount` - Amount of tokens being swapped in
/// * `pool_source_amount` - Current pool balance of the source token
/// * `pool_destination_amount` - Current pool balance of the destination token
/// * `token_b_offset` - Virtual offset for token B
/// * `trade_direction` - Direction of the trade (AtoB or BtoA)
///
/// # Returns
/// A `SwapResult` containing the amounts swapped
///
/// # Example
/// ```
/// use kdex_curve::{offset, TradeDirection};
///
/// // Pool has 1M token A, 0 token B, but 1M virtual offset
/// let result = offset::swap(
///     100,           // input 100 token A
///     1_000_000,     // 1M pool source (A)
///     0,             // 0 actual token B
///     1_000_000,     // 1M virtual offset
///     TradeDirection::AtoB,
/// ).unwrap();
///
/// // Can still get tokens due to virtual offset
/// assert!(result.destination_amount_swapped > 0);
/// ```
pub fn swap(
    source_amount: u128,
    pool_source_amount: u128,
    pool_destination_amount: u128,
    token_b_offset: u64,
    trade_direction: TradeDirection,
) -> Result<SwapResult> {
    let token_b_offset = token_b_offset as u128;

    // Adjust amounts based on trade direction
    let (adjusted_source, adjusted_dest) = match trade_direction {
        TradeDirection::AtoB => {
            // When trading A for B, add offset to destination (B side)
            (
                pool_source_amount,
                checked_add(pool_destination_amount, token_b_offset)?,
            )
        }
        TradeDirection::BtoA => {
            // When trading B for A, add offset to source (B side)
            (
                checked_add(pool_source_amount, token_b_offset)?,
                pool_destination_amount,
            )
        }
    };

    // Use constant product swap with adjusted amounts
    constant_product::swap(source_amount, adjusted_source, adjusted_dest)
}

/// Calculate an offset curve swap (ExactOut mode)
///
/// # Arguments
/// * `destination_amount` - Desired amount of tokens to receive
/// * `pool_source_amount` - Current pool balance of the source token
/// * `pool_destination_amount` - Current pool balance of the destination token
/// * `token_b_offset` - Virtual offset for token B
/// * `trade_direction` - Direction of the trade (AtoB or BtoA)
pub fn swap_exact_out(
    destination_amount: u128,
    pool_source_amount: u128,
    pool_destination_amount: u128,
    token_b_offset: u64,
    trade_direction: TradeDirection,
) -> Result<SwapResult> {
    let token_b_offset = token_b_offset as u128;

    // Adjust amounts based on trade direction
    let (adjusted_source, adjusted_dest) = match trade_direction {
        TradeDirection::AtoB => (
            pool_source_amount,
            checked_add(pool_destination_amount, token_b_offset)?,
        ),
        TradeDirection::BtoA => (
            checked_add(pool_source_amount, token_b_offset)?,
            pool_destination_amount,
        ),
    };

    // For AtoB, check we're not trying to withdraw more than actual + offset provides
    // Note: The offset provides virtual liquidity, so we only fail if exceeding adjusted_dest
    if destination_amount >= adjusted_dest {
        return Err(CurveError::CalculationFailure);
    }

    // For BtoA, the destination is A which has no offset
    if trade_direction == TradeDirection::BtoA && destination_amount >= pool_destination_amount {
        return Err(CurveError::CalculationFailure);
    }

    constant_product::swap_exact_out(destination_amount, adjusted_source, adjusted_dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_no_offset() {
        // With zero offset, should behave like constant product
        let result = swap(100, 1_000, 50_000, 0, TradeDirection::AtoB).unwrap();
        assert_eq!(result.source_amount_swapped, 100);
        // 100 / (1000 + 100) * 50000 ≈ 4545
        assert_eq!(result.destination_amount_swapped, 4545);
    }

    #[test]
    fn test_swap_with_offset_a_to_b() {
        // Pool: 1M token A, 0 token B, but 1M offset
        let result = swap(100, 1_000_000, 0, 1_000_000, TradeDirection::AtoB).unwrap();
        assert_eq!(result.source_amount_swapped, 100);
        // Should get ~99 tokens (1:1 ratio with tiny price impact)
        assert!(result.destination_amount_swapped > 90);
        assert!(result.destination_amount_swapped <= 100);
    }

    #[test]
    fn test_swap_with_offset_b_to_a() {
        // Pool: 0 token B (+ 1M offset), 1M token A
        let result = swap(100, 0, 1_000_000, 1_000_000, TradeDirection::BtoA).unwrap();
        assert_eq!(result.source_amount_swapped, 100);
        // Should get ~99 tokens
        assert!(result.destination_amount_swapped > 90);
        assert!(result.destination_amount_swapped <= 100);
    }

    #[test]
    fn test_swap_zero_b_no_offset_fails() {
        // Without offset, swapping to empty B side should fail
        let result = swap(100, 1_000_000, 0, 0, TradeDirection::AtoB);
        assert!(result.is_err() || result.unwrap().destination_amount_swapped == 0);
    }

    #[test]
    fn test_swap_b_to_a_overflow_protection() {
        // Large offset shouldn't cause overflow (may produce zero output due to extreme skew)
        let result = swap(1_000, 10_000_000, 1_000, u64::MAX, TradeDirection::BtoA);
        // Either succeeds with zero/minimal output, or fails with ZeroTradingTokens (not overflow)
        assert!(result.is_ok() || matches!(result, Err(CurveError::ZeroTradingTokens)));
    }

    #[test]
    fn test_swap_exact_out() {
        // Want exactly 99 tokens out
        let result = swap_exact_out(99, 1_000_000, 0, 1_000_000, TradeDirection::AtoB).unwrap();
        assert_eq!(result.destination_amount_swapped, 99);
        // Should need ~100 input
        assert!(result.source_amount_swapped >= 99);
        assert!(result.source_amount_swapped < 110);
    }
}
