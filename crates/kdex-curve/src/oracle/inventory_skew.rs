//! Inventory Skew Oracle Curve
//!
//! Oracle-based pricing with inventory-aware dynamic spreads.
//! This curve adjusts bid/ask spreads based on:
//! - Current inventory position relative to equilibrium (inventory skew)
//! - Trade size (larger trades incur wider spreads)
//! - Oracle price as the reference point
//!
//! The pricing model aims to maintain inventory balance by widening spreads when
//! inventory deviates from equilibrium.

use spl_math::uint::U256;

use crate::{
    math::{checked_add, checked_div, checked_mul, checked_sub},
    CurveError, Result, SwapResult, TradeDirection,
};

use super::BPS_DENOMINATOR;

/// Multiply then divide using U256 to avoid overflow
/// Computes (a * b) / c
#[inline]
fn mul_div(a: u128, b: u128, c: u128) -> Result<u128> {
    if c == 0 {
        return Err(CurveError::DivisionByZero);
    }
    let numerator = U256::from(a)
        .checked_mul(U256::from(b))
        .ok_or(CurveError::Overflow)?;
    let result = numerator
        .checked_div(U256::from(c))
        .ok_or(CurveError::DivisionByZero)?;
    result.try_into().map_err(|_| CurveError::Overflow)
}

/// Compute 10^exp as U256, supporting exponents up to 77
#[inline]
#[allow(clippy::arithmetic_side_effects)] // Safe: subtractions are guarded by if conditions
fn pow10_u256(exp: u64) -> U256 {
    // Max exp that fits in u128 is 38
    if exp <= 38 {
        U256::from(10u128.pow(exp as u32))
    } else {
        // For larger exponents, compute in steps
        let base = U256::from(10u128.pow(38));
        let remaining = exp - 38;
        if remaining <= 38 {
            base.saturating_mul(U256::from(10u128.pow(remaining as u32)))
        } else {
            // exp > 76, very unlikely but handle it
            let mid = U256::from(10u128.pow(38));
            let remaining2 = remaining - 38;
            base.saturating_mul(mid)
                .saturating_mul(U256::from(10u128.pow(remaining2.min(38) as u32)))
        }
    }
}

/// Multiply then divide using U256 with scale as U256
/// Computes (a * scale) / c where scale = 10^exp
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

/// Multiply then divide using U256 with scale as divisor
/// Computes (a * b) / scale where scale = 10^exp
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

/// Scaling factor for alpha (stored as alpha * ALPHA_SCALE)
/// e.g., alpha = 2.0 is stored as 20000
const ALPHA_SCALE: u128 = 10_000;

/// Maximum spread (just under 100%)
const MAX_SPREAD_BPS: u64 = 9999;

/// Parameters for inventory skew pricing
#[derive(Clone, Copy, Debug)]
pub struct InventorySkewParams {
    /// Base spread in basis points (minimum spread)
    pub base_spread_bps: u64,
    /// Size-dependent spread in basis points
    pub size_spread_bps: u64,
    /// Inventory skew spread in basis points
    pub skew_bps: u64,
    /// Inventory equilibrium target as a ratio (scaled by 10000, e.g., 5000 = 0.5 = 50%)
    pub inv_equilibrium: u64,
    /// Maximum inventory deviation for full skew as a ratio (scaled by 10000, e.g., 10000 = 1.0 = 100%)
    pub inv_max: u64,
    /// Reference trade size for size impact as ratio of pool value (scaled by 10000, e.g., 1000 = 0.1 = 10% of pool value)
    pub q_ref: u64,
    /// Alpha exponent for size impact (scaled by 10000, e.g., 20000 = 2.0)
    pub alpha: u64,
}

impl InventorySkewParams {
    /// Create new inventory skew parameters
    pub fn new(
        base_spread_bps: u64,
        size_spread_bps: u64,
        skew_bps: u64,
        inv_equilibrium: u64,
        inv_max: u64,
        q_ref: u64,
        alpha: u64,
    ) -> Self {
        Self {
            base_spread_bps,
            size_spread_bps,
            skew_bps,
            inv_equilibrium,
            inv_max,
            q_ref,
            alpha,
        }
    }
}

