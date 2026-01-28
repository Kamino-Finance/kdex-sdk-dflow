//! Oracle price fetching and calculation support for oracle-based curves
//!
//! This module provides functionality to fetch Scope oracle prices and calculate
//! quotes for oracle-based curves (ConstantSpreadOracle and InventorySkewOracle).
//!
//! ## Price Chain Mechanism
//!
//! Supports price chains - arrays of up to 4 Scope price indices that get multiplied together:
//! - Direct prices: `[X, MAX, MAX, MAX]` → single price at index X
//! - Derived prices: `[X, Y, MAX, MAX]` → `prices[X] * prices[Y]` (e.g., for LSTs)

use crate::error::{Result, SdkError};
use hyperplane::curve::fees::Fees;
use kdex_curve::oracle::InventorySkewParams;
pub use kdex_curve::TradeDirection;
use solana_sdk::account::Account;
use std::mem::size_of;

/// Price chain terminator value (u16::MAX)
const PRICE_CHAIN_TERMINATOR: u16 = u16::MAX;

/// Convert hyperplane TradeDirection to kdex_curve TradeDirection
pub fn convert_trade_direction(
    direction: hyperplane::curve::calculator::TradeDirection,
) -> TradeDirection {
    match direction {
        hyperplane::curve::calculator::TradeDirection::AtoB => TradeDirection::AtoB,
        hyperplane::curve::calculator::TradeDirection::BtoA => TradeDirection::BtoA,
    }
}

/// Fetches a single Scope oracle price from the price feed account
///
/// # Arguments
/// * `price_feed_account` - The Scope OraclePrices account data
/// * `price_index` - Index of the price in the feed (0-511)
///
/// # Returns
/// * `(price_value, price_exp)` - The price value and exponent
///
/// # Errors
/// * `InvalidOracleConfig` - If price index is out of bounds or account is invalid
pub fn fetch_scope_price(price_feed_account: &Account, price_index: u16) -> Result<(u64, i32)> {
    // Validate price index is within bounds
    if (price_index as usize) >= scope_types::MAX_ENTRIES {
        return Err(SdkError::InvalidOracleConfig(format!(
            "Price index {} out of bounds (max: {})",
            price_index,
            scope_types::MAX_ENTRIES
        )));
    }

    // Validate account owner is the Scope program
    if price_feed_account.owner != scope_types::id() {
        return Err(SdkError::InvalidOracleConfig(format!(
            "Invalid Scope account owner. Expected: {}, Got: {}",
            scope_types::id(),
            price_feed_account.owner
        )));
    }

    // OraclePrices layout: discriminator(8) + oracle_mappings(32) + prices[512]
    // Each DatedPrice is size_of::<DatedPrice>() bytes
    let dated_price_size = size_of::<scope_types::DatedPrice>();
    let offset = 8_usize
        .checked_add(32)
        .and_then(|base| {
            (price_index as usize)
                .checked_mul(dated_price_size)
                .and_then(|product| base.checked_add(product))
        })
        .ok_or_else(|| SdkError::InvalidOracleConfig("Offset calculation overflow".into()))?;

    let end_offset = offset
        .checked_add(dated_price_size)
        .ok_or_else(|| SdkError::InvalidOracleConfig("End offset calculation overflow".into()))?;

    if price_feed_account.data.len() < end_offset {
        return Err(SdkError::InvalidOracleConfig(format!(
            "Account data too short for price index {}",
            price_index
        )));
    }

    // Deserialize the DatedPrice at the specified index
    let dated_price: &scope_types::DatedPrice =
        bytemuck::from_bytes(&price_feed_account.data[offset..end_offset]);

    // Note: We don't validate price age here since this is a quote/simulation
    // The actual on-chain transaction will validate the price age

    // Return price value and exponent (convert u64 exp to i32)
    Ok((dated_price.price.value, dated_price.price.exp as i32))
}

/// Fetches and multiplies prices from a Scope oracle price chain
///
/// # Arguments
/// * `price_feed_account` - The Scope OraclePrices account data
/// * `price_chain` - Array of up to 4 price indices (65535 terminates the chain)
///
/// # Returns
/// * `(combined_value, combined_exp)` - The combined price value and exponent
pub fn fetch_scope_price_chain(
    price_feed_account: &Account,
    price_chain: &[u16; 4],
) -> Result<(u128, i32)> {
    // Validate account owner is the Scope program
    if price_feed_account.owner != scope_types::id() {
        return Err(SdkError::InvalidOracleConfig(format!(
            "Invalid Scope account owner. Expected: {}, Got: {}",
            scope_types::id(),
            price_feed_account.owner
        )));
    }

    // Count valid indices in chain
    let chain_len = price_chain
        .iter()
        .take_while(|&&idx| idx != PRICE_CHAIN_TERMINATOR)
        .count();

    if chain_len == 0 {
        return Err(SdkError::InvalidOracleConfig("Price chain is empty".into()));
    }

    // Fetch first price
    let (first_value, first_exp) = fetch_scope_price(price_feed_account, price_chain[0])?;
    let mut combined_value = first_value as u128;
    let mut combined_exp = first_exp;

    // Multiply remaining prices in the chain
    for &price_index in price_chain.iter().skip(1).take(chain_len.saturating_sub(1)) {
        let (value, exp) = fetch_scope_price(price_feed_account, price_index)?;
        combined_value = combined_value
            .checked_mul(value as u128)
            .ok_or_else(|| SdkError::CurveError("Price multiplication overflow".into()))?;
        combined_exp = combined_exp
            .checked_add(exp)
            .ok_or_else(|| SdkError::CurveError("Exponent addition overflow".into()))?;
    }

    Ok((combined_value, combined_exp))
}

