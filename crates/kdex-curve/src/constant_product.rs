//! Constant Product Curve (x * y = k)
//!
//! The classic AMM invariant used by Uniswap V2 and many other DEXes.
//! After a swap, the product of the pool balances remains constant (minus fees).

use crate::{
    math::{ceiling_div, ceiling_div_with_adjustment, checked_add, checked_mul, checked_sub},
    CurveError, Result, SwapResult,
};

/// Calculate a constant product swap
///
/// Given an input amount and pool balances, calculates the output amount
/// such that `source_balance * destination_balance = k` holds after the swap.
///
/// Uses ceiling division to favor the pool. The returned `source_amount_swapped`
/// may be less than the input `source_amount` due to rounding - this is the
/// precise amount actually needed to achieve the output.
///
/// # Arguments
/// * `source_amount` - Amount of tokens being swapped in
/// * `pool_source_amount` - Current pool balance of the source token
/// * `pool_destination_amount` - Current pool balance of the destination token
///
/// # Returns
/// A `SwapResult` containing the amounts swapped
///
/// # Example
/// ```
/// use kdex_curve::constant_product;
///
/// let result = constant_product::swap(
///     1_000_000,    // input 1M tokens
///     100_000_000,  // pool has 100M source
///     100_000_000,  // pool has 100M destination
/// ).unwrap();
///
/// // With 1% of pool, you get ~0.99% of the other side (due to price impact)
/// assert!(result.destination_amount_swapped < 1_000_000);
/// ```
pub fn swap(
    source_amount: u128,
    pool_source_amount: u128,
    pool_destination_amount: u128,
) -> Result<SwapResult> {
    if source_amount == 0 {
        return Ok(SwapResult::new(0, 0));
    }

    if pool_source_amount == 0 || pool_destination_amount == 0 {
        return Err(CurveError::ZeroAmount);
    }

    // invariant = pool_source_amount * pool_destination_amount
    let invariant = checked_mul(pool_source_amount, pool_destination_amount)?;

    // new_source_amount = pool_source_amount + source_amount
    let new_source_amount = checked_add(pool_source_amount, source_amount)?;

    // Ceiling division returns (new_destination_amount, actual_new_source) where:
    // - new_destination_amount = ceil(invariant / new_source_amount)
    // - actual_new_source = new_source_amount adjusted for ceiling rounding
    //
    // The second value represents the actual new pool source amount that achieves
    // the ceiling output. Due to rounding, we may not need all of source_amount.
    let (new_destination_amount, actual_new_source) =
        ceiling_div_with_adjustment(invariant, new_source_amount)?;

    // source_amount_swapped = actual_new_source - pool_source_amount
    // This may be less than source_amount when ceiling division rounds
    let source_amount_swapped = checked_sub(actual_new_source, pool_source_amount)?;

    // destination_amount_swapped = pool_destination_amount - new_destination_amount
    let destination_amount_swapped = checked_sub(pool_destination_amount, new_destination_amount)?;

    // Ensure we produce non-zero trading tokens (matches on-chain behavior)
    if source_amount_swapped == 0 || destination_amount_swapped == 0 {
        return Err(CurveError::ZeroTradingTokens);
    }

    Ok(SwapResult::new(
        source_amount_swapped,
        destination_amount_swapped,
    ))
}