/// Calculate inventory ratio: y = (inventory - equilibrium) / inv_max
/// All inputs are ratios scaled by 10000
/// Returns value scaled by 10000, clamped to [-10000, 10000] (i.e., [-1.0, 1.0])
fn calculate_inventory_ratio(
    current_inventory_ratio: u64,
    inv_equilibrium: u64,
    inv_max: u64,
) -> i128 {
    if inv_max == 0 {
        return 0;
    }

    let inv_equilibrium = inv_equilibrium as i128;
    let inv_max = inv_max as i128;
    let current_inventory = current_inventory_ratio as i128;

    // y = (inventory - equilibrium) / inv_max
    // All values already scaled by 10000, so we need to maintain scaling
    let numerator = current_inventory.saturating_sub(inv_equilibrium);
    // numerator is scaled by 10000, inv_max is scaled by 10000
    // To get proper scaling: (numerator * 10000) / inv_max
    // Safe: inv_max != 0 checked above, using checked_div for clippy
    let y = numerator
        .saturating_mul(10000)
        .checked_div(inv_max)
        .unwrap_or(0);

    // Clamp to [-10000, 10000]
    y.clamp(-10000, 10000)
}

/// Calculate size impact factor: f = (swap_size_ratio / q_ref)^alpha
/// where swap_size_ratio = value of source amount / value of pool
/// All inputs are ratios scaled by 10000
/// Returns value scaled by 10000
fn calculate_size_impact_factor(swap_size_ratio: u64, q_ref: u64, alpha: u64) -> Result<u128> {
    if q_ref == 0 {
        return Ok(0);
    }

    let swap_size_ratio = swap_size_ratio as u128;
    let q_ref = q_ref as u128;

    // ratio = swap_size_ratio / q_ref
    // Both already scaled by 10000, so we need to maintain scaling
    let ratio = checked_div(checked_mul(swap_size_ratio, 10000)?, q_ref)?;

    // Apply power function
    power_fixed_point(ratio, alpha as u128, ALPHA_SCALE)
}

/// Fixed-point power function: base^(exp_num/exp_denom)
/// All values scaled by 10000
fn power_fixed_point(base: u128, exp_num: u128, exp_denom: u128) -> Result<u128> {
    if base == 0 {
        return Ok(0);
    }

    if exp_num == 0 {
        return Ok(10000); // x^0 = 1 (scaled)
    }

    if exp_denom == 0 {
        return Err(CurveError::DivisionByZero);
    }

    // Check for integer exponents
    // Safe: exp_denom != 0 checked above
    if exp_num.checked_rem(exp_denom).unwrap_or(1) == 0 {
        let exp_int = exp_num.checked_div(exp_denom).unwrap_or(0);
        return power_integer(base, exp_int as u32);
    }

    // Common alpha values with optimized handling
    if exp_denom == ALPHA_SCALE {
        match exp_num {
            20000 => {
                // alpha = 2.0: x^2
                return checked_div(checked_mul(base, base)?, 10000);
            }
            15000 => {
                // alpha = 1.5: x * sqrt(x)
                let sqrt_x = base.saturating_mul(10000).isqrt();
                return checked_div(checked_mul(base, sqrt_x)?, 10000);
            }
            10000 => {
                // alpha = 1.0: x
                return Ok(base);
            }
            _ => {}
        }
    }

    // General case: linear approximation for on-chain efficiency
    let base_diff = base.saturating_sub(10000);
    let scaled_diff = checked_div(checked_mul(base_diff, exp_num)?, exp_denom)?;
    checked_add(10000, scaled_diff)
}

/// Integer power function for fixed-point numbers scaled by 10000
fn power_integer(base: u128, exp: u32) -> Result<u128> {
    let mut result = 10000u128;
    for _ in 0..exp {
        result = checked_div(checked_mul(result, base)?, 10000)?;
    }
    Ok(result)
}

