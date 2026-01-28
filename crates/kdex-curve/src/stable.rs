//! StableSwap Curve
//!
//! An optimized curve for trading pegged assets (e.g., stablecoins, LSTs).
//! Uses the StableSwap invariant from Curve Finance.
//!
//! The invariant is:
//! ```text
//! A * sum(x_i) * n^n + D = A * D * n^n + D^(n+1) / (n^n * prod(x_i))
//! ```
//!
//! Where:
//! - A is the amplification coefficient
//! - D is the total value when balanced
//! - n is the number of coins (2 for this implementation)
//! - x_i are the balances

use spl_math::{checked_ceil_div::CheckedCeilDiv, uint::U256};

use crate::{CurveError, Result, SwapResult};

/// Number of coins in the pool (always 2 for this implementation)
const N_COINS: u8 = 2;

/// Maximum iterations for Newton's method
const ITERATIONS: u16 = 256;

/// Minimum amplification coefficient
pub const MIN_AMP: u64 = 1;

/// Maximum amplification coefficient
pub const MAX_AMP: u64 = 1_000_000;

/// Parameters for stable curve swap
#[derive(Clone, Copy, Debug)]
pub struct StableSwapParams {
    /// Amplification coefficient (higher = more like constant sum)
    pub amp: u64,
    /// Decimal scaling factor for token A (e.g., 1 for same decimals, 1000 for 3 decimal difference)
    pub token_a_factor: u64,
    /// Decimal scaling factor for token B
    pub token_b_factor: u64,
}

impl StableSwapParams {
    /// Create new stable swap params
    pub fn new(amp: u64, token_a_factor: u64, token_b_factor: u64) -> Self {
        Self {
            amp,
            token_a_factor,
            token_b_factor,
        }
    }

    /// Create params from token decimals
    pub fn from_decimals(amp: u64, token_a_decimals: u8, token_b_decimals: u8) -> Result<Self> {
        let (token_a_factor, token_b_factor) = if token_a_decimals > token_b_decimals {
            // Safe: subtraction already guarded by comparison
            let diff = token_a_decimals.saturating_sub(token_b_decimals);
            (1, 10u64.saturating_pow(diff as u32))
        } else if token_b_decimals > token_a_decimals {
            // Safe: subtraction already guarded by comparison
            let diff = token_b_decimals.saturating_sub(token_a_decimals);
            (10u64.saturating_pow(diff as u32), 1)
        } else {
            (1, 1)
        };
        Ok(Self::new(amp, token_a_factor, token_b_factor))
    }
}

/// Compute Ann (A * n^n)
fn compute_ann(amp: u64) -> Result<u64> {
    amp.checked_mul(N_COINS as u64).ok_or(CurveError::Overflow)
}

/// Returns a^b using U256
fn try_u8_power(a: &U256, b: u8) -> Result<U256> {
    let mut result = *a;
    for _ in 1..b {
        result = result.checked_mul(*a).ok_or(CurveError::Overflow)?;
    }
    Ok(result)
}

/// Returns a * b using U256 (via repeated addition)
fn try_u8_mul(a: &U256, b: u8) -> Result<U256> {
    let mut result = *a;
    for _ in 1..b {
        result = result.checked_add(*a).ok_or(CurveError::Overflow)?;
    }
    Ok(result)
}

/// Compute next D value in Newton's method iteration
///
/// D = (AnnS + D_P * n) * D / ((Ann - 1) * D + (n + 1) * D_P)
fn compute_next_d(ann: u64, d_init: &U256, d_product: &U256, sum_x: u128) -> Result<U256> {
    // An^n * sum(x)
    let anns = U256::from(ann)
        .checked_mul(sum_x.into())
        .ok_or(CurveError::Overflow)?;

    // D = (AnnS + D_P * n) * D / ((Ann - 1) * D + (n + 1) * D_P)
    let d_product_times_n = try_u8_mul(d_product, N_COINS)?;
    let numerator = anns
        .checked_add(d_product_times_n)
        .ok_or(CurveError::Overflow)?
        .checked_mul(*d_init)
        .ok_or(CurveError::Overflow)?;

    let ann_minus_1 = ann.checked_sub(1).ok_or(CurveError::Overflow)?;
    let d_product_times_n_plus_1 = try_u8_mul(
        d_product,
        N_COINS.checked_add(1).ok_or(CurveError::Overflow)?,
    )?;
    let denominator = d_init
        .checked_mul(ann_minus_1.into())
        .ok_or(CurveError::Overflow)?
        .checked_add(d_product_times_n_plus_1)
        .ok_or(CurveError::Overflow)?;

    numerator
        .checked_div(denominator)
        .ok_or(CurveError::DivisionByZero)
}