/// Calculate input amount needed for a desired output (ExactOut mode)
///
/// Given a desired output amount, calculates how much input is needed.
///
/// # Arguments
/// * `destination_amount` - Desired amount of tokens to receive
/// * `pool_source_amount` - Current pool balance of the source token
/// * `pool_destination_amount` - Current pool balance of the destination token
///
/// # Returns
/// A `SwapResult` with the required input and desired output
pub fn swap_exact_out(
    destination_amount: u128,
    pool_source_amount: u128,
    pool_destination_amount: u128,
) -> Result<SwapResult> {
    if destination_amount == 0 {
        return Ok(SwapResult::new(0, 0));
    }

    if pool_source_amount == 0 || pool_destination_amount == 0 {
        return Err(CurveError::ZeroAmount);
    }

    // Cannot withdraw more than the pool has
    if destination_amount >= pool_destination_amount {
        return Err(CurveError::CalculationFailure);
    }

    // invariant = pool_source_amount * pool_destination_amount
    let invariant = checked_mul(pool_source_amount, pool_destination_amount)?;

    // new_destination_amount = pool_destination_amount - destination_amount
    let new_destination_amount = checked_sub(pool_destination_amount, destination_amount)?;

    // new_source_amount = invariant / new_destination_amount (ceiling to favor the pool)
    let new_source_amount = ceiling_div(invariant, new_destination_amount)?;

    // source_amount_needed = new_source_amount - pool_source_amount
    let source_amount_needed = checked_sub(new_source_amount, pool_source_amount)?;

    Ok(SwapResult::new(source_amount_needed, destination_amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_basic() {
        // Swap 1M into a 100M/100M pool
        let result = swap(1_000_000, 100_000_000, 100_000_000).unwrap();
        // source_amount_swapped may be <= input due to ceiling division adjustment
        assert!(result.source_amount_swapped <= 1_000_000);
        assert!(result.source_amount_swapped > 0);
        // ~990,099 due to price impact and ceiling division
        assert!(result.destination_amount_swapped > 980_000);
        assert!(result.destination_amount_swapped < 1_000_000);
    }

    #[test]
    fn test_swap_zero() {
        let result = swap(0, 100_000_000, 100_000_000).unwrap();
        assert_eq!(result.source_amount_swapped, 0);
        assert_eq!(result.destination_amount_swapped, 0);
    }

    #[test]
    fn test_swap_empty_pool() {
        assert!(swap(1000, 0, 100_000_000).is_err());
        assert!(swap(1000, 100_000_000, 0).is_err());
    }

    #[test]
    fn test_swap_preserves_invariant() {
        let source = 100_000_000u128;
        let dest = 100_000_000u128;
        let swap_amount = 10_000_000u128;

        let invariant_before = source * dest;
        let result = swap(swap_amount, source, dest).unwrap();

        let new_source = source + result.source_amount_swapped;
        let new_dest = dest - result.destination_amount_swapped;
        let invariant_after = new_source * new_dest;

        // Invariant should be >= before (ceiling division favors the pool)
        assert!(invariant_after >= invariant_before);
    }

    #[test]
    fn test_swap_exact_out_basic() {
        // Want exactly 990,000 out of a 100M/100M pool
        let result = swap_exact_out(990_000, 100_000_000, 100_000_000).unwrap();
        assert_eq!(result.destination_amount_swapped, 990_000);
        // Should need approximately 1M input
        assert!(result.source_amount_swapped > 990_000);
        assert!(result.source_amount_swapped < 1_010_000);
    }

    #[test]
    fn test_swap_exact_out_too_much() {
        // Cannot withdraw more than the pool has
        assert!(swap_exact_out(100_000_001, 100_000_000, 100_000_000).is_err());
    }

    #[test]
    fn test_swap_imbalanced_pool() {
        // Pool with 10:1 ratio
        let result = swap(1_000_000, 10_000_000, 100_000_000).unwrap();
        // Should get ~9M tokens (better rate on the cheap side)
        assert!(result.destination_amount_swapped > 8_000_000);
        assert!(result.destination_amount_swapped < 10_000_000);
    }

    /// Test cases matching hyperplane's constant_product_swap_rounding test
    /// These verify the precise source_amount_swapped calculation
    #[test]
    fn test_swap_rounding_matches_hyperplane() {
        // Test cases from hyperplane: (source_amount, pool_source, pool_dest, expected_source_swapped, expected_dest_swapped)
        let tests: &[(u128, u128, u128, u128, u128)] = &[
            (10, 4_000_000, 70_000_000_000, 10, 174_999),
            (20, 30_000 - 20, 10_000, 18, 6), // source can be 18 to get 6 dest
            (19, 30_000 - 20, 10_000, 18, 6),
            (18, 30_000 - 20, 10_000, 18, 6),
            (10, 20_000, 30_000, 10, 14),
            (10, 20_000 - 9, 30_000, 10, 14),
            (10, 20_000 - 10, 30_000, 10, 15),
            (100, 60_000, 30_000, 99, 49), // source can be 99 to get 49 dest
            (99, 60_000, 30_000, 99, 49),
            (98, 60_000, 30_000, 97, 48), // source can be 97 to get 48 dest
        ];

        for (source, pool_source, pool_dest, expected_source, expected_dest) in tests {
            let result = swap(*source, *pool_source, *pool_dest).unwrap();
            assert_eq!(
                result.source_amount_swapped, *expected_source,
                "source mismatch for ({}, {}, {}): got {}, expected {}",
                source, pool_source, pool_dest, result.source_amount_swapped, expected_source
            );
            assert_eq!(
                result.destination_amount_swapped, *expected_dest,
                "dest mismatch for ({}, {}, {}): got {}, expected {}",
                source, pool_source, pool_dest, result.destination_amount_swapped, expected_dest
            );
        }
    }

    #[test]
    fn test_swap_too_small_fails() {
        // Much too small - would result in 0 destination
        assert!(swap(10, 70_000_000_000, 4_000_000).is_err());
    }
}
