//! Quote calculation for non-oracle curves
//!
//! This module provides off-chain quote calculation for standard (non-oracle) curve types.

use crate::curves::{ConstantPriceCurve, OffsetCurve, StableCurve};
use crate::generated::types::Fees;
use borsh::BorshDeserialize;
use kdex_curve::CurveType;
pub use kdex_curve::TradeDirection;
use thiserror::Error;

/// Quote calculation errors
#[derive(Debug, Error)]
pub enum QuoteError {
    #[error("Deserialization error: {0}")]
    DeserializationError(String),

    #[error("Calculation error: {0}")]
    CalculationError(String),

    #[error("Unsupported curve type: {0:?}")]
    UnsupportedCurveType(CurveType),

    #[error("Insufficient liquidity: required {required}, available {available}")]
    InsufficientLiquidity { required: u64, available: u64 },
}

/// Result type for quote operations
pub type Result<T> = std::result::Result<T, QuoteError>;

/// Swap result from quote calculation
#[derive(Debug, Clone, Copy)]
pub struct SwapResult {
    /// Amount of source tokens swapped
    pub source_amount_swapped: u64,
    /// Amount of destination tokens received
    pub destination_amount_swapped: u64,
    /// Total fees charged (in source tokens)
    pub total_fees: u64,
}

/// Calculate fees from a Fees struct
fn calculate_fees(fees: &Fees, amount_in: u128) -> Result<(u128, u128)> {
    let kdex_fees = kdex_curve::Fees {
        trade_fee_numerator: fees.trade_fee_numerator,
        trade_fee_denominator: fees.trade_fee_denominator,
        owner_trade_fee_numerator: fees.owner_trade_fee_numerator,
        owner_trade_fee_denominator: fees.owner_trade_fee_denominator,
        owner_withdraw_fee_numerator: fees.owner_withdraw_fee_numerator,
        owner_withdraw_fee_denominator: fees.owner_withdraw_fee_denominator,
        host_fee_numerator: fees.host_fee_numerator,
        host_fee_denominator: fees.host_fee_denominator,
    };

    let trade_fee = kdex_fees
        .trading_fee(amount_in)
        .map_err(|e| QuoteError::CalculationError(e.to_string()))?;
    let owner_fee = kdex_fees
        .owner_trading_fee(amount_in)
        .map_err(|e| QuoteError::CalculationError(e.to_string()))?;

    Ok((trade_fee, owner_fee))
}

/// Calculate quote for ConstantProduct curve
pub fn calculate_constant_product_quote(
    fees: &Fees,
    amount_in: u64,
    pool_source_amount: u64,
    pool_destination_amount: u64,
    _curve_data: &[u8],
) -> Result<SwapResult> {
    let (trade_fee, owner_fee) = calculate_fees(fees, amount_in as u128)?;
    let total_fees = trade_fee
        .checked_add(owner_fee)
        .ok_or_else(|| QuoteError::CalculationError("Fee overflow".into()))?;

    let source_amount_less_fees = (amount_in as u128)
        .checked_sub(total_fees)
        .ok_or_else(|| QuoteError::CalculationError("Amount too small to cover fees".into()))?;

    // Constant product: x * y = k
    // new_y = k / new_x = (x * y) / (x + dx) = y - y * dx / (x + dx)
    let invariant = (pool_source_amount as u128)
        .checked_mul(pool_destination_amount as u128)
        .ok_or_else(|| QuoteError::CalculationError("Invariant overflow".into()))?;

    let new_source = (pool_source_amount as u128)
        .checked_add(source_amount_less_fees)
        .ok_or_else(|| QuoteError::CalculationError("New source overflow".into()))?;

    let new_destination = invariant
        .checked_div(new_source)
        .ok_or_else(|| QuoteError::CalculationError("Division by zero".into()))?;

    let destination_amount = (pool_destination_amount as u128)
        .checked_sub(new_destination)
        .ok_or_else(|| QuoteError::CalculationError("Destination underflow".into()))?;

    Ok(SwapResult {
        source_amount_swapped: source_amount_less_fees as u64,
        destination_amount_swapped: destination_amount as u64,
        total_fees: total_fees as u64,
    })
}

