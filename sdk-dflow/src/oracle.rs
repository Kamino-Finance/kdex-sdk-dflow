//! Oracle price fetching and calculation support for oracle-based curves
//!
//! This module re-exports shared oracle functionality from kdex-client with
//! error conversion to SDK-specific error types.
//!
//! The `_with_score` variants apply score-based spread widening and are
//! self-contained implementations (not delegating to kdex-client).

use crate::error::{Result, SdkError};
use anchor_lang::AccountDeserialize;
use kdex_client::curves::{ConstantSpreadOracleCurve, InventorySkewOracleCurve};
use kdex_client::generated::types::Fees;
use kdex_client::TradeDirection;
use kdex_curve::oracle::InventorySkewParams;
use solana_sdk::account::Account;

// Re-export types from kdex-client
pub use kdex_client::TradeDirection as ReexportedTradeDirection;

/// Fetches a single Scope oracle price from the price feed account
pub fn fetch_scope_price(price_feed_account: &Account, price_index: u16) -> Result<(u64, u64)> {
    kdex_client::oracle::fetch_scope_price(price_feed_account, price_index)
        .map_err(oracle_error_to_sdk_error)
}

/// Fetches the unix timestamp for a single Scope oracle price entry.
///
/// Returns `None` if the index is out of bounds, the account owner is wrong,
/// or the account data is too short.
pub fn fetch_scope_price_timestamp(price_feed_account: &Account, price_index: u16) -> Option<u64> {
    use std::mem::size_of;

    let scope_id = kdex_client::scope_types::id();
    if price_feed_account.owner != scope_id {
        return None;
    }
    if price_index as usize >= kdex_client::scope_types::MAX_ENTRIES {
        return None;
    }

    let entry_size = size_of::<kdex_client::scope_types::DatedPrice>();
    let offset = 8usize
        .checked_add(32)?
        .checked_add((price_index as usize).checked_mul(entry_size)?)?;
    let end = offset.checked_add(entry_size)?;

    if price_feed_account.data.len() < end {
        return None;
    }

    let dated_price: &kdex_client::scope_types::DatedPrice =
        bytemuck::from_bytes(&price_feed_account.data[offset..end]);
    Some(dated_price.unix_timestamp)
}

/// Fetches and multiplies prices from a Scope oracle price chain
pub fn fetch_scope_price_chain(
    price_feed_account: &Account,
    price_chain: &[u16; 4],
) -> Result<(u128, u64)> {
    kdex_client::oracle::fetch_scope_price_chain(price_feed_account, price_chain)
        .map_err(oracle_error_to_sdk_error)
}

/// Calculate quote for ConstantSpreadOracle curve
#[allow(clippy::too_many_arguments)]
pub fn calculate_constant_spread_quote(
    fees: &Fees,
    amount_in: u64,
    destination_vault_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
    token_a_decimals: u8,
    token_b_decimals: u8,
) -> Result<(u64, u64, u64)> {
    kdex_client::oracle::calculate_constant_spread_quote(
        fees,
        amount_in,
        destination_vault_amount,
        trade_direction,
        curve_data,
        scope_price_feed_account,
        token_a_decimals,
        token_b_decimals,
    )
    .map_err(oracle_error_to_sdk_error)
}

/// Calculate quote for InventorySkewOracle curve
#[allow(clippy::too_many_arguments)]
pub fn calculate_inventory_skew_quote(
    fees: &Fees,
    amount_in: u64,
    source_vault_amount: u64,
    destination_vault_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
    token_a_decimals: u8,
    token_b_decimals: u8,
) -> Result<(u64, u64, u64)> {
    kdex_client::oracle::calculate_inventory_skew_quote(
        fees,
        amount_in,
        source_vault_amount,
        destination_vault_amount,
        trade_direction,
        curve_data,
        scope_price_feed_account,
        token_a_decimals,
        token_b_decimals,
    )
    .map_err(oracle_error_to_sdk_error)
}