/// Calculate dynamic bid and ask spreads
///
/// Bid/ask are defined from the perspective of token A (base asset) quoted in token B (currency).
/// - BID = price MM pays when buying token A (user sells A via AtoB)
/// - ASK = price MM charges when selling token A (user buys A via BtoA)
///
/// Formula:
/// - base_half_spread = base_spread_bps/2 + size_spread_bps * f / 2
/// - extra_bid_bps = skew_bps * max(0, y)
/// - extra_ask_bps = skew_bps * max(0, -y)
///
/// Returns (bid_spread_bps, ask_spread_bps)
fn calculate_dynamic_spreads(
    base_spread_bps: u64,
    size_spread_bps: u64,
    skew_bps: u64,
    y: i128, // Inventory ratio scaled by 10000
    f: u128, // Size impact factor scaled by 10000
) -> Result<(u64, u64)> {
    // Base half-spread
    let base_half = base_spread_bps / 2; // Safe: division by constant

    // Size component: size_spread_bps * f / 2 (f is scaled by 10000)
    let size_component = checked_div(
        checked_div(checked_mul(size_spread_bps as u128, f)?, 10000)?,
        2,
    )? as u64;

    let base_half_spread = base_half.saturating_add(size_component);

    // Inventory skew adjustments
    // Token B (quote currency) perspective: bid/ask refer to the base asset (token A)
    // When y > 0 (excess token A), widen bid to discourage users selling more A (AtoB)
    // When y < 0 (deficit token A), widen ask to discourage users buying more A (BtoA)
    let extra_bid_bps = if y > 0 {
        let skew = skew_bps as i128;
        // Safe: skew and y are bounded, result fits in u64
        skew.saturating_mul(y)
            .checked_div(10000)
            .unwrap_or(0)
            .try_into()
            .unwrap_or(0)
    } else {
        0
    };

    let extra_ask_bps = if y < 0 {
        let skew = skew_bps as i128;
        let neg_y = y.saturating_neg();
        // Safe: skew and neg_y are bounded, result fits in u64
        skew.saturating_mul(neg_y)
            .checked_div(10000)
            .unwrap_or(0)
            .try_into()
            .unwrap_or(0)
    } else {
        0
    };

    // Final spreads, capped at MAX_SPREAD_BPS
    let bid_spread_bps = base_half_spread
        .saturating_add(extra_bid_bps)
        .min(MAX_SPREAD_BPS);
    let ask_spread_bps = base_half_spread
        .saturating_add(extra_ask_bps)
        .min(MAX_SPREAD_BPS);

    Ok((bid_spread_bps, ask_spread_bps))
}