/// Calculate quote for ConstantPrice curve
pub fn calculate_constant_price_quote(
    fees: &Fees,
    amount_in: u64,
    destination_vault_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
) -> Result<SwapResult> {
    let curve = ConstantPriceCurve::deserialize(&mut &curve_data[..])
        .map_err(|e| QuoteError::DeserializationError(e.to_string()))?;

    let (trade_fee, owner_fee) = calculate_fees(fees, amount_in as u128)?;
    let total_fees = trade_fee
        .checked_add(owner_fee)
        .ok_or_else(|| QuoteError::CalculationError("Fee overflow".into()))?;

    let source_amount_less_fees = (amount_in as u128)
        .checked_sub(total_fees)
        .ok_or_else(|| QuoteError::CalculationError("Amount too small to cover fees".into()))?;

    // ConstantPrice: token_b_price tokens of A = 1 token of B
    let destination_amount = match trade_direction {
        TradeDirection::AtoB => {
            // Selling A for B: output = input / price
            source_amount_less_fees
                .checked_div(curve.token_b_price as u128)
                .ok_or_else(|| QuoteError::CalculationError("Division by zero".into()))?
        }
        TradeDirection::BtoA => {
            // Selling B for A: output = input * price
            source_amount_less_fees
                .checked_mul(curve.token_b_price as u128)
                .ok_or_else(|| QuoteError::CalculationError("Multiplication overflow".into()))?
        }
    };

    if destination_amount > destination_vault_amount as u128 {
        return Err(QuoteError::InsufficientLiquidity {
            required: destination_amount as u64,
            available: destination_vault_amount,
        });
    }

    Ok(SwapResult {
        source_amount_swapped: source_amount_less_fees as u64,
        destination_amount_swapped: destination_amount as u64,
        total_fees: total_fees as u64,
    })
}

/// Calculate quote for Offset curve
pub fn calculate_offset_quote(
    fees: &Fees,
    amount_in: u64,
    pool_source_amount: u64,
    pool_destination_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
) -> Result<SwapResult> {
    let curve = OffsetCurve::deserialize(&mut &curve_data[..])
        .map_err(|e| QuoteError::DeserializationError(e.to_string()))?;

    let (trade_fee, owner_fee) = calculate_fees(fees, amount_in as u128)?;
    let total_fees = trade_fee
        .checked_add(owner_fee)
        .ok_or_else(|| QuoteError::CalculationError("Fee overflow".into()))?;

    let source_amount_less_fees = (amount_in as u128)
        .checked_sub(total_fees)
        .ok_or_else(|| QuoteError::CalculationError("Amount too small to cover fees".into()))?;

    // Offset curve: constant product with virtual offset on token B
    let (effective_source, effective_dest) = match trade_direction {
        TradeDirection::AtoB => (
            pool_source_amount as u128,
            (pool_destination_amount as u128)
                .checked_add(curve.token_b_offset as u128)
                .ok_or_else(|| QuoteError::CalculationError("Offset overflow".into()))?,
        ),
        TradeDirection::BtoA => (
            (pool_source_amount as u128)
                .checked_add(curve.token_b_offset as u128)
                .ok_or_else(|| QuoteError::CalculationError("Offset overflow".into()))?,
            pool_destination_amount as u128,
        ),
    };

    let invariant = effective_source
        .checked_mul(effective_dest)
        .ok_or_else(|| QuoteError::CalculationError("Invariant overflow".into()))?;

    let new_source = effective_source
        .checked_add(source_amount_less_fees)
        .ok_or_else(|| QuoteError::CalculationError("New source overflow".into()))?;

    let new_destination = invariant
        .checked_div(new_source)
        .ok_or_else(|| QuoteError::CalculationError("Division by zero".into()))?;

    let destination_amount = effective_dest
        .checked_sub(new_destination)
        .ok_or_else(|| QuoteError::CalculationError("Destination underflow".into()))?;

    Ok(SwapResult {
        source_amount_swapped: source_amount_less_fees as u64,
        destination_amount_swapped: destination_amount as u64,
        total_fees: total_fees as u64,
    })
}

