//! Oracle price fetching and calculation support for oracle-based curves
//!
//! This module re-exports oracle functionality from kdex-client with
//! error conversion to SDK-specific error types.
//!
//! ## Price Chain Mechanism
//!
//! Supports price chains - arrays of up to 4 Scope price indices that get multiplied together:
//! - Direct prices: `[X, MAX, MAX, MAX]` → single price at index X
//! - Derived prices: `[X, Y, MAX, MAX]` → `prices[X] * prices[Y]` (e.g., for LSTs)

use crate::error::{Result, SdkError};
use kdex_client::generated::types::Fees;
use kdex_client::TradeDirection;
use solana_sdk::account::Account;

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
pub fn fetch_scope_price(price_feed_account: &Account, price_index: u16) -> Result<(u64, u64)> {
    kdex_client::oracle::fetch_scope_price(price_feed_account, price_index)
        .map_err(oracle_error_to_sdk_error)
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
) -> Result<(u128, u64)> {
    kdex_client::oracle::fetch_scope_price_chain(price_feed_account, price_chain)
        .map_err(oracle_error_to_sdk_error)
}

/// Calculate quote for ConstantSpreadOracle curve
///
/// This replicates the on-chain logic from the swap handler
///
/// # Arguments
/// * `fees` - Pool fees configuration
/// * `amount_in` - Amount to swap in
/// * `destination_vault_amount` - Current balance in destination vault
/// * `trade_direction` - Direction of the trade
/// * `curve_data` - Raw curve account data
/// * `scope_price_feed_account` - Scope oracle account
/// * `token_a_decimals` - Decimals of token A mint
/// * `token_b_decimals` - Decimals of token B mint
///
/// # Returns
/// * Quote with in_amount, out_amount, and total_fees
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
///
/// This replicates the on-chain logic from the swap handler
///
/// # Arguments
/// * `fees` - Pool fees configuration
/// * `amount_in` - Amount to swap in
/// * `source_vault_amount` - Current balance in source vault
/// * `destination_vault_amount` - Current balance in destination vault
/// * `trade_direction` - Direction of the trade
/// * `curve_data` - Raw curve account data
/// * `scope_price_feed_account` - Scope oracle account
/// * `token_a_decimals` - Decimals of token A mint
/// * `token_b_decimals` - Decimals of token B mint
///
/// # Returns
/// * Quote with in_amount, out_amount, and total_fees
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