/// Calculate inventory and swap size ratios for inventory skew pricing
///
/// This helper function converts absolute token amounts and price into normalized ratios
/// needed for the inventory skew pricing model. It normalizes everything to the destination
/// token to handle price variations correctly.
///
/// # Arguments
/// * `source_amount` - Amount of source tokens being swapped
/// * `price_value` - Oracle price value as u128 (supports multiplied price chains)
/// * `price_exp` - Oracle price exponent
/// * `trade_direction` - Direction of the trade (AtoB or BtoA)
/// * `source_vault_amount` - Current balance in source vault
/// * `destination_vault_amount` - Current balance in destination vault
///
/// # Returns
/// A tuple of (current_inventory_ratio, swap_size_ratio), both scaled by 10000
///
/// # Example
/// ```
/// use kdex_curve::oracle::inventory_skew::calculate_ratios;
/// use kdex_curve::TradeDirection;
///
/// let (inv_ratio, swap_ratio) = calculate_ratios(
///     100_000000,      // 100 tokens
///     100_00000000_u128, // $100 price
///     8,               // price exponent
///     TradeDirection::AtoB,
///     1_000_000000,    // 1000 tokens in source
///     2_000_000000,    // 2000 tokens in destination
/// ).unwrap();
/// ```
pub fn calculate_ratios(
    source_amount: u128,
    price_value: u128,
    price_exp: u64,
    trade_direction: TradeDirection,
    source_vault_amount: u128,
    destination_vault_amount: u128,
) -> Result<(u64, u64)> {
    // Map vault amounts to token A and B based on trade direction
    let (pool_token_a_amount, pool_token_b_amount) = match trade_direction {
        TradeDirection::AtoB => (source_vault_amount, destination_vault_amount),
        TradeDirection::BtoA => (destination_vault_amount, source_vault_amount),
    };

    // Calculate ratios normalized to the destination token
    // For AtoB: normalize to token B, for BtoA: normalize to token A
    let (current_inventory_ratio, swap_size_ratio) = match trade_direction {
        TradeDirection::AtoB => {
            // Normalize to token B
            // value_of_a_in_b = pool_token_a_amount * scale / price_value
            let value_of_a_in_b = mul_scale_div(pool_token_a_amount, price_exp, price_value)?;
            let total_pool_in_b = checked_add(value_of_a_in_b, pool_token_b_amount)?;

            let current_inventory_ratio =
                checked_div(checked_mul(value_of_a_in_b, 10000)?, total_pool_in_b)? as u64;

            // swap_value_in_b = source_amount * scale / price_value
            let swap_value_in_b = mul_scale_div(source_amount, price_exp, price_value)?;
            let swap_size_ratio =
                checked_div(checked_mul(swap_value_in_b, 10000)?, total_pool_in_b)? as u64;

            (current_inventory_ratio, swap_size_ratio)
        }
        TradeDirection::BtoA => {
            // Normalize to token A
            // value_of_b_in_a = pool_token_b_amount * price_value / scale
            // Use mul_div_scale to avoid overflow with large price values and exponents
            let value_of_b_in_a = mul_div_scale(pool_token_b_amount, price_value, price_exp)?;
            let total_pool_in_a = checked_add(pool_token_a_amount, value_of_b_in_a)?;

            let current_inventory_ratio =
                checked_div(checked_mul(pool_token_a_amount, 10000)?, total_pool_in_a)? as u64;

            // swap_value_in_a = source_amount * price_value / scale
            // Use mul_div_scale to avoid overflow with large price values and exponents
            let swap_value_in_a = mul_div_scale(source_amount, price_value, price_exp)?;
            let swap_size_ratio =
                checked_div(checked_mul(swap_value_in_a, 10000)?, total_pool_in_a)? as u64;

            (current_inventory_ratio, swap_size_ratio)
        }
    };

    Ok((current_inventory_ratio, swap_size_ratio))
}