/// Compute stable swap invariant (D) using Newton's method
///
/// D is the total amount of tokens when they have equal price (equilibrium).
pub fn compute_d(ann: u64, amount_a: u128, amount_b: u128) -> Result<u128> {
    let sum_x = amount_a.checked_add(amount_b).ok_or(CurveError::Overflow)?;

    if sum_x == 0 {
        return Ok(0);
    }

    let amount_a_times_coins = try_u8_mul(&U256::from(amount_a), N_COINS)?;
    let amount_b_times_coins = try_u8_mul(&U256::from(amount_b), N_COINS)?;

    let mut d_previous: U256;
    let mut d: U256 = sum_x.into();

    // Iteratively approximate D
    for _ in 0..ITERATIONS {
        // D_P = D^(n+1) / (n^n * prod(x_i))
        let mut d_product = d;
        d_product = d_product
            .checked_mul(d)
            .ok_or(CurveError::Overflow)?
            .checked_div(amount_a_times_coins)
            .ok_or(CurveError::DivisionByZero)?;
        d_product = d_product
            .checked_mul(d)
            .ok_or(CurveError::Overflow)?
            .checked_div(amount_b_times_coins)
            .ok_or(CurveError::DivisionByZero)?;

        d_previous = d;
        d = compute_next_d(ann, &d, &d_product, sum_x)?;

        // Equality with precision of 1
        if d.abs_diff(d_previous) <= 1.into() {
            break;
        }
    }

    u128::try_from(d).map_err(|_| CurveError::ConversionFailure)
}

/// Compute swap amount y in proportion to x using Newton's method
///
/// Solves for y in the StableSwap invariant given the new x.
pub fn compute_y(ann: u64, x: u128, d: u128) -> Result<u128> {
    let ann: U256 = ann.into();
    let new_source_amount: U256 = x.into();
    let d: U256 = d.into();
    let zero = U256::zero();
    let one = U256::one();

    // b = S + D / Ann
    let b = new_source_amount
        .checked_add(d.checked_div(ann).ok_or(CurveError::DivisionByZero)?)
        .ok_or(CurveError::Overflow)?;

    // c = D^(n+1) / (n^n * P * Ann)
    // Rewrite to avoid overflows: c = (D * D / P * n) * (D / Ann * n)
    let n_coins: u128 = N_COINS.into();
    let x_times_n = x.checked_mul(n_coins).ok_or(CurveError::Overflow)?;
    let mut c = d
        .checked_mul(d)
        .ok_or(CurveError::Overflow)?
        .checked_div(x_times_n.into())
        .ok_or(CurveError::DivisionByZero)?;

    let ann_times_n = ann
        .checked_mul(N_COINS.into())
        .ok_or(CurveError::Overflow)?;
    c = c
        .checked_mul(d)
        .ok_or(CurveError::Overflow)?
        .checked_div(ann_times_n)
        .ok_or(CurveError::DivisionByZero)?;

    // Solve for y using Newton's method
    let mut y = d;
    for _ in 0..ITERATIONS {
        // y = (y^2 + c) / (2y + b - D)
        let numerator = try_u8_power(&y, 2)?
            .checked_add(c)
            .ok_or(CurveError::Overflow)?;
        let denominator = try_u8_mul(&y, 2)?
            .checked_add(b)
            .ok_or(CurveError::Overflow)?
            .checked_sub(d)
            .ok_or(CurveError::Overflow)?;

        let (y_new, _) = numerator.checked_ceil_div(denominator).unwrap_or_else(|| {
            if numerator == U256::from(0u128) {
                (zero, zero)
            } else {
                (one, zero)
            }
        });

        if y_new == y {
            break;
        } else {
            y = y_new;
        }
    }

    u128::try_from(y).map_err(|_| CurveError::CalculationFailure)
}

/// Scale up amount by factor
fn scale_up(amount: u128, factor: u64) -> Result<u128> {
    if factor == 0 {
        return Err(CurveError::CalculationFailure);
    }
    if factor == 1 {
        return Ok(amount);
    }
    amount
        .checked_mul(factor as u128)
        .ok_or(CurveError::Overflow)
}

/// Scale down amount by factor
fn scale_down(amount: u128, factor: u64, round_up: bool) -> Result<u128> {
    if factor == 0 {
        return Err(CurveError::CalculationFailure);
    }
    if factor == 1 {
        return Ok(amount);
    }
    let factor = factor as u128;
    // Safe: factor != 0 checked above
    let result = amount
        .checked_div(factor)
        .ok_or(CurveError::DivisionByZero)?;
    // Safe: factor != 0 and result <= amount/factor, so multiplication won't overflow
    let product = factor.saturating_mul(result);
    if round_up && product < amount {
        result.checked_add(1).ok_or(CurveError::Overflow)
    } else {
        Ok(result)
    }
}