/// Calculate quote for ConstantSpreadOracle curve
///
/// This replicates the on-chain logic from the swap handler
///
/// # Arguments
/// * `fees` - Pool fees configuration
/// * `amount_in` - Amount to swap in
/// * `trade_direction` - Direction of the trade
/// * `curve_data` - Raw curve account data
/// * `scope_price_feed_account` - Scope oracle account
///
/// # Returns
/// * Quote with in_amount, out_amount, and total_fees
pub fn calculate_constant_spread_quote(
    fees: &Fees,
    amount_in: u64,
    trade_direction: hyperplane::curve::calculator::TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
) -> Result<(u64, u64, u64)> {
    let trade_direction = convert_trade_direction(trade_direction);
    // Deserialize the full ConstantSpreadOracleCurve to access all parameters
    let mut data = curve_data;
    let curve: hyperplane::state::ConstantSpreadOracleCurve =
        anchor_lang::AccountDeserialize::try_deserialize(&mut data)
            .map_err(|e| SdkError::DeserializationError(e.to_string()))?;

    // Fetch Scope price chain
    let (price_value, price_exp) =
        fetch_scope_price_chain(scope_price_feed_account, &curve.price_chain)?;

    // Calculate fees (same as on-chain)
    let trade_fee = fees
        .trading_fee(amount_in as u128)
        .map_err(|e| SdkError::CurveError(e.to_string()))?;
    let owner_fee = fees
        .owner_trading_fee(amount_in as u128)
        .map_err(|e| SdkError::CurveError(e.to_string()))?;
    let total_fees = trade_fee
        .checked_add(owner_fee)
        .ok_or_else(|| SdkError::CurveError("Fee calculation overflow".into()))?;
    let source_amount_less_fees = (amount_in as u128)
        .checked_sub(total_fees)
        .ok_or_else(|| SdkError::CurveError("Amount too small to cover fees".into()))?;

    // Calculate swap using ConstantSpread logic from kdex-curve
    let swap_result = kdex_curve::oracle::constant_spread_swap(
        source_amount_less_fees,
        price_value as u64,
        price_exp as u64,
        curve.bps_from_oracle,
        trade_direction,
    )
    .map_err(|e| SdkError::CurveError(e.to_string()))?;

    Ok((
        swap_result.source_amount_swapped as u64,
        swap_result.destination_amount_swapped as u64,
        total_fees as u64,
    ))
}

/// Calculate quote for InventorySkewOracle curve
///
/// This replicates the on-chain logic from the swap handler
///
/// # Arguments
/// * `fees` - Pool fees configuration
/// * `amount_in` - Amount to swap in
/// * `source_vault_amount` - Current balance in source vault
/// * `trade_direction` - Direction of the trade
/// * `curve_data` - Raw curve account data
/// * `scope_price_feed_account` - Scope oracle account
///
/// # Returns
/// * Quote with in_amount, out_amount, and total_fees
pub fn calculate_inventory_skew_quote(
    fees: &Fees,
    amount_in: u64,
    source_vault_amount: u64,
    trade_direction: hyperplane::curve::calculator::TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
) -> Result<(u64, u64, u64)> {
    let trade_direction = convert_trade_direction(trade_direction);
    // Deserialize the full InventorySkewOracleCurve to access all parameters
    let mut data = curve_data;
    let curve: hyperplane::state::InventorySkewOracleCurve =
        anchor_lang::AccountDeserialize::try_deserialize(&mut data)
            .map_err(|e| SdkError::DeserializationError(e.to_string()))?;

    // Fetch Scope price chain
    let (price_value, price_exp) =
        fetch_scope_price_chain(scope_price_feed_account, &curve.price_chain)?;

    // Calculate fees (same as on-chain)
    let trade_fee = fees
        .trading_fee(amount_in as u128)
        .map_err(|e| SdkError::CurveError(e.to_string()))?;
    let owner_fee = fees
        .owner_trading_fee(amount_in as u128)
        .map_err(|e| SdkError::CurveError(e.to_string()))?;
    let total_fees = trade_fee
        .checked_add(owner_fee)
        .ok_or_else(|| SdkError::CurveError("Fee calculation overflow".into()))?;
    let source_amount_less_fees = (amount_in as u128)
        .checked_sub(total_fees)
        .ok_or_else(|| SdkError::CurveError("Amount too small to cover fees".into()))?;

    // Convert curve parameters to kdex-curve InventorySkewParams
    let params = InventorySkewParams::new(
        curve.base_spread_bps,
        curve.size_spread_bps,
        curve.skew_bps,
        curve.inv_equilibrium,
        curve.inv_max,
        curve.q_ref,
        curve.alpha,
    );

    // Calculate swap using InventorySkew logic from kdex-curve
    let swap_result = kdex_curve::oracle::inventory_skew_swap(
        source_amount_less_fees,
        price_value as u64,
        price_exp as u64,
        trade_direction,
        source_vault_amount as u128,
        &params,
    )
    .map_err(|e| SdkError::CurveError(e.to_string()))?;

    Ok((
        swap_result.source_amount_swapped as u64,
        swap_result.destination_amount_swapped as u64,
        total_fees as u64,
    ))
}