/// Calculate inventory-aware swap amounts given an oracle price
///
/// # Arguments
/// * `source_amount` - Amount of source tokens to swap
/// * `price_value` - Oracle price value as u128 (supports multiplied price chains, ratio of raw A units to raw B units, scaled by 10^price_exp)
/// * `price_exp` - Oracle price exponent
/// * `trade_direction` - Direction of the trade (AtoB or BtoA)
/// * `current_inventory_ratio` - Current inventory as a ratio (scaled by 10000, e.g., 5000 = 0.5 = 50%)
/// * `swap_size_ratio` - Ratio of swap value to pool value (scaled by 10000, e.g., 1000 = 0.1 = 10% of pool value)
/// * `params` - Inventory skew parameters
///
/// # Returns
/// A `SwapResult` containing the amounts swapped
pub fn swap(
    source_amount: u128,
    price_value: u128,
    price_exp: u64,
    trade_direction: TradeDirection,
    current_inventory_ratio: u64,
    swap_size_ratio: u64,
    params: &InventorySkewParams,
) -> Result<SwapResult> {
    if source_amount == 0 {
        return Err(CurveError::ZeroAmount);
    }

    // Calculate inventory ratio
    let y = calculate_inventory_ratio(
        current_inventory_ratio,
        params.inv_equilibrium,
        params.inv_max,
    );

    // Calculate size impact factor
    let f = calculate_size_impact_factor(swap_size_ratio, params.q_ref, params.alpha)?;

    // Calculate dynamic spreads
    let (bid_spread_bps, ask_spread_bps) = calculate_dynamic_spreads(
        params.base_spread_bps,
        params.size_spread_bps,
        params.skew_bps,
        y,
        f,
    )?;

    let (source_amount_swapped, destination_amount_swapped) = match trade_direction {
        // User selling A for B - receives bid price (MM buys A)
        TradeDirection::AtoB => {
            // effective_price = price_value * (BPS_DENOMINATOR + bid_spread_bps) / BPS_DENOMINATOR
            let effective_price = mul_div(
                price_value,
                checked_add(BPS_DENOMINATOR, bid_spread_bps as u128)?,
                BPS_DENOMINATOR,
            )?;

            // destination = source_amount * scale / effective_price
            let destination = mul_scale_div(source_amount, price_exp, effective_price)?;

            (source_amount, destination)
        }
        // User buying A with B - pays ask price (MM sells A)
        TradeDirection::BtoA => {
            // effective_price = price_value * (BPS_DENOMINATOR - ask_spread_bps) / BPS_DENOMINATOR
            let effective_price = mul_div(
                price_value,
                checked_sub(BPS_DENOMINATOR, ask_spread_bps as u128)?,
                BPS_DENOMINATOR,
            )?;

            // destination = source_amount * effective_price / scale
            // Use mul_div_scale to avoid overflow with large price values and exponents
            let destination = mul_div_scale(source_amount, effective_price, price_exp)?;

            (source_amount, destination)
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

    fn make_params() -> InventorySkewParams {
        InventorySkewParams::new(
            10,    // base_spread_bps = 0.1%
            40,    // size_spread_bps = 0.4%
            100,   // skew_bps = 1%
            5000,  // inv_equilibrium = 0.5 (50% of pool)
            5000,  // inv_max = 0.5 (50% deviation range)
            1000,  // q_ref = 0.1 (10% of pool as reference trade size)
            20000, // alpha = 2.0
        )
    }

    #[test]
    fn test_inventory_ratio_balanced() {
        // current = equilibrium → y = 0
        let y = calculate_inventory_ratio(5000, 5000, 5000);
        assert_eq!(y, 0);
    }

    #[test]
    fn test_inventory_ratio_excess() {
        // current = 10000 (100%), equilibrium = 5000 (50%), max = 5000 (50%) → y = 1.0
        let y = calculate_inventory_ratio(10000, 5000, 5000);
        assert_eq!(y, 10000);
    }

    #[test]
    fn test_inventory_ratio_deficit() {
        // current = 0 (0%), equilibrium = 5000 (50%), max = 5000 (50%) → y = -1.0
        let y = calculate_inventory_ratio(0, 5000, 5000);
        assert_eq!(y, -10000);
    }

    #[test]
    fn test_size_impact_factor_equal() {
        // swap_size_ratio = q_ref → f = 1.0
        let f = calculate_size_impact_factor(1000, 1000, 10000).unwrap();
        assert_eq!(f, 10000);
    }

    #[test]
    fn test_size_impact_factor_half() {
        // swap_size_ratio is half of q_ref → f = 0.5
        let f = calculate_size_impact_factor(500, 1000, 10000).unwrap();
        assert_eq!(f, 5000);
    }

    #[test]
    fn test_dynamic_spreads_balanced() {
        // Balanced inventory, reference size
        let (bid, ask) = calculate_dynamic_spreads(10, 40, 100, 0, 10000).unwrap();
        // base_half = 5, size_component = 40 * 1.0 / 2 = 20
        // base_half_spread = 25
        assert_eq!(bid, 25);
        assert_eq!(ask, 25);
    }

    #[test]
    fn test_dynamic_spreads_excess_inventory() {
        // y = 0.5 (excess inventory) → widen bid
        let (bid, ask) = calculate_dynamic_spreads(10, 40, 100, 5000, 10000).unwrap();
        // extra_bid = 100 * 0.5 = 50
        assert_eq!(bid, 75);
        assert_eq!(ask, 25);
    }

    #[test]
    fn test_dynamic_spreads_deficit_inventory() {
        // y = -0.5 (deficit inventory) → widen ask
        let (bid, ask) = calculate_dynamic_spreads(10, 40, 100, -5000, 10000).unwrap();
        // extra_ask = 100 * 0.5 = 50
        assert_eq!(bid, 25);
        assert_eq!(ask, 75);
    }

    #[test]
    fn test_swap_balanced() {
        let params = make_params();
        let result = swap(
            100_000000,        // 100 tokens (with 6 decimals)
            100_00000000_u128, // $100 price
            8,
            TradeDirection::AtoB,
            5000, // current_inventory_ratio = 0.5 (at equilibrium)
            1000, // swap_size_ratio = 0.1 (same as q_ref)
            &params,
        )
        .unwrap();

        // Should get slightly less than 1 token due to spreads
        // With base_spread_bps = 10 (0.1%), size f = 1.0, spread = 5 + 20 = 25 bps = 0.25%
        // Effective price = $100.25, output = 100 / 100.25 ≈ 0.9975 tokens
        assert!(result.destination_amount_swapped > 990000);
        assert!(result.destination_amount_swapped < 1000000);
    }

    #[test]
    fn test_swap_excess_inventory_a_to_b() {
        let params = make_params();

        // When inventory is above equilibrium and user sells A (AtoB),
        // the bid spread is widened to discourage adding more A
        let result = swap(
            100_000000,
            100_00000000_u128,
            8,
            TradeDirection::AtoB,
            10000, // current_inventory_ratio = 1.0 (100%, excess inventory)
            1000,  // swap_size_ratio = 0.1
            &params,
        )
        .unwrap();

        // Should still work, bid spread is widened to discourage this trade
        assert!(result.destination_amount_swapped > 0);
    }

    #[test]
    fn test_swap_zero_amount() {
        let params = make_params();
        assert!(swap(
            0,
            100_00000000_u128,
            8,
            TradeDirection::AtoB,
            5000,
            1000,
            &params
        )
        .is_err());
    }

    #[test]
    fn test_power_integer() {
        // 1.5^2 = 2.25
        let result = power_integer(15000, 2).unwrap();
        assert_eq!(result, 22500);

        // 2.0^3 = 8.0
        let result = power_integer(20000, 3).unwrap();
        assert_eq!(result, 80000);
    }

    #[test]
    fn test_swap_large_price_chain() {
        // Test with a multiplied price chain that produces very large price values
        // This simulates a scenario like USDC -> TOKEN via two oracle prices
        // Combined price: 11499657551185936795317425317558800 with exp 34
        // This previously caused overflow in calculate_ratios
        let params = make_params();

        let price_value = 11499657551185936795317425317558800_u128;
        let price_exp = 34u64;
        let pool_token_a_amount = 4347826_u128; // ~4.3M units
        let pool_token_b_amount = 5000000_u128; // 5M units

        // BtoA direction (the direction that previously overflowed)
        let (inv_ratio, swap_ratio) = calculate_ratios(
            1, // 1 unit source amount
            price_value,
            price_exp,
            TradeDirection::BtoA,
            pool_token_b_amount, // source vault for BtoA
            pool_token_a_amount, // dest vault for BtoA
        )
        .unwrap();

        // Ratios should be reasonable (scaled by 10000)
        assert!(inv_ratio <= 10000);
        assert!(swap_ratio <= 10000);

        // Now test the full swap
        let result = swap(
            1,
            price_value,
            price_exp,
            TradeDirection::BtoA,
            inv_ratio,
            swap_ratio,
            &params,
        );

        // The swap may fail with ZeroTradingTokens for 1 unit input at this price,
        // but it should NOT fail with overflow
        assert!(result.is_ok() || matches!(result, Err(CurveError::ZeroTradingTokens)));
    }

    #[test]
    fn test_swap_large_price_chain_larger_amount() {
        // Same as above but with a larger input amount
        let params = make_params();

        let price_value = 11499657551185936795317425317558800_u128;
        let price_exp = 34u64;
        let pool_token_a_amount = 4347826_u128;
        let pool_token_b_amount = 5000000_u128;

        // Larger source amount (1 million units)
        let source_amount = 1_000_000_u128;

        let (inv_ratio, swap_ratio) = calculate_ratios(
            source_amount,
            price_value,
            price_exp,
            TradeDirection::BtoA,
            pool_token_b_amount,
            pool_token_a_amount,
        )
        .unwrap();

        let result = swap(
            source_amount,
            price_value,
            price_exp,
            TradeDirection::BtoA,
            inv_ratio,
            swap_ratio,
            &params,
        )
        .unwrap();

        // Should get a valid result
        assert!(result.source_amount_swapped == source_amount);
        assert!(result.destination_amount_swapped > 0);
    }
}