/// Calculate quote for ConstantSpreadOracle curve with score-based spread widening
///
/// Same as `calculate_constant_spread_quote` but widens `bps_from_oracle` by
/// `score_multiplier_bps`: `bps = bps * (10000 + score_multiplier_bps) / 10000`
#[allow(clippy::too_many_arguments)]
pub fn calculate_constant_spread_quote_with_score(
    fees: &Fees,
    amount_in: u64,
    destination_vault_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
    token_a_decimals: u8,
    token_b_decimals: u8,
    score_multiplier_bps: u64,
) -> Result<(u64, u64, u64)> {
    let mut data = curve_data;
    let curve: ConstantSpreadOracleCurve = ConstantSpreadOracleCurve::try_deserialize(&mut data)
        .map_err(|e| SdkError::DeserializationError(e.to_string()))?;

    let (price_value, price_exp) =
        fetch_scope_price_chain(scope_price_feed_account, &curve.price_chain)?;

    let price_exp = adjust_price_exp(price_exp, token_a_decimals, token_b_decimals)?;

    let curve_fees = to_curve_fees(fees);
    let (source_amount_less_fees, total_fees) = calculate_fees(&curve_fees, amount_in)?;

    let adjusted_price = kdex_curve::oracle::apply_price_offset(
        price_value,
        curve
            .price_offset_bps
            .checked_neg()
            .ok_or_else(|| SdkError::CurveError("Price offset negate overflow".into()))?,
    )
    .map_err(|e| SdkError::CurveError(e.to_string()))?;

    // Apply score-based spread widening
    let bps_from_oracle = if score_multiplier_bps > 0 {
        curve
            .bps_from_oracle
            .checked_mul(
                10000u64
                    .checked_add(score_multiplier_bps)
                    .ok_or_else(|| SdkError::CurveError("Score widening overflow".into()))?,
            )
            .ok_or_else(|| SdkError::CurveError("Score widening overflow".into()))?
            / 10000
    } else {
        curve.bps_from_oracle
    };

    let swap_result = kdex_curve::oracle::constant_spread_swap(
        source_amount_less_fees,
        adjusted_price,
        price_exp,
        bps_from_oracle,
        trade_direction,
    )
    .map_err(|e| SdkError::CurveError(e.to_string()))?;

    if swap_result.destination_amount_swapped > destination_vault_amount as u128 {
        return Err(SdkError::InsufficientLiquidity {
            required: swap_result.destination_amount_swapped as u64,
            available: destination_vault_amount,
        });
    }

    Ok((
        swap_result.source_amount_swapped as u64,
        swap_result.destination_amount_swapped as u64,
        total_fees as u64,
    ))
}

/// Calculate quote for InventorySkewOracle curve with score-based spread widening
///
/// Same as `calculate_inventory_skew_quote` but widens `base_spread_bps` by
/// `score_multiplier_bps`: `bps = bps * (10000 + score_multiplier_bps) / 10000`
#[allow(clippy::too_many_arguments)]
pub fn calculate_inventory_skew_quote_with_score(
    fees: &Fees,
    amount_in: u64,
    source_vault_amount: u64,
    destination_vault_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
    token_a_decimals: u8,
    token_b_decimals: u8,
    score_multiplier_bps: u64,
) -> Result<(u64, u64, u64)> {
    let mut data = curve_data;
    let curve: InventorySkewOracleCurve = InventorySkewOracleCurve::try_deserialize(&mut data)
        .map_err(|e| SdkError::DeserializationError(e.to_string()))?;

    let (price_value, price_exp) =
        fetch_scope_price_chain(scope_price_feed_account, &curve.price_chain)?;

    let price_exp = adjust_price_exp(price_exp, token_a_decimals, token_b_decimals)?;

    let curve_fees = to_curve_fees(fees);
    let (source_amount_less_fees, total_fees) = calculate_fees(&curve_fees, amount_in)?;

    let adjusted_price = kdex_curve::oracle::apply_price_offset(
        price_value,
        curve
            .price_offset_bps
            .checked_neg()
            .ok_or_else(|| SdkError::CurveError("Price offset negate overflow".into()))?,
    )
    .map_err(|e| SdkError::CurveError(e.to_string()))?;

    // Apply score-based spread widening to base spread
    let base_spread_bps = if score_multiplier_bps > 0 {
        curve
            .base_spread_bps
            .checked_mul(
                10000u64
                    .checked_add(score_multiplier_bps)
                    .ok_or_else(|| SdkError::CurveError("Score widening overflow".into()))?,
            )
            .ok_or_else(|| SdkError::CurveError("Score widening overflow".into()))?
            / 10000
    } else {
        curve.base_spread_bps
    };

    let params = InventorySkewParams::new(
        base_spread_bps,
        curve.size_spread_bps,
        curve.skew_bps,
        curve.inv_equilibrium,
        curve.inv_max,
        curve.q_ref,
        curve.alpha,
    );

    let (current_inventory_ratio, swap_size_ratio) =
        kdex_curve::oracle::inventory_skew::calculate_ratios(
            source_amount_less_fees,
            adjusted_price,
            price_exp,
            trade_direction,
            source_vault_amount as u128,
            destination_vault_amount as u128,
        )
        .map_err(|e| SdkError::CurveError(e.to_string()))?;

    let swap_result = kdex_curve::oracle::inventory_skew_swap(
        source_amount_less_fees,
        adjusted_price,
        price_exp,
        trade_direction,
        current_inventory_ratio,
        swap_size_ratio,
        &params,
    )
    .map_err(|e| SdkError::CurveError(e.to_string()))?;

    if swap_result.destination_amount_swapped > destination_vault_amount as u128 {
        return Err(SdkError::InsufficientLiquidity {
            required: swap_result.destination_amount_swapped as u64,
            available: destination_vault_amount,
        });
    }

    Ok((
        swap_result.source_amount_swapped as u64,
        swap_result.destination_amount_swapped as u64,
        total_fees as u64,
    ))
}