/// Calculate quote for Stable curve
pub fn calculate_stable_quote(
    fees: &Fees,
    amount_in: u64,
    pool_source_amount: u64,
    pool_destination_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
) -> Result<SwapResult> {
    let curve = StableCurve::deserialize(&mut &curve_data[..])
        .map_err(|e| QuoteError::DeserializationError(e.to_string()))?;

    let (trade_fee, owner_fee) = calculate_fees(fees, amount_in as u128)?;
    let total_fees = trade_fee
        .checked_add(owner_fee)
        .ok_or_else(|| QuoteError::CalculationError("Fee overflow".into()))?;

    let source_amount_less_fees = (amount_in as u128)
        .checked_sub(total_fees)
        .ok_or_else(|| QuoteError::CalculationError("Amount too small to cover fees".into()))?;

    // Scale amounts by factors for stable swap calculation
    let (source_factor, dest_factor) = match trade_direction {
        TradeDirection::AtoB => (curve.token_a_factor, curve.token_b_factor),
        TradeDirection::BtoA => (curve.token_b_factor, curve.token_a_factor),
    };

    // Apply scaling factors
    let scaled_source = (pool_source_amount as u128)
        .checked_mul(source_factor as u128)
        .ok_or_else(|| QuoteError::CalculationError("Scaling overflow".into()))?;
    let scaled_dest = (pool_destination_amount as u128)
        .checked_mul(dest_factor as u128)
        .ok_or_else(|| QuoteError::CalculationError("Scaling overflow".into()))?;
    let scaled_input = source_amount_less_fees
        .checked_mul(source_factor as u128)
        .ok_or_else(|| QuoteError::CalculationError("Scaling overflow".into()))?;

    // Use stable swap calculation (simplified version)
    // For full accuracy, this should use the actual StableSwap invariant with amplification
    // This is a simplified constant-product approximation for quote estimation
    let _d = scaled_source
        .checked_add(scaled_dest)
        .ok_or_else(|| QuoteError::CalculationError("D overflow".into()))?;

    let new_scaled_source = scaled_source
        .checked_add(scaled_input)
        .ok_or_else(|| QuoteError::CalculationError("New source overflow".into()))?;

    // Apply amplification coefficient
    let amp = curve.amp as u128;

    // Stable swap formula approximation
    // y = (D + y * A * 2) / (A * 2 + x / y)
    // Simplified: use weighted constant product
    let product = scaled_source
        .checked_mul(scaled_dest)
        .ok_or_else(|| QuoteError::CalculationError("Product overflow".into()))?;

    // Blend between constant product and constant sum based on amp
    let const_product_out = scaled_dest
        .checked_sub(
            product
                .checked_div(new_scaled_source)
                .ok_or_else(|| QuoteError::CalculationError("Division by zero".into()))?,
        )
        .ok_or_else(|| QuoteError::CalculationError("Underflow".into()))?;

    let output_scaled = if amp > 0 {
        // Higher amp = closer to constant sum (1:1)
        let const_sum_out = scaled_input;
        let blend = amp.min(100);
        let product_weight = 100u128.saturating_sub(blend);
        let sum_weight = blend;

        const_product_out
            .checked_mul(product_weight)
            .and_then(|p| const_sum_out.checked_mul(sum_weight).map(|s| (p, s)))
            .and_then(|(p, s)| p.checked_add(s))
            .and_then(|sum| sum.checked_div(100))
            .ok_or_else(|| QuoteError::CalculationError("Blend calculation error".into()))?
    } else {
        const_product_out
    };

    // Unscale output
    let destination_amount = output_scaled
        .checked_div(dest_factor as u128)
        .ok_or_else(|| QuoteError::CalculationError("Unscaling error".into()))?;

    Ok(SwapResult {
        source_amount_swapped: source_amount_less_fees as u64,
        destination_amount_swapped: destination_amount as u64,
        total_fees: total_fees as u64,
    })
}

/// Calculate quote for any non-oracle curve type
pub fn calculate_quote(
    curve_type: CurveType,
    fees: &Fees,
    amount_in: u64,
    pool_source_amount: u64,
    pool_destination_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
) -> Result<SwapResult> {
    match curve_type {
        CurveType::ConstantProduct => calculate_constant_product_quote(
            fees,
            amount_in,
            pool_source_amount,
            pool_destination_amount,
            curve_data,
        ),
        CurveType::ConstantPrice => calculate_constant_price_quote(
            fees,
            amount_in,
            pool_destination_amount,
            trade_direction,
            curve_data,
        ),
        CurveType::Offset => calculate_offset_quote(
            fees,
            amount_in,
            pool_source_amount,
            pool_destination_amount,
            trade_direction,
            curve_data,
        ),
        CurveType::Stable => calculate_stable_quote(
            fees,
            amount_in,
            pool_source_amount,
            pool_destination_amount,
            trade_direction,
            curve_data,
        ),
        CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle => {
            Err(QuoteError::UnsupportedCurveType(curve_type))
        }
    }
}