/// Calculate a stable swap
///
/// # Arguments
/// * `source_amount` - Amount of tokens being swapped in
/// * `pool_source_amount` - Current pool balance of the source token
/// * `pool_destination_amount` - Current pool balance of the destination token
/// * `params` - Stable swap parameters (amp, scaling factors)
/// * `is_a_to_b` - True if swapping A to B, false for B to A
///
/// # Returns
/// A `SwapResult` containing the amounts swapped
pub fn swap(
    source_amount: u128,
    pool_source_amount: u128,
    pool_destination_amount: u128,
    params: &StableSwapParams,
    is_a_to_b: bool,
) -> Result<SwapResult> {
    if source_amount == 0 {
        return Ok(SwapResult::new(0, 0));
    }

    let ann = compute_ann(params.amp)?;

    // Scale inputs based on trade direction
    let (source_amt_scaled, pool_source_amt_scaled, pool_dest_amt_scaled) = if is_a_to_b {
        (
            scale_up(source_amount, params.token_a_factor)?,
            scale_up(pool_source_amount, params.token_a_factor)?,
            scale_up(pool_destination_amount, params.token_b_factor)?,
        )
    } else {
        (
            scale_up(source_amount, params.token_b_factor)?,
            scale_up(pool_source_amount, params.token_b_factor)?,
            scale_up(pool_destination_amount, params.token_a_factor)?,
        )
    };

    // Calculate new amounts
    let new_source_amount = pool_source_amt_scaled
        .checked_add(source_amt_scaled)
        .ok_or(CurveError::Overflow)?;

    let d = compute_d(ann, pool_source_amt_scaled, pool_dest_amt_scaled)?;
    let new_destination_amount = compute_y(ann, new_source_amount, d)?;

    // Scale down the result
    let dest_factor = if is_a_to_b {
        params.token_b_factor
    } else {
        params.token_a_factor
    };

    let new_destination_amount_unscaled = scale_down(new_destination_amount, dest_factor, true)?;

    let amount_swapped = pool_destination_amount
        .checked_sub(new_destination_amount_unscaled)
        .ok_or(CurveError::CalculationFailure)?;

    Ok(SwapResult::new(source_amount, amount_swapped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(amp: u64) -> StableSwapParams {
        StableSwapParams::new(amp, 1, 1)
    }

    #[test]
    fn test_swap_zero() {
        let params = make_params(100);
        let result = swap(0, 100_000_000, 100_000_000, &params, true).unwrap();
        assert_eq!(result.source_amount_swapped, 0);
        assert_eq!(result.destination_amount_swapped, 0);
    }

    #[test]
    fn test_swap_balanced_pool() {
        let params = make_params(100);
        let result = swap(1_000_000, 100_000_000, 100_000_000, &params, true).unwrap();

        assert_eq!(result.source_amount_swapped, 1_000_000);
        // Should get close to 1:1 with high amp
        assert!(result.destination_amount_swapped > 900_000);
        assert!(result.destination_amount_swapped < 1_000_000);
    }

    #[test]
    fn test_swap_higher_amp_gives_better_rate() {
        let params_low = make_params(10);
        let params_high = make_params(1000);

        let result_low = swap(1_000_000, 100_000_000, 100_000_000, &params_low, true).unwrap();
        let result_high = swap(1_000_000, 100_000_000, 100_000_000, &params_high, true).unwrap();

        // Higher amp should give better rate (closer to 1:1)
        assert!(result_high.destination_amount_swapped > result_low.destination_amount_swapped);
    }

    #[test]
    fn test_compute_d_balanced() {
        let ann = compute_ann(100).unwrap();
        let d = compute_d(ann, 100_000_000, 100_000_000).unwrap();
        // D should be approximately the sum for balanced pool
        assert!(d > 199_000_000);
        assert!(d < 201_000_000);
    }

    #[test]
    fn test_from_decimals() {
        // Token A has fewer decimals (6), token B has more (9)
        // Scale up A by 10^3 to match B
        let params = StableSwapParams::from_decimals(100, 6, 9).unwrap();
        assert_eq!(params.token_a_factor, 1000); // scale A up by 10^(9-6)
        assert_eq!(params.token_b_factor, 1);

        // Token A has more decimals (9), token B has fewer (6)
        // Scale up B by 10^3 to match A
        let params = StableSwapParams::from_decimals(100, 9, 6).unwrap();
        assert_eq!(params.token_a_factor, 1);
        assert_eq!(params.token_b_factor, 1000); // scale B up by 10^(9-6)

        let params = StableSwapParams::from_decimals(100, 6, 6).unwrap();
        assert_eq!(params.token_a_factor, 1);
        assert_eq!(params.token_b_factor, 1);
    }
}
