//! Math utilities for curve calculations

use crate::{CurveError, Result, RoundDirection, TradingTokenResult};

/// Ceiling division: (a + b - 1) / b
#[inline]
pub fn ceiling_div(a: u128, b: u128) -> Result<u128> {
    if b == 0 {
        return Err(CurveError::DivisionByZero);
    }
    // Safe: b != 0 checked above
    a.saturating_add(b)
        .saturating_sub(1)
        .checked_div(b)
        .ok_or(CurveError::DivisionByZero)
}

/// Ceiling division with adjusted divisor
///
/// Returns (ceiling_quotient, adjusted_divisor) where:
/// - ceiling_quotient = ceil(a / b)
/// - adjusted_divisor = the minimum value that when used as divisor, still gives
///   the same ceiling quotient while maximizing fairness to the pool
///
/// This matches the behavior of spl-math's `checked_ceil_div`:
/// 1. Calculate initial quotient and check for remainder
/// 2. If remainder exists, ceiling the quotient
/// 3. Recalculate the divisor: adjusted_divisor = ceil(a / ceiling_quotient)
///
/// This ensures that `ceiling_quotient * adjusted_divisor >= a` (invariant preserved).
///
/// Example: ceil_div_with_adjustment(299_800_000, 30_000) = (9994, 29998)
/// - 299_800_000 / 30_000 = 9993 remainder 10000
/// - ceiling quotient = 9994
/// - adjusted_divisor = ceil(299_800_000 / 9994) = ceil(29997.99...) = 29998
///
/// This is useful for constant product swaps where you want to know exactly
/// how much input was needed to produce the ceiling output.
#[inline]
pub fn ceiling_div_with_adjustment(a: u128, b: u128) -> Result<(u128, u128)> {
    if b == 0 {
        return Err(CurveError::DivisionByZero);
    }

    // Safe: b != 0 checked above
    let quotient = a.checked_div(b).ok_or(CurveError::DivisionByZero)?;

    // Avoid dividing a small number by a big one and returning 1
    // (this matches spl-math behavior)
    if quotient == 0 {
        return Err(CurveError::CalculationFailure);
    }

    // Safe: b != 0 checked above
    let remainder = a.checked_rem(b).ok_or(CurveError::DivisionByZero)?;

    if remainder > 0 {
        // Ceiling the quotient
        let ceiling_quotient = quotient.checked_add(1).ok_or(CurveError::Overflow)?;

        // Recalculate the divisor to find the minimum value that still gives this ceiling
        // adjusted_divisor = ceil(a / ceiling_quotient)
        // Safe: ceiling_quotient > 0 since quotient >= 1
        let adjusted_divisor = a
            .checked_div(ceiling_quotient)
            .ok_or(CurveError::DivisionByZero)?;
        let remainder2 = a
            .checked_rem(ceiling_quotient)
            .ok_or(CurveError::DivisionByZero)?;

        let adjusted_divisor = if remainder2 > 0 {
            adjusted_divisor
                .checked_add(1)
                .ok_or(CurveError::Overflow)?
        } else {
            adjusted_divisor
        };

        Ok((ceiling_quotient, adjusted_divisor))
    } else {
        // No remainder, quotient and divisor are exact
        Ok((quotient, b))
    }
}

/// Checked multiplication with overflow protection
#[inline]
pub fn checked_mul(a: u128, b: u128) -> Result<u128> {
    a.checked_mul(b).ok_or(CurveError::Overflow)
}

/// Checked division with zero check
#[inline]
pub fn checked_div(a: u128, b: u128) -> Result<u128> {
    a.checked_div(b).ok_or(CurveError::DivisionByZero)
}

/// Checked addition with overflow protection
#[inline]
pub fn checked_add(a: u128, b: u128) -> Result<u128> {
    a.checked_add(b).ok_or(CurveError::Overflow)
}

/// Checked subtraction with underflow protection
#[inline]
pub fn checked_sub(a: u128, b: u128) -> Result<u128> {
    a.checked_sub(b).ok_or(CurveError::Overflow)
}

