//! Liquidity-aware quoting utilities
//!
//! This module provides helpers for finding the maximum input amount whose swap
//! output fits within the destination vault, using binary search. It also includes
//! an oracle-based preemptive estimator to seed the search efficiently.
//!
//! These utilities are used by SDK adapters and arb bots.

use crate::TradeDirection;

/// Result of attempting a swap quote at a given input amount.
///
/// Generic over `T`, the payload carried on success (e.g. `u64` for output-only,
/// or `(u64, u64)` for (output, fees)).
#[derive(Debug)]
pub enum SwapFit<T = u64> {
    /// Output fits within the destination vault.
    Fits(T),
    /// Output exceeds destination vault liquidity.
    ExceedsVault,
    /// Other error (e.g., input too small to cover fees).
    OtherError,
}

/// Binary search for the maximum input amount whose swap output fits within
/// the destination vault.
///
/// Uses a proportional estimate seeded from the `required`/`available` ratio to
/// start the search within 1% of the true answer, keeping iterations to ~2–5
/// in practice rather than 64.
///
/// # Arguments
/// * `amount_in` - The originally requested input amount (which exceeded vault capacity)
/// * `required` - The output amount that was required (from an `InsufficientLiquidity` error)
/// * `available` - The actual available vault balance
/// * `try_swap` - Closure that attempts a quote at a given input and returns a `SwapFit<T>`
///
/// # Returns
/// `(consumed_input, T)` — the largest input that fits and the associated output payload.
/// Returns `(0, T::default())` if no valid input is found.
pub fn search_max_input<T: Copy + Default>(
    amount_in: u64,
    required: u64,
    available: u64,
    try_swap: impl Fn(u64) -> SwapFit<T>,
) -> (u64, T) {
    // Proportional estimate: exact for linear curves, close for non-linear
    let estimate = (amount_in as u128)
        .saturating_mul(available as u128)
        .checked_div(required as u128)
        .unwrap_or(0) as u64;

    // Seed the binary search around the estimate with a 1% margin
    let margin = estimate / 100;
    let mut lo: u64 = estimate.saturating_sub(margin);
    let mut hi: u64 = estimate
        .saturating_add(margin)
        .min(amount_in.saturating_sub(1));
    let mut best: (u64, T) = (0, T::default());

    while lo <= hi {
        let mid = lo.saturating_add(hi.saturating_sub(lo) / 2);
        match try_swap(mid) {
            SwapFit::Fits(out) => {
                best = (mid, out);
                if mid == u64::MAX {
                    break;
                }
                lo = mid.saturating_add(1);
            }
            SwapFit::ExceedsVault => {
                if mid == 0 {
                    break;
                }
                hi = mid.saturating_sub(1);
            }
            SwapFit::OtherError => {
                if mid == u64::MAX {
                    break;
                }
                lo = mid.saturating_add(1);
            }
        }
    }

    best
}

/// Preemptively estimate the maximum input that will not exceed a target fraction
/// of the destination vault, using the oracle price.
///
/// This avoids a round-trip through the quote function when the oracle price is
/// available, targeting `target_bps / 10000` of vault capacity (e.g. 9800 = 98%).
///
/// # Arguments
/// * `amount_in` - The originally requested input (used as a ceiling)
/// * `vault_capacity` - Current destination vault balance
/// * `oracle_price_value` - Raw oracle price numerator (e.g. Scope `price.value`)
/// * `oracle_price_exp` - Oracle price exponent; actual price = value / 10^exp
/// * `price_offset_bps` - Price offset in basis points (negated before applying per SDK convention)
/// * `trade_direction` - Direction of the swap
/// * `fee_bps` - Total fee in basis points (trade + owner)
/// * `target_bps` - Target vault utilization in basis points (e.g. 9800 = 98%)
///
/// # Returns
/// Estimated maximum input, capped to `amount_in`.
#[allow(clippy::too_many_arguments)]
pub fn estimate_max_input_for_vault(
    amount_in: u64,
    vault_capacity: u64,
    oracle_price_value: u128,
    oracle_price_exp: u64,
    price_offset_bps: i64,
    trade_direction: TradeDirection,
    fee_bps: u64,
    target_bps: u16,
) -> u64 {
    // Target a fraction of vault capacity
    let target_output = (vault_capacity as u128)
        .saturating_mul(target_bps as u128)
        .checked_div(10000)
        .expect("division by 10000 should never fail");

    // Adjust oracle price for price_offset_bps (negated per SDK convention)
    let adjusted_price = {
        let negated_offset = (price_offset_bps as i128)
            .checked_neg()
            .expect("price offset negation should not overflow");
        let offset_factor = 10000i128
            .checked_add(negated_offset)
            .expect("price offset addition should not overflow");
        if offset_factor <= 0 {
            panic!(
                "price offset resulted in non-positive multiplier: {}",
                offset_factor
            );
        }
        oracle_price_value
            .checked_mul(offset_factor as u128)
            .expect("price multiplication should not overflow")
            .checked_div(10000)
            .expect("division by 10000 should never fail")
    };

    // Conservative fee factor (worst-case: full fee applied)
    let fee_factor = (10000u128).saturating_sub(fee_bps as u128);

    let estimated_max = match trade_direction {
        TradeDirection::AtoB => {
            // output ≈ input * price / 10^exp * (1 - fees)
            // ⇒ input_max = target_output * 10^exp / price / (1 - fees)
            let scale = 10u128.saturating_pow(oracle_price_exp as u32);
            target_output
                .saturating_mul(scale)
                .saturating_mul(10000)
                .checked_div(adjusted_price)
                .expect("price division should not fail with positive adjusted price")
                .checked_div(fee_factor)
                .expect("fee factor division should not fail")
        }
        TradeDirection::BtoA => {
            // output ≈ input * 10^exp / price * (1 - fees)
            // ⇒ input_max = target_output * price / 10^exp / (1 - fees)
            let scale = 10u128.saturating_pow(oracle_price_exp as u32);
            target_output
                .saturating_mul(adjusted_price)
                .saturating_mul(10000)
                .checked_div(scale)
                .expect("scale division should not fail with valid exponent")
                .checked_div(fee_factor)
                .expect("fee factor division should not fail")
        }
    } as u64;

    amount_in.min(estimated_max)
}

/// Proportionally cap input when the estimated amount still exceeds vault capacity.
///
/// Used as a fallback after `estimate_max_input_for_vault` when an
/// `InsufficientLiquidity` error still occurs, scaling down the input based
/// on the ratio of available to required output.
///
/// # Arguments
/// * `amount_in` - The input that was tried (which exceeded capacity)
/// * `output_amount` - The output that was required (from `InsufficientLiquidity`)
/// * `vault_capacity` - The available vault balance
/// * `target_bps` - Target vault utilization in basis points (e.g. 9800 = 98%)
pub fn cap_input_proportional(
    amount_in: u64,
    output_amount: u64,
    vault_capacity: u64,
    target_bps: u16,
) -> u64 {
    (amount_in as u128)
        .saturating_mul(vault_capacity as u128)
        .saturating_mul(target_bps as u128)
        .checked_div(output_amount as u128)
        .unwrap_or(0)
        .checked_div(10000)
        .unwrap_or(0) as u64
}
