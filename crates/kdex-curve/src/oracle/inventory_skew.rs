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

use crate::{
    math::{checked_add, checked_div, checked_mul, checked_sub},
    CurveError, Result, SwapResult, TradeDirection,
};

use super::BPS_DENOMINATOR;

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
    /// Inventory equilibrium target (in lamports)
    pub inv_equilibrium: u64,
    /// Maximum inventory deviation for full skew (in lamports)
    pub inv_max: u64,
    /// Reference trade size for size impact (in lamports)
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
/// Returns value scaled by 10000, clamped to [-10000, 10000] (i.e., [-1.0, 1.0])
fn calculate_inventory_ratio(current_inventory: u128, inv_equilibrium: u64, inv_max: u64) -> i128 {
    if inv_max == 0 {
        return 0;
    }

    let inv_equilibrium = inv_equilibrium as i128;
    let inv_max = inv_max as i128;
    let current_inventory = current_inventory as i128;

    // y = (inventory - equilibrium) / inv_max, scaled by 10000
    let numerator = current_inventory.saturating_sub(inv_equilibrium);
    // Safe: inv_max != 0 checked above, using checked_div for clippy
    let y = numerator
        .saturating_mul(10000)
        .checked_div(inv_max)
        .unwrap_or(0);

    // Clamp to [-10000, 10000]
    y.clamp(-10000, 10000)
}

/// Calculate size impact factor: f = (swap_size / q_ref)^alpha
/// Returns value scaled by 10000
fn calculate_size_impact_factor(swap_size: u128, q_ref: u64, alpha: u64) -> Result<u128> {
    if q_ref == 0 {
        return Ok(0);
    }

    let q_ref = q_ref as u128;

    // ratio = swap_size / q_ref, scaled by 10000
    let ratio = checked_div(checked_mul(swap_size, 10000)?, q_ref)?;

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

/// Calculate inventory-aware swap amounts given an oracle price
///
/// # Arguments
/// * `source_amount` - Amount of source tokens to swap
/// * `price_value` - Oracle price value
/// * `price_exp` - Oracle price exponent
/// * `trade_direction` - Direction of the trade (AtoB or BtoA)
/// * `pool_source_amount` - Current pool balance of source token (for inventory calculation)
/// * `params` - Inventory skew parameters
///
/// # Returns
/// A `SwapResult` containing the amounts swapped
pub fn swap(
    source_amount: u128,
    price_value: u64,
    price_exp: u64,
    trade_direction: TradeDirection,
    pool_source_amount: u128,
    params: &InventorySkewParams,
) -> Result<SwapResult> {
    if source_amount == 0 {
        return Err(CurveError::ZeroAmount);
    }

    let price_value = price_value as u128;

    // Calculate inventory ratio
    let y = calculate_inventory_ratio(pool_source_amount, params.inv_equilibrium, params.inv_max);

    // Calculate size impact factor
    let f = calculate_size_impact_factor(source_amount, params.q_ref, params.alpha)?;

    // Calculate dynamic spreads
    let (bid_spread_bps, ask_spread_bps) = calculate_dynamic_spreads(
        params.base_spread_bps,
        params.size_spread_bps,
        params.skew_bps,
        y,
        f,
    )?;

    let (source_amount_swapped, destination_amount_swapped) = match trade_direction {
        // User buying B with A - pays ask price
        TradeDirection::AtoB => {
            let effective_price = checked_div(
                checked_mul(
                    price_value,
                    checked_add(BPS_DENOMINATOR, ask_spread_bps as u128)?,
                )?,
                BPS_DENOMINATOR,
            )?;

            let scale = 10u128.pow(price_exp as u32);
            let destination = checked_div(checked_mul(source_amount, scale)?, effective_price)?;

            (source_amount, destination)
        }
        // User selling B for A - receives bid price
        TradeDirection::BtoA => {
            let effective_price = checked_div(
                checked_mul(
                    price_value,
                    checked_sub(BPS_DENOMINATOR, bid_spread_bps as u128)?,
                )?,
                BPS_DENOMINATOR,
            )?;

            let scale = 10u128.pow(price_exp as u32);
            let destination = checked_div(checked_mul(source_amount, effective_price)?, scale)?;

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
            10,            // base_spread_bps = 0.1%
            40,            // size_spread_bps = 0.4%
            100,           // skew_bps = 1%
            1_000_000_000, // inv_equilibrium (1B units)
            500_000_000,   // inv_max (500M units)
            100_000_000,   // q_ref (100M units - matches 100 token swap with 6 decimals)
            20000,         // alpha = 2.0
        )
    }

    #[test]
    fn test_inventory_ratio_balanced() {
        let y = calculate_inventory_ratio(1000, 1000, 500);
        assert_eq!(y, 0);
    }

    #[test]
    fn test_inventory_ratio_excess() {
        // 500 over equilibrium, max deviation 500 → y = 1.0
        let y = calculate_inventory_ratio(1500, 1000, 500);
        assert_eq!(y, 10000);
    }

    #[test]
    fn test_inventory_ratio_deficit() {
        // 500 under equilibrium → y = -1.0
        let y = calculate_inventory_ratio(500, 1000, 500);
        assert_eq!(y, -10000);
    }

    #[test]
    fn test_size_impact_factor_equal() {
        // Size equals q_ref → f = 1.0
        let f = calculate_size_impact_factor(1000, 1000, 10000).unwrap();
        assert_eq!(f, 10000);
    }

    #[test]
    fn test_size_impact_factor_half() {
        // Size is half of q_ref → f = 0.5
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
        assert_eq!(bid, 25);
        assert_eq!(ask, 75);
    }

    #[test]
    fn test_swap_balanced() {
        let params = make_params();
        let result = swap(
            100_000000,   // 100 tokens (with 6 decimals)
            100_00000000, // $100 price
            8,
            TradeDirection::AtoB,
            1_000_000_000, // at equilibrium
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

        // When inventory is above equilibrium and buying B (selling A),
        // the ask spread is normal
        let result = swap(
            100_000000,
            100_00000000,
            8,
            TradeDirection::AtoB,
            1_500_000_000, // excess inventory (above equilibrium)
            &params,
        )
        .unwrap();

        // Should still work, bid spread is widened but we're using ask
        assert!(result.destination_amount_swapped > 0);
    }

    #[test]
    fn test_swap_zero_amount() {
        let params = make_params();
        assert!(swap(0, 100_00000000, 8, TradeDirection::AtoB, 1000, &params).is_err());
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
}