/// Convert pool tokens to trading tokens (for deposits/withdrawals)
///
/// This function calculates how many of each trading token corresponds to
/// a given amount of pool tokens.
///
/// # Arguments
/// * `pool_tokens` - Amount of pool tokens
/// * `pool_token_supply` - Total supply of pool tokens
/// * `pool_token_a_amount` - Current amount of token A in the pool
/// * `pool_token_b_amount` - Current amount of token B in the pool
/// * `round_direction` - Whether to round up (ceiling) or down (floor)
pub fn pool_tokens_to_trading_tokens(
    pool_tokens: u128,
    pool_token_supply: u128,
    pool_token_a_amount: u128,
    pool_token_b_amount: u128,
    round_direction: RoundDirection,
) -> Result<TradingTokenResult> {
    if pool_token_supply == 0 {
        return Err(CurveError::DivisionByZero);
    }

    let (token_a_amount, token_b_amount) = match round_direction {
        RoundDirection::Floor => {
            let token_a = checked_mul(pool_tokens, pool_token_a_amount)?;
            let token_a = checked_div(token_a, pool_token_supply)?;
            let token_b = checked_mul(pool_tokens, pool_token_b_amount)?;
            let token_b = checked_div(token_b, pool_token_supply)?;
            (token_a, token_b)
        }
        RoundDirection::Ceiling => {
            let token_a = checked_mul(pool_tokens, pool_token_a_amount)?;
            let token_a = ceiling_div(token_a, pool_token_supply)?;
            let token_b = checked_mul(pool_tokens, pool_token_b_amount)?;
            let token_b = ceiling_div(token_b, pool_token_supply)?;
            (token_a, token_b)
        }
    };

    Ok(TradingTokenResult {
        token_a_amount,
        token_b_amount,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ceiling_div() {
        assert_eq!(ceiling_div(10, 3).unwrap(), 4); // 10/3 = 3.33... -> 4
        assert_eq!(ceiling_div(9, 3).unwrap(), 3); // 9/3 = 3 -> 3
        assert_eq!(ceiling_div(1, 2).unwrap(), 1); // 1/2 = 0.5 -> 1
        assert_eq!(ceiling_div(0, 5).unwrap(), 0); // 0/5 = 0 -> 0
        assert!(ceiling_div(10, 0).is_err()); // Division by zero
    }

    #[test]
    fn test_pool_tokens_to_trading_tokens_floor() {
        let result =
            pool_tokens_to_trading_tokens(100, 1000, 5000, 10000, RoundDirection::Floor).unwrap();
        // 100/1000 = 10% of pool
        // token_a = 5000 * 10% = 500
        // token_b = 10000 * 10% = 1000
        assert_eq!(result.token_a_amount, 500);
        assert_eq!(result.token_b_amount, 1000);
    }

    #[test]
    fn test_pool_tokens_to_trading_tokens_ceiling() {
        // With non-divisible amounts, ceiling should round up
        let result = pool_tokens_to_trading_tokens(1, 3, 10, 10, RoundDirection::Ceiling).unwrap();
        // 1/3 of 10 = 3.33... -> 4
        assert_eq!(result.token_a_amount, 4);
        assert_eq!(result.token_b_amount, 4);
    }

    #[test]
    fn test_ceiling_div_with_adjustment() {
        // Test case from hyperplane's constant_product_swap_rounding test
        // (20, 30_000 - 20, 10_000, 18, 6) - source can be 18 to get 6 dest
        // invariant = 29980 * 10000 = 299_800_000
        // new_pool_source = 29980 + 20 = 30_000
        // ceiling_div_with_adjustment(299_800_000, 30_000) should return (9994, 29998)
        let (quotient, adjusted_divisor) =
            ceiling_div_with_adjustment(299_800_000, 30_000).unwrap();
        assert_eq!(quotient, 9994);
        assert_eq!(adjusted_divisor, 29998);
        // source_swapped = 29998 - 29980 = 18

        // Exact division case: no remainder means divisor unchanged
        let (quotient, adjusted_divisor) = ceiling_div_with_adjustment(100, 10).unwrap();
        assert_eq!(quotient, 10);
        assert_eq!(adjusted_divisor, 10);

        // Another test case: (10, 4_000_000, 70_000_000_000, 10, 174_999)
        // invariant = 4_000_000 * 70_000_000_000 = 280_000_000_000_000_000
        // new_source = 4_000_000 + 10 = 4_000_010
        let (quotient, adjusted_divisor) =
            ceiling_div_with_adjustment(280_000_000_000_000_000, 4_000_010).unwrap();
        // new_dest = quotient, source_swapped = adjusted_divisor - 4_000_000
        // Expected: dest_swapped = 70_000_000_000 - new_dest = 174_999
        // So new_dest = 69_999_825_001
        assert_eq!(70_000_000_000u128 - quotient, 174_999);
        assert_eq!(adjusted_divisor - 4_000_000, 10);

        // Division by zero should fail
        assert!(ceiling_div_with_adjustment(100, 0).is_err());

        // Small number divided by large should fail (quotient = 0)
        assert!(ceiling_div_with_adjustment(10, 100).is_err());
    }
}