/// Convert generated Fees to kdex_curve::Fees for calculations
fn to_curve_fees(fees: &Fees) -> kdex_curve::Fees {
    kdex_curve::Fees {
        trade_fee_numerator: fees.trade_fee_numerator,
        trade_fee_denominator: fees.trade_fee_denominator,
        owner_trade_fee_numerator: fees.owner_trade_fee_numerator,
        owner_trade_fee_denominator: fees.owner_trade_fee_denominator,
        owner_withdraw_fee_numerator: fees.owner_withdraw_fee_numerator,
        owner_withdraw_fee_denominator: fees.owner_withdraw_fee_denominator,
        host_fee_numerator: fees.host_fee_numerator,
        host_fee_denominator: fees.host_fee_denominator,
    }
}

/// Adjust price exponent for token decimal difference
fn adjust_price_exp(price_exp: u64, token_a_decimals: u8, token_b_decimals: u8) -> Result<u64> {
    if token_a_decimals >= token_b_decimals {
        price_exp
            .checked_add(u64::from(token_a_decimals.abs_diff(token_b_decimals)))
            .ok_or_else(|| SdkError::CurveError("Price exponent overflow".into()))
    } else {
        price_exp
            .checked_sub(u64::from(token_b_decimals.abs_diff(token_a_decimals)))
            .ok_or_else(|| SdkError::CurveError("Price exponent underflow".into()))
    }
}

/// Calculate fees and return (source_amount_less_fees, total_fees) as u128
fn calculate_fees(curve_fees: &kdex_curve::Fees, amount_in: u64) -> Result<(u128, u128)> {
    let trade_fee = curve_fees
        .trading_fee(amount_in as u128)
        .map_err(|e| SdkError::CurveError(e.to_string()))?;
    let owner_fee = curve_fees
        .owner_trading_fee(amount_in as u128)
        .map_err(|e| SdkError::CurveError(e.to_string()))?;
    let total_fees = trade_fee
        .checked_add(owner_fee)
        .ok_or_else(|| SdkError::CurveError("Fee calculation overflow".into()))?;
    let source_amount_less_fees = (amount_in as u128)
        .checked_sub(total_fees)
        .ok_or_else(|| SdkError::CurveError("Amount too small to cover fees".into()))?;
    Ok((source_amount_less_fees, total_fees))
}

/// Convert kdex-client OracleError to SDK-specific SdkError
fn oracle_error_to_sdk_error(err: kdex_client::oracle::OracleError) -> SdkError {
    match err {
        kdex_client::oracle::OracleError::InvalidOracleConfig(msg) => {
            SdkError::InvalidOracleConfig(msg)
        }
        kdex_client::oracle::OracleError::CurveError(msg) => SdkError::CurveError(msg),
        kdex_client::oracle::OracleError::DeserializationError(msg) => {
            SdkError::DeserializationError(msg)
        }
        kdex_client::oracle::OracleError::InsufficientLiquidity {
            required,
            available,
        } => SdkError::InsufficientLiquidity {
            required,
            available,
        },
    }
}
